//! Teal 官方知识制品的生成、确定性与发行边界检查。
//!
//! 权威来源是语言手册、工作台手册、内置扩展包接口、Builtin Interface 和验证示例；官方知识
//! 没有提交入库的制品，`teal-knowledge` 在构建期生成内嵌二进制。本命令用同一份生成器源码
//! 复现该制品，供人工检查与确定性验收。

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::generator;

/// 组件间共享的构建制品文件与目录，任何形式都不允许重新提交入库。
const FORBIDDEN_ARTIFACTS: [&str; 3] = [
    "crates/teal-knowledge/src/generated",
    "crates/teal-knowledge/src/generated/knowledge.bin",
    "runtime/knowledge",
];

/// 执行 `cargo xtask knowledge generate [--output PATH]|verify`。
pub(crate) fn run(command: &str, arguments: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    match command {
        "generate" => {
            let output = parse_output(arguments, &root)?;
            let bytes = build_bytes(&root)?;
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&output, &bytes)?;
            println!(
                "Teal knowledge artifact: {} ({} bytes)",
                output.display(),
                bytes.len()
            );
            Ok(())
        }
        "verify" => {
            if !arguments.is_empty() {
                return Err("knowledge verify does not accept arguments".into());
            }
            verify(&root)
        }
        _ => Err("Usage: cargo xtask knowledge <generate [--output PATH]|verify>".into()),
    }
}

/// 从当前权威来源生成制品字节。
pub(crate) fn build_bytes(root: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let artifact = generator::build_artifact(root, env!("CARGO_PKG_VERSION"))?;
    generator::encode_artifact(&artifact)
}

/// 解析 `generate` 的可选输出路径。
fn parse_output(
    arguments: Vec<String>,
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut output = root.join("target/knowledge/knowledge.bin");
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--output" => {
                let value = arguments.next().ok_or("--output requires a path")?;
                output = PathBuf::from(value);
            }
            other => return Err(format!("unknown knowledge generate argument `{other}`").into()),
        }
    }
    Ok(output)
}

/// 校验来源可复现同一制品，并确认仓库没有重新出现提交入库的制品。
fn verify(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    reject_committed_artifacts(root)?;
    let first = build_bytes(root)?;
    let second = build_bytes(root)?;
    if first != second {
        return Err("knowledge artifact generation is not deterministic".into());
    }
    let catalog = teal_knowledge::KnowledgeCatalog::from_bytes(&first)
        .map_err(|error| format!("generated knowledge artifact is invalid: {error}"))?;
    if first != teal_knowledge::KnowledgeCatalog::embedded_bytes() {
        return Err(
            "the embedded knowledge artifact was built from different sources; rebuild the crate"
                .into(),
        );
    }
    println!(
        "Teal knowledge v{} sources {} content sha256 {} is reproducible",
        catalog.toolchain_version(),
        catalog.snapshot().sources.len(),
        catalog.content_digest()
    );
    Ok(())
}

/// 拒绝仓库内重新出现已提交的知识制品。
fn reject_committed_artifacts(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for relative in FORBIDDEN_ARTIFACTS {
        let path = root.join(relative);
        if path.exists() {
            return Err(format!(
                "official knowledge must not exist in the repository: {}",
                path.display()
            )
            .into());
        }
    }
    if !root.join(".git").exists() {
        return Ok(());
    }
    let output = match Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["ls-files", "--"])
        .args(FORBIDDEN_ARTIFACTS)
        .output()
    {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            return Err(format!(
                "cannot inspect tracked knowledge artifacts: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        Err(error) => return Err(format!("cannot run git: {error}").into()),
    };
    let tracked = String::from_utf8(output.stdout)?;
    if !tracked.trim().is_empty() {
        return Err(format!(
            "official knowledge artifacts are tracked in git:\n{}",
            tracked.trim()
        )
        .into());
    }
    Ok(())
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask must be located in the tools directory of the workspace")
        .to_owned()
}
