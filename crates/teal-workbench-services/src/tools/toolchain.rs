//! 随分发工具的可执行文件解析与参数适配。

use super::*;

/// Task 启动前必须存在的同版本 Teal 工具链。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskToolchain {
    /// 独立编译检查命令。
    pub teal_c: PathBuf,
    /// 独立运行命令。
    pub teal_run: PathBuf,
    /// 独立测试命令。
    pub teal_test: PathBuf,
    /// 与当前工作台可执行文件位于同一目录的格式化命令。
    pub teal_fmt: PathBuf,
}

/// Task 启动前工具链预检失败的脱敏诊断。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolchainPreflightError {
    /// 当前进程所在目录；只显示目录，不包含环境变量或凭证。
    pub directory: Option<PathBuf>,
    /// 缺失或不是普通文件的工具名。
    pub missing: Vec<String>,
}

impl std::fmt::Display for ToolchainPreflightError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(directory) = &self.directory {
            write!(
                formatter,
                "Teal task toolchain is unavailable in {}",
                directory.display()
            )?;
        } else {
            formatter.write_str("Teal task toolchain location is unavailable")?;
        }
        if !self.missing.is_empty() {
            write!(formatter, "; missing: {}", self.missing.join(", "))?;
        }
        formatter.write_str(
            "; build with `cargo build -p teal --bin teal-c --bin teal-run --bin teal-test --bin teal-fmt`",
        )
    }
}

/// 验证 Task 所需的 sibling binaries，避免模型启动后才发现本地工具缺失。
pub fn preflight_task_toolchain() -> Result<TaskToolchain, ToolchainPreflightError> {
    let directory = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let Some(directory) = directory.clone() else {
        return Err(ToolchainPreflightError {
            directory: None,
            missing: vec![
                "teal-c".into(),
                "teal-run".into(),
                "teal-test".into(),
                "teal-fmt".into(),
            ],
        });
    };

    let required = ["teal-c", "teal-run", "teal-test", "teal-fmt"];
    let mut missing = Vec::new();
    let mut paths = BTreeMap::new();
    for stem in required {
        let candidate = executable_in(&directory, stem);
        if candidate.is_file() {
            paths.insert(stem, candidate);
        } else {
            missing.push(format!("{} ({})", stem, candidate.display()));
        }
    }
    if !missing.is_empty() {
        return Err(ToolchainPreflightError {
            directory: Some(directory),
            missing,
        });
    }

    Ok(TaskToolchain {
        teal_c: paths
            .remove("teal-c")
            .expect("preflight inserted the teal-c executable"),
        teal_run: paths
            .remove("teal-run")
            .expect("preflight inserted the teal-run executable"),
        teal_test: paths
            .remove("teal-test")
            .expect("preflight inserted the teal-test executable"),
        teal_fmt: paths
            .remove("teal-fmt")
            .expect("preflight inserted the teal-fmt executable"),
    })
}

fn executable_in(directory: &Path, stem: &str) -> PathBuf {
    let name = if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_owned()
    };
    directory.join(name)
}

pub(super) fn teal_executable() -> Option<PathBuf> {
    sibling_executable("teal")
}

pub(super) fn teal_format_executable() -> Option<PathBuf> {
    sibling_executable("teal-fmt")
}

pub(super) fn teal_tool_executable(name: &str) -> Option<PathBuf> {
    sibling_executable(teal_tool_stem(name))
}

pub(super) fn teal_tool_stem(name: &str) -> &'static str {
    match name {
        "teal.check" => "teal-c",
        "teal.fmt_check" => "teal-fmt",
        "teal.test" => "teal-test",
        _ => "teal-run",
    }
}

pub(super) fn teal_tool_arguments(name: &str, path: &Path) -> Vec<String> {
    let path = path.to_string_lossy().into_owned();
    match name {
        "teal.fmt_check" => vec!["--check".to_owned(), path],
        _ => vec![path],
    }
}

fn sibling_executable(stem: &str) -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    let directory = current.parent()?;
    let candidate = executable_in(directory, stem);
    candidate.is_file().then_some(candidate)
}
