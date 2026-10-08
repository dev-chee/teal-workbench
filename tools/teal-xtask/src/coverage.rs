//! `teal-abi` 与 `teal-vm` 生产代码覆盖率门禁。

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Command,
};

use serde::Deserialize;

/// CI 与本地验证共同要求的最低行/区域覆盖率。
const ABI_MINIMUM_COVERAGE: &str = "95";
const VM_MINIMUM_COVERAGE: &str = "60";
const VM_CRITICAL_MINIMUM_LINES: &str = "80";
const HOST_MINIMUM_COVERAGE: &str = "80";
const HOST_CRITICAL_MINIMUM_LINES: &str = "90";

/// 状态机、外部输入验证与调试目标必须维持更高的行覆盖率。
const VM_CRITICAL_MODULES: &[(&str, &str)] = &[
    ("state machine", "crates/teal-vm/src/execution.rs"),
    ("boundary", "crates/teal-vm/src/boundary.rs"),
    ("snapshot facade", "crates/teal-vm/src/snapshot.rs"),
    ("snapshot capture", "crates/teal-vm/src/snapshot/capture.rs"),
    ("snapshot codec", "crates/teal-vm/src/snapshot/codec.rs"),
    ("snapshot context", "crates/teal-vm/src/snapshot/context.rs"),
    ("snapshot restore", "crates/teal-vm/src/snapshot/restore.rs"),
    (
        "snapshot validation",
        "crates/teal-vm/src/snapshot/validation.rs",
    ),
    ("debug target", "crates/teal-vm/src/debug.rs"),
    ("debug evaluation", "crates/teal-vm/src/debug/evaluate.rs"),
    ("debug session", "crates/teal-vm/src/debug/session.rs"),
    ("external dispatch", "crates/teal-vm/src/external.rs"),
];

/// 一个覆盖率目标的判据：统计范围、全局阈值与关键模块阈值。
///
/// 阈值只有这一处来源，采集与判定都从这里读取，避免多处常量漂移。
#[derive(Clone, Copy)]
struct TargetCriteria {
    /// 打印与报告使用的包名。
    package: &'static str,
    /// 工作区内参与统计的源码前缀。
    source_root: &'static str,
    /// 全局行覆盖率下限（百分比）。
    minimum: &'static str,
    /// 关键模块清单：名称与工作区相对路径。
    critical_modules: &'static [(&'static str, &'static str)],
    /// 关键模块行覆盖率下限（百分比）。
    critical_minimum: &'static str,
}

/// 返回 VM 目标的判据。
const fn vm_criteria() -> TargetCriteria {
    TargetCriteria {
        package: "teal-vm",
        source_root: "crates/teal-vm/src",
        minimum: VM_MINIMUM_COVERAGE,
        critical_modules: VM_CRITICAL_MODULES,
        critical_minimum: VM_CRITICAL_MINIMUM_LINES,
    }
}

/// 返回 Host 目标的判据。
const fn host_criteria() -> TargetCriteria {
    TargetCriteria {
        package: "teal-host",
        source_root: "crates/teal-host/src",
        minimum: HOST_MINIMUM_COVERAGE,
        critical_modules: HOST_CRITICAL_MODULES,
        critical_minimum: HOST_CRITICAL_MINIMUM_LINES,
    }
}

const HOST_CRITICAL_MODULES: &[(&str, &str)] = &[
    ("authorization", "crates/teal-host/src/authorization.rs"),
    ("dylib", "crates/teal-host/src/ffi.rs"),
    ("dylib policy", "crates/teal-host/src/ffi/policy.rs"),
    (
        "native filesystem",
        "crates/teal-host/src/native/file_system.rs",
    ),
    ("files", "crates/teal-host/src/standard/files.rs"),
    ("process", "crates/teal-host/src/standard/process.rs"),
    (
        "process spec",
        "crates/teal-host/src/standard/process/spec.rs",
    ),
];

/// 排除独立测试源码，只统计 `teal-abi` 生产实现。
const TEST_SOURCE_REGEX: &str = r"[/\\]tests([/\\]|\.rs$)";

/// 执行 ABI 或 VM 覆盖率门禁。
pub fn run(arguments: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    ensure_llvm_cov(&cargo)?;
    match arguments.as_slice() {
        [target] if target == "abi" => {
            run_abi_coverage(&cargo)?;
        }
        [target] if target == "vm" => run_vm_coverage(&cargo)?,
        [target] if target == "host" => run_host_coverage(&cargo)?,
        [target] if target == "all" => run_all_coverage(&cargo)?,
        _ => {
            return Err("coverage accepts exactly one target: abi, vm, host, or all".into());
        }
    }
    Ok(())
}

/// 运行 ABI 门禁：直接使用 `cargo llvm-cov` 的阈值选项，只统计生产源码。
fn run_abi_coverage(cargo: &OsStr) -> Result<(), Box<dyn std::error::Error>> {
    clean_coverage(cargo)?;
    let status = Command::new(cargo)
        .current_dir(workspace_root())
        .args(abi_arguments())
        .status()?;
    if !status.success() {
        return Err(format!(
            "teal-abi coverage command failed: tests failed, or production line/region coverage did not reach {ABI_MINIMUM_COVERAGE}%"
        )
        .into());
    }
    Ok(())
}

/// 返回传给 `cargo llvm-cov` 的稳定参数，供命令与回归测试共用。
fn abi_arguments() -> [&'static str; 12] {
    [
        "llvm-cov",
        "--package",
        "teal-abi",
        "--lib",
        "--summary-only",
        "--ignore-filename-regex",
        TEST_SOURCE_REGEX,
        "--fail-under-lines",
        ABI_MINIMUM_COVERAGE,
        "--fail-under-regions",
        ABI_MINIMUM_COVERAGE,
        "--locked",
    ]
}

fn vm_arguments() -> [&'static str; 4] {
    ["llvm-cov", "--all-targets", "--json", "--output-path"]
}

const VM_COVERAGE_PACKAGES: &[&str] = &[
    "teal",
    "teal-abi",
    "teal-vm",
    "teal-compiler",
    "teal-core",
    "teal-host",
    "teal-agent",
    "teal-dap",
];

#[derive(Deserialize)]
struct CoverageExport {
    data: Vec<CoverageData>,
}

#[derive(Deserialize)]
struct CoverageData {
    files: Vec<CoverageFile>,
}

#[derive(Deserialize)]
struct CoverageFile {
    filename: PathBuf,
    summary: CoverageSummary,
}

#[derive(Deserialize)]
struct CoverageSummary {
    lines: CoverageCount,
    regions: CoverageCount,
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
struct CoverageCount {
    count: u64,
    covered: u64,
}

/// 一次判定的结果，供打印与结构化报告共用。
#[derive(Debug, serde::Serialize)]
struct TargetSummary {
    package: &'static str,
    minimum: &'static str,
    critical_minimum: &'static str,
    lines: CoverageCount,
    regions: CoverageCount,
    critical: Vec<CriticalSummary>,
}

/// 关键模块的覆盖结果。
#[derive(Debug, serde::Serialize)]
struct CriticalSummary {
    name: &'static str,
    lines: CoverageCount,
    regions: CoverageCount,
}

/// 运行单个工作区目标的覆盖率门禁。
fn run_vm_coverage(cargo: &OsStr) -> Result<(), Box<dyn std::error::Error>> {
    run_workspace_target(cargo, "vm", vm_criteria())
}

/// 运行 Host 目标的覆盖率门禁。
fn run_host_coverage(cargo: &OsStr) -> Result<(), Box<dyn std::error::Error>> {
    run_workspace_target(cargo, "host", host_criteria())
}

/// 一次工作区采集，同时判定 VM 与 Host 两个目标。
///
/// 两个目标共用同一份 `--all-targets` 覆盖数据；任一失败都会先写出报告，再返回合并后的
/// 失败原因。
fn run_all_coverage(cargo: &OsStr) -> Result<(), Box<dyn std::error::Error>> {
    let criteria = [vm_criteria(), host_criteria()];
    preflight_modules(&criteria)?;
    let root = workspace_root();
    let export = collect_workspace_coverage(cargo, &root, "all")?;
    let mut summaries = Vec::new();
    let mut failures = Vec::new();
    for target in criteria {
        let summary = evaluate_target(&root, &export, target);
        if let Err(reason) = enforce_summary(&summary) {
            failures.push(reason);
        }
        summaries.push(summary);
    }
    write_combined_summary(&root, "all", &summaries)?;
    if failures.is_empty() {
        return Ok(());
    }
    Err(format!("coverage all failed:\n{}", failures.join("\n")).into())
}

/// 采集一次工作区覆盖数据并判定单个目标。
fn run_workspace_target(
    cargo: &OsStr,
    target: &str,
    criteria: TargetCriteria,
) -> Result<(), Box<dyn std::error::Error>> {
    preflight_modules(&[criteria])?;
    let root = workspace_root();
    let export = collect_workspace_coverage(cargo, &root, target)?;
    let summary = evaluate_target(&root, &export, criteria);
    write_combined_summary(&root, target, std::slice::from_ref(&summary))?;
    enforce_summary(&summary).map_err(Into::into)
}

/// 按判据汇总一份覆盖数据。
fn evaluate_target(
    root: &Path,
    export: &CoverageExport,
    criteria: TargetCriteria,
) -> TargetSummary {
    let mut lines = CoverageCount {
        count: 0,
        covered: 0,
    };
    let mut regions = CoverageCount {
        count: 0,
        covered: 0,
    };
    let mut critical = criteria
        .critical_modules
        .iter()
        .map(|(name, path)| {
            (
                *name,
                Path::new(path),
                CoverageCount {
                    count: 0,
                    covered: 0,
                },
                CoverageCount {
                    count: 0,
                    covered: 0,
                },
            )
        })
        .collect::<Vec<_>>();
    collect_crate_coverage(
        root,
        export,
        Path::new(criteria.source_root),
        &mut lines,
        &mut regions,
        &mut critical,
    );
    TargetSummary {
        package: criteria.package,
        minimum: criteria.minimum,
        critical_minimum: criteria.critical_minimum,
        lines,
        regions,
        critical: critical
            .into_iter()
            .map(|(name, _, lines, regions)| CriticalSummary {
                name,
                lines,
                regions,
            })
            .collect(),
    }
}

/// 采集整份工作区覆盖数据到目标目录并解析为导出结构。
fn collect_workspace_coverage(
    cargo: &OsStr,
    root: &Path,
    target: &str,
) -> Result<CoverageExport, Box<dyn std::error::Error>> {
    clean_coverage(cargo)?;
    let output = coverage_report_path(root, target).with_extension("export.json");
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut command = Command::new(cargo);
    command.current_dir(root).args(vm_arguments()).arg(&output);
    for package in VM_COVERAGE_PACKAGES {
        command.args(["--package", package]);
    }
    let status = command.arg("--locked").status()?;
    if !status.success() {
        return Err("Workspace tests or coverage data generation failed".into());
    }

    let export: CoverageExport = serde_json::from_slice(&std::fs::read(&output)?)?;
    Ok(export)
}

/// 返回门禁报告路径：`target/llvm-cov/<target>.json`。
fn coverage_report_path(root: &Path, target: &str) -> PathBuf {
    root.join("target")
        .join("llvm-cov")
        .join(format!("{target}.json"))
}

/// 在采集任何覆盖数据之前校验关键模块路径存在。
///
/// 关键模块改名或移动后，覆盖率会静默变成 0%，只靠阈值不足很难定位；这里在启动 cargo
/// 之前直接报出缺失路径。
fn preflight_modules(criteria: &[TargetCriteria]) -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    for target in criteria {
        for (name, path) in target.critical_modules {
            if !root.join(path).is_file() {
                return Err(format!(
                    "missing critical coverage module for {}: {name} ({path})",
                    target.package
                )
                .into());
            }
        }
    }
    Ok(())
}

/// 写出结构化判定摘要；失败时仍写，便于对照。
fn write_combined_summary(
    root: &Path,
    target: &str,
    summaries: &[TargetSummary],
) -> Result<(), Box<dyn std::error::Error>> {
    let path = coverage_report_path(root, target);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let document = serde_json::json!({
        "target": target,
        "results": summaries,
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&document)?)?;
    Ok(())
}

fn clean_coverage(cargo: &OsStr) -> Result<(), Box<dyn std::error::Error>> {
    let clean = Command::new(cargo)
        .current_dir(workspace_root())
        .args(["llvm-cov", "clean", "--workspace"])
        .status()?;
    if !clean.success() {
        return Err("Failed to clean old coverage artifacts".into());
    }
    Ok(())
}

fn collect_crate_coverage<'a>(
    root: &Path,
    export: &CoverageExport,
    source_root: &Path,
    lines: &mut CoverageCount,
    regions: &mut CoverageCount,
    critical: &mut [(&'a str, &'a Path, CoverageCount, CoverageCount)],
) {
    for file in export.data.iter().flat_map(|data| data.files.iter()) {
        let Ok(relative) = file.filename.strip_prefix(root) else {
            continue;
        };
        if !relative.starts_with(source_root) || is_test_source(relative) {
            continue;
        }
        if let Some((_, _, critical_lines, critical_regions)) = critical
            .iter_mut()
            .find(|(_, path, _, _)| relative == *path)
        {
            critical_lines.count += file.summary.lines.count;
            critical_lines.covered += file.summary.lines.covered;
            critical_regions.count += file.summary.regions.count;
            critical_regions.covered += file.summary.regions.covered;
        }
        lines.count += file.summary.lines.count;
        lines.covered += file.summary.lines.covered;
        regions.count += file.summary.regions.count;
        regions.covered += file.summary.regions.covered;
    }
}

/// 返回该路径是否是测试源码：路径含 `tests` 目录组件，或文件名为 `tests.rs`。
///
/// 只按整路径组件判断，避免把 `latest.rs` 这类普通文件名误当成测试源码。
fn is_test_source(relative: &Path) -> bool {
    relative
        .components()
        .any(|part| part.as_os_str() == "tests")
        || relative.file_name().is_some_and(|name| name == "tests.rs")
}

/// 按判据检查一次汇总结果，打印各项百分比并返回失败原因。
fn enforce_summary(summary: &TargetSummary) -> Result<(), String> {
    let package = summary.package;
    let line_percent = percent(&summary.lines);
    let region_percent = percent(&summary.regions);
    println!(
        "{package} production code coverage: lines {line_percent:.2}%, regions {region_percent:.2}%"
    );
    let minimum: f64 = summary
        .minimum
        .parse()
        .map_err(|_| format!("{package}: invalid coverage threshold"))?;
    if line_percent < minimum || region_percent < minimum {
        return Err(format!(
            "{package} production line and region coverage must both reach {}%",
            summary.minimum
        ));
    }
    let critical_minimum: f64 = summary
        .critical_minimum
        .parse()
        .map_err(|_| format!("{package}: invalid critical coverage threshold"))?;
    for module in &summary.critical {
        let module_line_percent = percent(&module.lines);
        let module_region_percent = percent(&module.regions);
        println!(
            "{package} {} coverage: lines {module_line_percent:.2}%, regions {module_region_percent:.2}%",
            module.name
        );
        if module_line_percent < critical_minimum {
            return Err(format!(
                "{package} {} line coverage must reach {}%",
                module.name, summary.critical_minimum
            ));
        }
    }
    Ok(())
}

fn percent(count: &CoverageCount) -> f64 {
    if count.count == 0 {
        0.0
    } else {
        count.covered as f64 * 100.0 / count.count as f64
    }
}

/// 确认 Cargo 子命令可用，并给出可复现的固定版本安装方式。
fn ensure_llvm_cov(cargo: &OsStr) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(cargo)
        .args(["llvm-cov", "--version"])
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(
            "cargo-llvm-cov 0.9.0 is missing; please run `cargo install cargo-llvm-cov --version 0.9.0 --locked`"
                .into(),
        )
    }
}

/// 返回工作区根目录，不依赖调用命令时的当前目录。
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask must be located in the tools directory of the workspace")
        .to_owned()
}

#[cfg(test)]
mod tests;
