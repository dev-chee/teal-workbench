//! 交付核验使用的当前工作区程序摘要。

use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

/// 将当前程序重新编译的请求限制在一个已确定的工作区内。
pub struct WorkspaceProgramDigest {
    workspace: PathBuf,
}

impl WorkspaceProgramDigest {
    /// 绑定已存在的工作区；路径解析和内容读取在每次核验时重新进行。
    ///
    /// # Errors
    /// 工作区不存在或无法规范化时返回错误。
    pub fn new(workspace: &Path) -> Result<Self, String> {
        Ok(Self {
            workspace: workspace
                .canonicalize()
                .map_err(|error| error.to_string())?,
        })
    }
}

impl teal_agent::ProgramDigestProvider for WorkspaceProgramDigest {
    fn digest(
        &self,
        path: String,
        profile: String,
    ) -> teal_agent::ProviderFuture<Result<String, teal_agent::AgentError>> {
        let workspace = self.workspace.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let path = workspace
                    .join(path)
                    .canonicalize()
                    .map_err(|error| teal_agent::AgentError::Protocol(error.to_string()))?;
                if !path.starts_with(&workspace) || !path.is_file() {
                    return Err(teal_agent::AgentError::Protocol(
                        "program path is outside the workspace or is not a file".into(),
                    ));
                }
                crate::compile_digest_in_workspace(&workspace, &path, &profile)
                    .map_err(teal_agent::AgentError::Protocol)
            })
            .await
            .map_err(|error| teal_agent::AgentError::Protocol(error.to_string()))?
        })
    }

    fn test_count(
        &self,
        path: String,
        profile: String,
    ) -> teal_agent::ProviderFuture<Result<usize, teal_agent::AgentError>> {
        let workspace = self.workspace.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let path = workspace
                    .join(path)
                    .canonicalize()
                    .map_err(|error| teal_agent::AgentError::Protocol(error.to_string()))?;
                if !path.starts_with(&workspace) || !path.is_file() {
                    return Err(teal_agent::AgentError::Protocol(
                        "program path is outside the workspace or is not a file".into(),
                    ));
                }
                crate::compile_test_names_in_workspace(&workspace, &path, &profile)
                    .map(|tests| tests.len())
                    .map_err(teal_agent::AgentError::Protocol)
            })
            .await
            .map_err(|error| teal_agent::AgentError::Protocol(error.to_string()))?
        })
    }
}

impl teal_agent::FileEvidenceProvider for WorkspaceProgramDigest {
    fn evidence(
        &self,
        path: String,
    ) -> teal_agent::ProviderFuture<Result<teal_agent::FileEvidence, teal_agent::AgentError>> {
        let workspace = self.workspace.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let path = workspace
                    .join(path)
                    .canonicalize()
                    .map_err(|error| teal_agent::AgentError::Protocol(error.to_string()))?;
                if !path.starts_with(&workspace) || !path.is_file() {
                    return Err(teal_agent::AgentError::Protocol(
                        "evidence path is outside the workspace or is not a file".into(),
                    ));
                }
                let mut file = std::fs::File::open(&path)
                    .map_err(|error| teal_agent::AgentError::Protocol(error.to_string()))?;
                let mut digest = Sha256::new();
                let mut bytes = 0_u64;
                let mut chunk = [0_u8; 32 * 1024];
                loop {
                    let read = file
                        .read(&mut chunk)
                        .map_err(|error| teal_agent::AgentError::Protocol(error.to_string()))?;
                    if read == 0 {
                        break;
                    }
                    bytes = bytes.saturating_add(read as u64);
                    digest.update(&chunk[..read]);
                }
                Ok(teal_agent::FileEvidence {
                    path: path.to_string_lossy().into_owned(),
                    sha256: format!("{:x}", digest.finalize()),
                    bytes,
                })
            })
            .await
            .map_err(|error| teal_agent::AgentError::Protocol(error.to_string()))?
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use teal_agent::{FileEvidenceProvider, ProgramDigestProvider};

    #[tokio::test]
    async fn file_evidence_is_scoped_to_the_workspace_and_rechecks_content() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = root.path().join("file.txt");
        std::fs::write(&path, b"before").unwrap();
        let provider = WorkspaceProgramDigest::new(root.path()).unwrap();
        let before = provider.evidence("file.txt".into()).await.unwrap();
        assert_eq!(before.bytes, 6);
        std::fs::write(&path, b"after").unwrap();
        let after = provider.evidence("file.txt".into()).await.unwrap();
        assert_ne!(before.sha256, after.sha256);
        let outsider = outside.path().join("outside.txt");
        std::fs::write(&outsider, b"secret").unwrap();
        assert!(
            provider
                .evidence(outsider.to_string_lossy().into())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn current_program_rejects_a_path_outside_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let path = other.path().join("other.tl");
        std::fs::write(&path, "task main() {}").unwrap();
        let provider = WorkspaceProgramDigest::new(root.path()).unwrap();
        let error = provider
            .digest(path.to_string_lossy().into(), "hosted".into())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("outside the workspace"));
    }

    #[tokio::test]
    async fn current_program_includes_live_imports_in_its_digest() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("helper.tl"),
            "pub fn value() -> int { return 1; }\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("main.tl"),
            "import \"helper.tl\" as helper;\nfn main() { print(helper::value()); }\n",
        )
        .unwrap();
        let provider = WorkspaceProgramDigest::new(root.path()).unwrap();
        let original = provider
            .digest("main.tl".into(), "hosted".into())
            .await
            .unwrap();
        std::fs::write(
            root.path().join("helper.tl"),
            "pub fn value() -> int { return 2; }\n",
        )
        .unwrap();
        let changed = provider
            .digest("main.tl".into(), "hosted".into())
            .await
            .unwrap();
        assert_ne!(original, changed);
    }
}
