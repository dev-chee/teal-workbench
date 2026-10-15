@edition("2026");

// DHE Showcase 共享证据层：路径、进程、日志和追加式事件报告。
import "std/crypto.tl" as crypto;
import "std/fs.tl" as fs;
import "std/process.tl" as process;
import "std/system.tl" as system;

pub static default_plugin_root = "E:/HybridCLR/hybridclr-unity";
pub static default_libil2cpp_root = "E:/HybridCLR/hybridclr-libil2cpp";
pub static default_baseline_root = "E:/SS/worktrees/baseline/project/nexgen-dragon";
pub static default_hotfix_root = "E:/SS/worktrees/hotfix/project/nexgen-dragon";
pub static default_success_marker = "NPP_SceneObjectsReady";
pub static default_dhe_load_min = 2;

pub class Options {
    pub mode: string,
    pub run_root: string,
    pub plugin_root: string,
    pub libil2cpp_root: string,
    pub baseline_root: string,
    pub hotfix_root: string,
    pub demo_root: string,
    pub player_path: string,
    pub player_log: string,
    pub build_inputs_root: string,
    pub success_marker: string,
    pub dhe_load_min: int,
    pub fatal_patterns: string[],
    pub demo_skip_build: bool,
}

pub class SourceState {
    pub role: string,
    pub path: string,
    pub head: string,
    pub upstream: string,
    pub branch: string,
    pub clean: bool,
    pub plugin_revision: string,
    pub libil2cpp_revision: string,
}

pub class LogCheck {
    pub path: string,
    pub sha256: string,
    pub size: string,
    pub source_length: string,
    pub offset: string,
    pub fatal_count: int,
    pub fatal_hits: string[],
    pub dhe_load_count: int,
    pub success_count: int,
    pub fresh: bool,
}

pub class InvestigationState {
    pub source_ok: bool,
    pub revision_ok: bool,
    pub artifact_ok: bool,
    pub player_ok: bool,
}

pub fn join_path(left: string, right: string) -> string {
    if left.ends_with("/") || left.ends_with("\\") {
        return left + right;
    }
    return left + "/" + right;
}

pub fn file_name(path: string) -> string {
    let parts = path.replace("\\", "/").split("/");
    return parts[-1];
}

pub fn parent_path(path: string) -> string {
    let parts = path.replace("\\", "/").split("/");
    if parts.len() <= 1 {
        return ".";
    }
    parts.remove(parts.len() - 1);
    return parts.join("/");
}

pub fn contains_string(values: string[], candidate: string) -> bool {
    for value in values {
        if value == candidate {
            return true;
        }
    }
    return false;
}

pub fn process_succeeded(result: process::ProcessResult) -> bool {
    match result.state {
        process::ProcessState.exited { code } => {
            return code == 0;
        } _ => {
            return false;
        }
    }
}

pub fn process_exit_code(result: process::ProcessResult) -> int {
    match result.state {
        process::ProcessState.exited { code } => {
            return code;
        } _ => {
            return -1;
        }
    }
}

pub fn process_state(result: process::ProcessResult) -> string {
    match result.state {
        process::ProcessState.running => {
            return "running";
        }
        process::ProcessState.timed_out => {
            return "timed_out";
        }
        process::ProcessState.terminated => {
            return "terminated";
        }
        process::ProcessState.exited { code } => {
            return "exited:" + string(code);
        }
    }
}

pub fn command(program: string, args: string[]) -> process::ProcessSpec {
    let spec = process::command(program, args);
    spec.capture_limit_bytes = 16777216;
    return spec;
}

pub task run_command(program: string, args: string[], cwd: string, timeout_ms: int) -> process::ProcessResult | Error {
    let spec = command(program, args);
    spec.cwd = cwd;
    return process::run(spec, timeout_ms).await ?;
}

pub task spawn_command(program: string, args: string[], cwd: string) -> process::ProcessHandle | Error {
    let spec = command(program, args);
    spec.cwd = cwd;
    spec.stdout = process::StreamTarget.discard;
    spec.stderr = process::StreamTarget.discard;
    return process::spawn(spec).await ?;
}

pub task wait_command(handle: process::ProcessHandle, timeout_ms: int) -> process::ProcessResult | Error {
    return process::wait(handle, timeout_ms).await ?;
}

pub task terminate_command(handle: process::ProcessHandle) -> process::ProcessResult | Error {
    return process::terminate(handle).await ?;
}

pub task process_count(path: string) -> int | Error {
    return process::find_by_executable(path).await ?.len();
}

pub task resolve_executable(program: string) -> process::ResolvedExecutable | Error {
    return process::resolve(program).await ?;
}

pub task git(path: string, args: string[]) -> process::ProcessResult | Error {
    return run_command("git", args, path, 120000).await ?;
}

pub fn require_command(label: string, result: process::ProcessResult) -> string | Error {
    if !process_succeeded(result) {
        return Error::make("example_failure", label + " failed with exit code " + string(process_exit_code(result)) + "\n" + result.stderr.trim(), ErrorOrigin.user, false);
    }
    if result.stdout_truncated || result.stderr_truncated {
        return Error::make("example_failure", label + " exceeded the configured output limit", ErrorOrigin.user, false);
    }
    return result.stdout.trim();
}

pub fn event_escape(value: string) -> string {
    return value.replace("%", "%25").replace("|", "%7C").replace("\r", "%0D").replace("\n", "%0A");
}

pub fn event_field(key: string, value: string) -> string {
    return key + "=" + event_escape(value);
}

pub task append_line(report: string, line: string) -> none | Error {
    fs::append(report, line + "\n").await ?;
}

pub task append_event(report: string, sequence: int, run_id: string, phase: string, kind: string, effect: string, code: string, detail: string) -> none | Error {
    let line = "DHE-EVENT|schema=1|seq=" + string(sequence) + "|run=" + event_escape(run_id) + "|at=" + event_escape(system::utc_now()) + "|phase=" + event_escape(phase) + "|kind=" + event_escape(kind) + "|effect=" + event_escape(effect) + "|code=" + event_escape(code) + "|detail=" + event_escape(detail);
    append_line(report, line).await ?;
}

pub fn fatal_patterns() -> string[] {
    return ["crash!!!", "0xc0000005", "access violation", "typeloadexception", "missingmethodexception", "badimageformatexception", "executionengineexception", "dhe_bad_dhao_data", "method body is null", "baseline mismatch", "methodnotfind", "nullexception", "typeinitializationexception", "filenotfoundexception"];
}

fn count_pattern_lines(text: string, patterns: string[], hits: string[]) -> int {
    let count = 0;
    for line in text.split("\n") {
        let lower = line.lowercase();
        let matched = false;
        for pattern in patterns {
            if lower.contains(pattern.lowercase()) {
                matched = true;
                break;
            }
        }
        if matched {
            count += 1;
            if hits.len() < 12 {
                hits.push(line.trim());
            }
        }
    }
    return count;
}

pub task scan_log(path: string, before_fingerprint: string, success_marker: string, patterns: string[]) -> LogCheck | Error {
    if !(fs::exists(path).await ?) {
        return Error::make("filesystem", "Player.log is missing: " + path, ErrorOrigin. host, false);
    }
    let metadata = fs::metadata(path).await ?;
    let fingerprint = metadata.modified_unix_ms + ":" + metadata.size;
    let tail = fs::read_tail(path, 16777216).await ?;
    let hits: string[] = [];
    let fatal_count = count_pattern_lines(tail.text, patterns, hits);
    let dhe_load_count = count_pattern_lines(tail.text, ["load dhe assembly"], []);
    let success_count = if success_marker == "" { 0 } else { count_pattern_lines(tail.text, [success_marker], []) };
    let hash = crypto::sha256_file(path).await;
    return LogCheck {
        path: path,
        sha256: hash.sha256,
        size: hash.size,
        source_length: tail.source_length,
        offset: tail.offset,
        fatal_count: fatal_count,
        fatal_hits: hits,
        dhe_load_count: dhe_load_count,
        success_count: success_count,
        fresh: before_fingerprint == "" || fingerprint != before_fingerprint,
    };
}

pub task copy_log(source: string, destination: string, before_fingerprint: string) -> LogCheck | Error {
    if !(fs::exists(source).await ?) {
        return Error::make("filesystem", "Player.log is missing: " + source, ErrorOrigin. host, false);
    }
    let copied = fs::copy(source, destination, true).await ?;
    if copied == "" {
        return Error::make("runtime", "Player.log copy returned no byte count", ErrorOrigin.user, false);
    }
    return scan_log(destination, before_fingerprint, "", fatal_patterns()).await ?;
}

pub task canonical_existing(path: string, label: string) -> string | Error {
    if !(fs::exists(path).await ?) {
        return Error::make("example_failure", label + " does not exist: " + path, ErrorOrigin.user, false);
    }
    return fs::canonicalize(path).await ?;
}

pub task require_inside(root: string, path: string, label: string) -> string | Error {
    let canonical_root = canonical_existing(root, "root").await ?;
    let canonical_path = canonical_existing(path, label).await ?;
    let normalized_root = canonical_root.replace("\\", "/");
    let normalized_path = canonical_path.replace("\\", "/");
    let prefix = if normalized_root.ends_with("/") { normalized_root } else { normalized_root + "/" };
    if normalized_path != normalized_root && !normalized_path.starts_with(prefix) {
        return Error::make("example_failure", label + " is outside root: " + canonical_path, ErrorOrigin.user, false);
    }
    return canonical_path;
}

pub task source_state(role: string, path: string) -> SourceState | Error {
    if !(fs::exists(path).await ?) {
        return SourceState { role: role, path: path, head: "missing", upstream: "missing", branch: "missing", clean: false, plugin_revision: "", libil2cpp_revision: "" };
    }
    let head_result = git(path, ["rev-parse", "HEAD"]).await ?;
    let upstream_result = git(path, ["rev-parse", "@{{upstream}}"]).await ?;
    let branch_result = git(path, ["branch", "--show-current"]).await ?;
    let status_result = git(path, ["status", "--porcelain=v1", "--untracked-files=all"]).await ?;
    let head = if process_succeeded(head_result) { head_result.stdout.trim() } else { "git-error" };
    let upstream = if process_succeeded(upstream_result) { upstream_result.stdout.trim() } else { "git-error" };
    let branch = if process_succeeded(branch_result) { branch_result.stdout.trim() } else { "git-error" };
    let clean = process_succeeded(status_result) && status_result.stdout.trim() == "";
    return SourceState { role: role, path: path, head: head, upstream: upstream, branch: branch, clean: clean, plugin_revision: "", libil2cpp_revision: "" };
}

pub fn source_ready(state: SourceState) -> bool {
    return state.clean && state.head != "missing" && state.head != "git-error" && state.head != "" && state.head == state.upstream;
}

pub task read_revision(package_root: string) -> string | Error {
    let path = join_path(package_root, "Packages/com.funplus.hybridclr/Data~/revision.txt");
    if !(fs::exists(path).await ?) {
        return "missing";
    }
    return fs::read(path).await ?.trim();
}

pub task start_report(root: string, run_id: string) -> string | Error {
    fs::create_dir_all(root).await ?;
    let report = join_path(root, "dhe-evidence.md");
    if !(fs::exists(report).await ?) {
        fs::write(report, "# DHE Evidence\n\nSchema: 1\nRun: " + run_id + "\nCollected: " + system::utc_now() + "\n").await ?;
    }
    return report;
}
