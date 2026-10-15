//! 源码 Agent 交付收据和工作区程序摘要工具。

use std::{
    io::Read,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 交付时固定的一项工作区文件事实。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryArtifact {
    /// 规范化的工作区相对路径。
    pub path: String,
    /// 普通文件字节数。
    pub bytes: u64,
    /// 文件内容 SHA-256。
    pub sha256: String,
}

/// 模型提交且已绑定真实 ToolResult 的一项验证说明。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryVerification {
    /// 当前任务历史中的工具调用身份。
    pub call_id: String,
    /// 模型对该证据用途的简短说明。
    pub claim: String,
}

/// 已通过应用级验证的任务交付收据。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    /// 模型提交并由宿主接受的摘要。
    pub summary: String,
    /// 主入口文件事实。
    pub script: DeliveryArtifact,
    /// 其余交付产物。
    pub artifacts: Vec<DeliveryArtifact>,
    /// `teal.check` 与 `teal.run` 共同确认的程序摘要。
    pub program_digest: String,
    /// 生成程序摘要时使用的运行配置档。
    pub tier: String,
    /// 被交付结果引用的工具调用。
    pub evidence: Vec<DeliveryVerification>,
    /// 验收时 Unix 毫秒时间戳。
    pub accepted_at_ms: i64,
}

/// 返回已验收收据是否仍与当前工作区文件和编译程序一致。
#[must_use]
pub fn delivery_is_current(workspace: &Path, receipt: &DeliveryReceipt) -> bool {
    let Ok(workspace) = workspace.canonicalize() else {
        return false;
    };
    let files_match = std::iter::once(&receipt.script)
        .chain(&receipt.artifacts)
        .all(|expected| {
            resolve_file(&workspace, &expected.path)
                .and_then(|path| artifact(&workspace, &path))
                .is_ok_and(|actual| actual == *expected)
        });
    if !files_match {
        return false;
    }
    let script = workspace.join(Path::new(&receipt.script.path));
    compile_digest_in_workspace(&workspace, &script, &receipt.tier)
        .is_ok_and(|digest| digest == receipt.program_digest)
}

fn resolve_file(workspace: &Path, input: &str) -> Result<PathBuf, String> {
    if input.trim().is_empty() || input.chars().any(char::is_control) {
        return Err("Task delivery path is invalid".into());
    }
    let input = Path::new(input);
    let path = if input.is_absolute() {
        input.to_path_buf()
    } else {
        workspace.join(input)
    };
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    if !path.starts_with(workspace) || !path.is_file() {
        return Err("Task delivery file is outside the workspace or missing".into());
    }
    Ok(path)
}

fn artifact(workspace: &Path, path: &Path) -> Result<DeliveryArtifact, String> {
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or_else(|| "Task delivery file is too large".to_owned())?;
        hasher.update(&chunk[..read]);
    }
    let relative = path
        .strip_prefix(workspace)
        .map_err(|_| "Task artifact is outside the workspace")?;
    Ok(DeliveryArtifact {
        path: relative.to_string_lossy().replace('\\', "/"),
        bytes,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

/// 在指定工作区边界内编译入口，拒绝用户源码导入工作区外的文件。
pub fn compile_digest_in_workspace(
    workspace: &Path,
    path: &Path,
    tier: &str,
) -> Result<String, String> {
    let tier = teal::RuntimeTier::from_name(tier)
        .ok_or_else(|| format!("Unknown runtime tier `{tier}`"))?;
    let workspace = workspace
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    if !path.starts_with(&workspace) {
        return Err("Task program is outside the workspace".into());
    }
    let source = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let source_name = stable_source_name(&path);
    let loader = crate::tools::SourceQuerySnapshot::new(&workspace, [])
        .map_err(|error| format!("Unable to prepare workspace source loader: {error}"))?;
    let program = teal::compile_source(
        teal::Source::new(source_name, source),
        &loader,
        teal::CompileOptions::default().with_tier(tier),
    )
    .map_err(|errors| format!("Task program no longer compiles: {errors:?}"))?;
    let bytes = program.serialize().map_err(|error| error.to_string())?;
    Ok(digest(&bytes))
}

/// 在指定工作区边界内读取入口程序登记的测试名称。
pub fn compile_test_names_in_workspace(
    workspace: &Path,
    path: &Path,
    tier: &str,
) -> Result<Vec<String>, String> {
    let tier = teal::RuntimeTier::from_name(tier)
        .ok_or_else(|| format!("Unknown runtime tier `{tier}`"))?;
    let workspace = workspace
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    if !path.starts_with(&workspace) {
        return Err("Task program is outside the workspace".into());
    }
    let source = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let source_name = stable_source_name(&path);
    let loader = crate::tools::SourceQuerySnapshot::new(&workspace, [])
        .map_err(|error| format!("Unable to prepare workspace source loader: {error}"))?;
    let program = teal::compile_source(
        teal::Source::new(source_name, source),
        &loader,
        teal::CompileOptions::default().with_tier(tier),
    )
    .map_err(|errors| format!("Task program no longer compiles: {errors:?}"))?;
    Ok(program
        .tests()
        .iter()
        .map(|test| test.name.to_string())
        .collect())
}

fn stable_source_name(path: &Path) -> String {
    let path = path.to_string_lossy();
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        path.into_owned()
    }
}

/// 返回字节内容的十六进制 SHA-256 摘要。
#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    teal_utils::sha256_hex(bytes)
}

/// 返回当前 Unix 毫秒时间戳；系统时钟早于 epoch 时返回零。
#[must_use]
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, source: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, source).unwrap();
    }

    #[test]
    fn workspace_loader_accepts_a_sibling_inside_root_and_rejects_escape() {
        let parent = tempfile::tempdir().unwrap();
        let root_path = parent.path().join("workspace");
        let outside_path = parent.path().join("outside/helper.tl");
        std::fs::create_dir_all(&root_path).unwrap();
        write(
            &root_path.join("shared/helper.tl"),
            "pub fn value() -> int { return 7; }\n",
        );
        write(
            &root_path.join("src/main.tl"),
            "import \"../shared/helper.tl\" as helper;\nfn main() { print(helper::value()); }\n",
        );
        assert!(
            compile_digest_in_workspace(&root_path, &root_path.join("src/main.tl"), "agentic")
                .is_ok()
        );

        write(&outside_path, "pub fn value() -> int { return 9; }\n");
        write(
            &root_path.join("src/escape.tl"),
            "import \"../../outside/helper.tl\" as helper;\nfn main() { print(helper::value()); }\n",
        );
        let error =
            compile_digest_in_workspace(&root_path, &root_path.join("src/escape.tl"), "agentic")
                .unwrap_err();
        assert!(error.contains("outside the workspace"), "{error}");
    }

    #[test]
    fn workspace_loader_keeps_first_party_package_imports_available() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.tl");
        write(&path, "import \"std/json.tl\" as json;\nfn main() {}\n");
        assert!(compile_digest_in_workspace(root.path(), &path, "agentic").is_ok());
    }
}
