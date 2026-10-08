//! 可恢复的仓库门禁调度、内容指纹和证据记录。

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::Command,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

type DynError = Box<dyn std::error::Error>;

const STATE_VERSION: u32 = 1;
// 旧缓存可能来自输入变化后仍写入 PASS 的调度器；改变键以拒绝复用该证据。
const CACHE_POLICY_VERSION: &[u8] = b"stable-input-v2";
const CACHE_ENVIRONMENT: &[&str] = &[
    "CARGO_BUILD_TARGET",
    "CARGO_INCREMENTAL",
    "CARGO_TARGET_DIR",
    "RUSTDOCFLAGS",
    "RUSTFLAGS",
    "RUSTUP_TOOLCHAIN",
];

/// 执行 `test-gate` 维护命令。
pub fn run(arguments: Vec<String>) -> Result<(), DynError> {
    let options = Options::parse(arguments)?;
    let workspace = workspace_root();
    let summary = run_spec(&workspace, &options.spec, options.force)?;
    println!("test-gate {}: {}", summary.task_id, summary.status.as_str());
    if summary.status == RunStatus::Pass {
        Ok(())
    } else {
        Err(format!(
            "test-gate {} finished with {}",
            summary.task_id,
            summary.status.as_str()
        )
        .into())
    }
}

/// 根据规范运行门禁；进程失败记录为结果，规范或证据错误才直接返回错误。
fn run_spec(workspace: &Path, spec_path: &Path, force: bool) -> Result<RunSummary, DynError> {
    let started = Instant::now();
    let started_unix_ms = unix_millis();
    let workspace = workspace.canonicalize()?;
    let spec_path = absolute_path(&workspace, spec_path);
    let spec_bytes = fs::read(&spec_path)?;
    let spec: GateSpec = toml::from_str(std::str::from_utf8(&spec_bytes)?)?;
    let output = validate_spec(&workspace, &spec_path, &spec)?;
    let toolchain = Toolchain::collect(&workspace)?;
    let before = InputSnapshot::collect(&workspace, &spec.inputs)?;
    before.write_tsv(&output.join("fingerprints-before.tsv"))?;

    let state_path = output.join("state.json");
    let mut cache = CacheState::load(&state_path)?;
    let mut pending_passes = BTreeMap::new();
    let mut results = Vec::with_capacity(spec.gates.len());
    let mut tool_duration_ms = 0_u64;
    let mut blocked = false;
    let mut stale = false;

    for (index, gate) in spec.gates.iter().enumerate() {
        let cache_key = gate_cache_key(&spec_bytes, &before.digest, &toolchain, gate);
        let log_name = format!("{:02}-{}.log", index + 1, safe_name(&gate.name));
        let log_path = output.join(&log_name);

        if !blocked {
            let current = InputSnapshot::collect(&workspace, &spec.inputs)?;
            if current.digest != before.digest {
                current.write_tsv(&output.join("fingerprints-stale.tsv"))?;
                stale = true;
                blocked = true;
            }
        }
        if blocked {
            fs::write(
                &log_path,
                if stale {
                    "BLOCKED: declared inputs changed; this command was not started.\n"
                } else {
                    "BLOCKED: an earlier required gate failed; this command was not started.\n"
                },
            )?;
            results.push(GateResult::blocked(gate, log_name));
            continue;
        }
        if !force
            && cache
                .passes
                .get(&gate.name)
                .is_some_and(|entry| entry.cache_key == cache_key)
        {
            results.push(GateResult::reused(gate, log_name));
            continue;
        }

        let cwd = gate.working_directory(&workspace)?;
        let gate_started = Instant::now();
        let execution = execute(gate, &cwd, &log_path);
        let duration_ms = elapsed_ms(gate_started.elapsed());
        tool_duration_ms = tool_duration_ms.saturating_add(duration_ms);

        match execution {
            Ok(exit_code) if exit_code == Some(0) => {
                pending_passes.insert(
                    gate.name.clone(),
                    CachedPass {
                        cache_key,
                        command: gate.command.clone(),
                    },
                );
                results.push(GateResult::passed(gate, log_name, exit_code, duration_ms));
            }
            Ok(exit_code) => {
                cache.passes.remove(&gate.name);
                cache.save(&state_path)?;
                results.push(GateResult::failed(gate, log_name, exit_code, duration_ms));
                blocked = true;
            }
            Err(error) => {
                cache.passes.remove(&gate.name);
                cache.save(&state_path)?;
                fs::write(&log_path, format!("Unable to start command: {error}\n"))?;
                results.push(GateResult::failed(gate, log_name, None, duration_ms));
                blocked = true;
            }
        }
        let current = InputSnapshot::collect(&workspace, &spec.inputs)?;
        if current.digest != before.digest {
            current.write_tsv(&output.join("fingerprints-stale.tsv"))?;
            stale = true;
            blocked = true;
        }
    }

    let after = InputSnapshot::collect(&workspace, &spec.inputs)?;
    after.write_tsv(&output.join("fingerprints-after.tsv"))?;
    let status = if stale || before.digest != after.digest {
        RunStatus::Stale
    } else if results
        .iter()
        .any(|result| result.status == GateStatus::Failed)
    {
        RunStatus::Fail
    } else {
        RunStatus::Pass
    };
    if status == RunStatus::Pass && !pending_passes.is_empty() {
        cache.passes.extend(pending_passes);
        cache.save(&state_path)?;
    }
    let cache_hits = results
        .iter()
        .filter(|result| result.status == GateStatus::Reused)
        .count();
    let cache_status = CacheStatus::from_hits(cache_hits, results.len());
    let summary = RunSummary {
        schema_version: STATE_VERSION,
        task_id: spec.task_id,
        status,
        input_digest: before.digest,
        started_unix_ms,
        wall_duration_ms: elapsed_ms(started.elapsed()),
        tool_duration_ms,
        cache_hits,
        cache_status,
        toolchain,
        gates: results,
    };
    write_json(&output.join("run.json"), &summary)?;
    fs::write(output.join("run-summary.md"), summary.markdown())?;
    if summary.status == RunStatus::Pass {
        Ok(summary)
    } else {
        Err(format!(
            "test-gate {} finished with {}",
            summary.task_id,
            summary.status.as_str()
        )
        .into())
    }
}

/// 命令行选项；规范文件是唯一必需参数。
struct Options {
    spec: PathBuf,
    force: bool,
}

impl Options {
    fn parse(arguments: Vec<String>) -> Result<Self, DynError> {
        let mut arguments = arguments.into_iter();
        if arguments.next().as_deref() != Some("run") {
            return Err("Usage: cargo xtask test-gate run --spec PATH [--force]".into());
        }
        let mut spec = None;
        let mut force = false;
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--spec" => {
                    if spec.is_some() {
                        return Err("--spec can only be specified once".into());
                    }
                    spec = Some(PathBuf::from(
                        arguments.next().ok_or("--spec requires a file path")?,
                    ));
                }
                "--force" => force = true,
                _ => return Err(format!("Unknown test-gate option: {argument:?}").into()),
            }
        }
        Ok(Self {
            spec: spec.ok_or("--spec is required")?,
            force,
        })
    }
}

/// 一次最终门禁的声明，不包含任何隐式影响范围判断。
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GateSpec {
    task_id: String,
    inputs: Vec<PathBuf>,
    gates: Vec<Gate>,
}

/// 单项门禁使用 argv 数组直接启动，不经过命令字符串解释。
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Gate {
    name: String,
    #[serde(default = "default_cwd")]
    cwd: PathBuf,
    command: Vec<String>,
}

fn default_cwd() -> PathBuf {
    PathBuf::from(".")
}

impl Gate {
    fn working_directory(&self, workspace: &Path) -> Result<PathBuf, DynError> {
        let relative = checked_relative(&self.cwd, "gate cwd")?;
        let path = workspace.join(relative);
        let canonical = path.canonicalize()?;
        if !canonical.starts_with(workspace) || !canonical.is_dir() {
            return Err(format!(
                "Gate working directory is outside the workspace: {}",
                self.cwd.display()
            )
            .into());
        }
        Ok(canonical)
    }
}

/// 跨调用保存的仅成功缓存；失败不会被当作可复用证据。
#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CacheState {
    schema_version: u32,
    passes: BTreeMap<String, CachedPass>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CachedPass {
    cache_key: String,
    command: Vec<String>,
}

impl CacheState {
    fn load(path: &Path) -> Result<Self, DynError> {
        if !path.exists() {
            return Ok(Self {
                schema_version: STATE_VERSION,
                passes: BTreeMap::new(),
            });
        }
        let state: Self = serde_json::from_slice(&fs::read(path)?)?;
        if state.schema_version != STATE_VERSION {
            return Err(format!(
                "Unsupported test-gate state version: {}",
                state.schema_version
            )
            .into());
        }
        Ok(state)
    }

    fn save(&self, path: &Path) -> Result<(), DynError> {
        write_json(path, self)
    }
}

/// 工具链身份属于缓存键，避免跨 Rust/Cargo 版本复用结果。
#[derive(Serialize)]
struct Toolchain {
    cargo: String,
    rustc: String,
    environment: BTreeMap<String, String>,
}

impl Toolchain {
    fn collect(workspace: &Path) -> Result<Self, DynError> {
        Ok(Self {
            cargo: command_version(workspace, "cargo", &["-V"])?,
            rustc: command_version(workspace, "rustc", &["-Vv"])?,
            environment: CACHE_ENVIRONMENT
                .iter()
                .filter_map(|name| env::var(name).ok().map(|value| ((*name).to_owned(), value)))
                .collect(),
        })
    }
}

/// 指纹条目同时记录不存在的输入，支持删除文件进入证据范围。
#[derive(Serialize)]
struct FingerprintEntry {
    path: String,
    state: &'static str,
    sha256: String,
}

struct InputSnapshot {
    entries: Vec<FingerprintEntry>,
    digest: String,
}

impl InputSnapshot {
    fn collect(workspace: &Path, inputs: &[PathBuf]) -> Result<Self, DynError> {
        let mut entries = Vec::with_capacity(inputs.len());
        let mut unique = BTreeSet::new();
        for input in inputs {
            let relative = checked_relative(input, "input")?;
            let display = slash_path(relative);
            if !unique.insert(display.clone()) {
                return Err(format!("Duplicate test-gate input: {display}").into());
            }
            let path = workspace.join(relative);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(format!("Symlink inputs are not supported: {display}").into());
                }
                Ok(metadata) if metadata.is_file() => entries.push(FingerprintEntry {
                    path: display,
                    state: "FILE",
                    sha256: sha256(&fs::read(path)?),
                }),
                Ok(_) => return Err(format!("Input must be a file: {display}").into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    entries.push(FingerprintEntry {
                        path: display,
                        state: "MISSING",
                        sha256: sha256(b"MISSING"),
                    });
                }
                Err(error) => return Err(error.into()),
            }
        }
        let digest = sha256(&serde_json::to_vec(&entries)?);
        Ok(Self { entries, digest })
    }

    fn write_tsv(&self, path: &Path) -> Result<(), DynError> {
        let mut output = String::from("state\tpath\tsha256\n");
        for entry in &self.entries {
            output.push_str(&format!(
                "{}\t{}\t{}\n",
                entry.state, entry.path, entry.sha256
            ));
        }
        fs::write(path, output)?;
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum RunStatus {
    Pass,
    Fail,
    Stale,
}

impl RunStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Stale => "STALE",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Serialize)]
enum GateStatus {
    #[serde(rename = "PASS")]
    Passed,
    #[serde(rename = "FAIL")]
    Failed,
    #[serde(rename = "REUSED")]
    Reused,
    #[serde(rename = "BLOCKED")]
    Blocked,
}

/// 本轮执行相对于已有成功证据的复用程度。
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum CacheStatus {
    Cold,
    Partial,
    Hot,
}

impl CacheStatus {
    fn from_hits(hits: usize, gates: usize) -> Self {
        if hits == 0 {
            Self::Cold
        } else if hits == gates {
            Self::Hot
        } else {
            Self::Partial
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Cold => "COLD",
            Self::Partial => "PARTIAL",
            Self::Hot => "HOT",
        }
    }
}

impl GateStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "PASS",
            Self::Failed => "FAIL",
            Self::Reused => "REUSED",
            Self::Blocked => "BLOCKED",
        }
    }
}

#[derive(Serialize)]
struct GateResult {
    name: String,
    status: GateStatus,
    command: Vec<String>,
    cwd: String,
    exit_code: Option<i32>,
    duration_ms: u64,
    log: String,
}

impl GateResult {
    fn passed(gate: &Gate, log: String, exit_code: Option<i32>, duration_ms: u64) -> Self {
        Self::new(gate, GateStatus::Passed, log, exit_code, duration_ms)
    }

    fn failed(gate: &Gate, log: String, exit_code: Option<i32>, duration_ms: u64) -> Self {
        Self::new(gate, GateStatus::Failed, log, exit_code, duration_ms)
    }

    fn reused(gate: &Gate, log: String) -> Self {
        Self::new(gate, GateStatus::Reused, log, Some(0), 0)
    }

    fn blocked(gate: &Gate, log: String) -> Self {
        Self::new(gate, GateStatus::Blocked, log, None, 0)
    }

    fn new(
        gate: &Gate,
        status: GateStatus,
        log: String,
        exit_code: Option<i32>,
        duration_ms: u64,
    ) -> Self {
        Self {
            name: gate.name.clone(),
            status,
            command: gate.command.clone(),
            cwd: slash_path(&gate.cwd),
            exit_code,
            duration_ms,
            log,
        }
    }
}

#[derive(Serialize)]
struct RunSummary {
    schema_version: u32,
    task_id: String,
    status: RunStatus,
    input_digest: String,
    started_unix_ms: u128,
    wall_duration_ms: u64,
    tool_duration_ms: u64,
    cache_hits: usize,
    cache_status: CacheStatus,
    toolchain: Toolchain,
    gates: Vec<GateResult>,
}

impl RunSummary {
    fn markdown(&self) -> String {
        let mut output = format!(
            "# test_gate run summary\n\n- Task:`{}`\n- Result:**{}**\n- Total wall time:{:.3}s\n- Tool wall time:{:.3}s\n- Cache status: {} ({} hits)\n- Input fingerprint:`{}`\n\n| Status | Gate | Duration | Exit code | Log |\n| --- | --- | ---: | ---: | --- |\n",
            self.task_id,
            self.status.as_str(),
            self.wall_duration_ms as f64 / 1_000.0,
            self.tool_duration_ms as f64 / 1_000.0,
            self.cache_status.as_str(),
            self.cache_hits,
            self.input_digest,
        );
        for gate in &self.gates {
            output.push_str(&format!(
                "| {} | `{}` | {:.3}s | {} | `{}` |\n",
                gate.status.as_str(),
                gate.name,
                gate.duration_ms as f64 / 1_000.0,
                gate.exit_code
                    .map_or_else(|| "-".to_owned(), |code| code.to_string()),
                gate.log,
            ));
        }
        output
    }
}

fn validate_spec(workspace: &Path, spec_path: &Path, spec: &GateSpec) -> Result<PathBuf, DynError> {
    validate_task_id(&spec.task_id)?;
    if spec.inputs.is_empty() {
        return Err("test-gate spec must declare at least one input".into());
    }
    if spec.gates.is_empty() {
        return Err("test-gate spec must declare at least one gate".into());
    }
    if spec_path.file_name().and_then(|name| name.to_str()) != Some("gate.toml") {
        return Err("test-gate spec file must be named gate.toml".into());
    }
    let output = workspace.join("target/test-gate").join(&spec.task_id);
    let spec_parent = spec_path
        .parent()
        .ok_or("test-gate spec must have a parent directory")?
        .canonicalize()?;
    if spec_parent != output.canonicalize()? {
        return Err(format!(
            "Spec must be located at target/test-gate/{}/gate.toml",
            spec.task_id
        )
        .into());
    }

    let mut names = BTreeSet::new();
    for gate in &spec.gates {
        validate_gate_name(&gate.name)?;
        if !names.insert(&gate.name) {
            return Err(format!("Duplicate gate name: {}", gate.name).into());
        }
        if gate.command.is_empty() || gate.command[0].is_empty() {
            return Err(format!("Gate {:?} must have a command", gate.name).into());
        }
        gate.working_directory(workspace)?;
    }
    Ok(output)
}

fn execute(gate: &Gate, cwd: &Path, log: &Path) -> Result<Option<i32>, DynError> {
    let output = Command::new(&gate.command[0])
        .args(&gate.command[1..])
        .current_dir(cwd)
        .output()?;
    let mut evidence = fs::File::create(log)?;
    evidence.write_all(&output.stdout)?;
    evidence.write_all(&output.stderr)?;
    std::io::stdout().write_all(&output.stdout)?;
    std::io::stderr().write_all(&output.stderr)?;
    Ok(output.status.code())
}

fn gate_cache_key(spec: &[u8], inputs: &str, toolchain: &Toolchain, gate: &Gate) -> String {
    let mut material = Vec::new();
    material.extend_from_slice(CACHE_POLICY_VERSION);
    material.extend_from_slice(spec);
    material.extend_from_slice(inputs.as_bytes());
    material.extend_from_slice(&serde_json::to_vec(toolchain).expect("toolchain is serializable"));
    material.extend_from_slice(&serde_json::to_vec(gate).expect("gate is serializable"));
    sha256(&material)
}

fn command_version(
    workspace: &Path,
    program: &str,
    arguments: &[&str],
) -> Result<String, DynError> {
    let output = Command::new(program)
        .args(arguments)
        .current_dir(workspace)
        .output()?;
    if !output.status.success() {
        return Err(format!("Unable to identify {program} toolchain").into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn checked_relative<'a>(path: &'a Path, description: &str) -> Result<&'a Path, DynError> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(format!(
            "{description} must be workspace-relative: {}",
            path.display()
        )
        .into());
    }
    Ok(path)
}

fn validate_task_id(task_id: &str) -> Result<(), DynError> {
    if task_id.is_empty()
        || task_id == "."
        || task_id == ".."
        || !task_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!("Unsafe test-gate task id: {task_id:?}").into());
    }
    Ok(())
}

fn validate_gate_name(name: &str) -> Result<(), DynError> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!("Unsafe gate name: {name:?}").into());
    }
    Ok(())
}

fn safe_name(name: &str) -> String {
    name.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
                char::from(byte)
            } else {
                '_'
            }
        })
        .collect()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask must be located in the tools directory of the workspace")
        .to_owned()
}

fn absolute_path(workspace: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        workspace.join(path)
    }
}

fn slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn elapsed_ms(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), DynError> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

#[cfg(test)]
mod tests;
