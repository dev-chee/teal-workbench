//! 以路径、内容和完整能力集绑定的工作区授权存储。
//!
//! 只有精确快照匹配才视为已授权；内容或能力集发生任何变化都需要重新批准。
//! 此模块保存授权数据，不负责判断脚本属于用户配置还是工作区代码。

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use thiserror::Error;

/// 持久化且绑定内容的能力批准。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Grant {
    hash: [u8; 32],
    capabilities: BTreeSet<String>,
}

/// 工作区脚本在当前内容与导入下是否可以执行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustDecision {
    /// 精确路径、内容哈希和能力集合已获批准。
    Trusted,

    /// 执行前需要用户批准。
    ApprovalRequired {
        /// 规范脚本路径。
        path: PathBuf,

        /// 已解析宿主导入请求的能力。
        capabilities: BTreeSet<String>,
    },
}

/// 信任存储访问或解码失败。
#[derive(Debug, Error)]
pub enum TrustError {
    /// 文件系统访问失败。
    #[error("Script trust store read and write failed: {0}")]
    Io(#[from] std::io::Error),

    /// JSON 解码失败。
    #[error("Script trust store format error: {0}")]
    Json(#[from] serde_json::Error),
}

/// 绑定路径、内容和能力的工作区脚本批准。
#[derive(Clone, Debug)]
pub struct TrustStore {
    path: PathBuf,
    grants: BTreeMap<String, Grant>,
}

impl TrustStore {
    /// 加载现有存储；文件不存在时从空存储开始。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, TrustError> {
        let path = path.as_ref().to_path_buf();
        let grants = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self { path, grants })
    }

    /// 检查精确脚本快照而不更改批准状态。
    ///
    /// `contents` 是由调用方确定的信任字节，Teal 工作台传入的是完整导入闭包而非单个入口文件。
    /// 路径规范化失败时保留原路径；能力集必须完全相等，不能把旧授权当作更小能力集的自动授权。
    #[must_use]
    pub fn check(
        &self,
        path: &Path,
        contents: &[u8],
        capabilities: &BTreeSet<String>,
    ) -> TrustDecision {
        self.check_with_inputs(path, contents, capabilities, std::iter::once(path))
    }

    /// 检查精确脚本批准或覆盖全部导入输入的目录能力批准。
    #[must_use]
    pub fn check_with_inputs<'a>(
        &self,
        path: &Path,
        contents: &[u8],
        capabilities: &BTreeSet<String>,
        inputs: impl IntoIterator<Item = &'a Path>,
    ) -> TrustDecision {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let key = canonical.to_string_lossy().into_owned();
        let hash: [u8; 32] = Sha256::digest(contents).into();

        // 能力集即使缩小，也代表不同的批准快照。
        if self
            .grants
            .get(&key)
            .is_some_and(|grant| grant.hash == hash && &grant.capabilities == capabilities)
        {
            return TrustDecision::Trusted;
        }
        let inputs = inputs
            .into_iter()
            .map(|input| input.canonicalize().unwrap_or_else(|_| input.to_path_buf()))
            .collect::<Vec<_>>();
        for (key, grant) in &self.grants {
            let Some(root) = key.strip_prefix("directory:") else {
                continue;
            };
            let root = PathBuf::from(root);
            if canonical.starts_with(&root)
                && capabilities.is_subset(&grant.capabilities)
                && inputs.iter().all(|input| input.starts_with(&root))
            {
                return TrustDecision::Trusted;
            }
        }
        TrustDecision::ApprovalRequired {
            path: canonical,
            capabilities: capabilities.clone(),
        }
    }

    /// 在内存中登记精确脚本快照，并原子持久化授权表。
    ///
    /// 必须在取得用户批准后调用。磁盘持久化失败会返回错误，但内存中的登记已经发生；
    /// 调用方不得把错误当作授权已可靠保存，需自行决定是否丢弃或重新加载此存储。
    pub fn approve(
        &mut self,
        path: &Path,
        contents: &[u8],
        capabilities: BTreeSet<String>,
    ) -> Result<(), TrustError> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.grants.insert(
            canonical.to_string_lossy().into_owned(),
            Grant {
                hash: Sha256::digest(contents).into(),
                capabilities,
            },
        );
        self.persist()
    }

    /// 批准一个规范目录及其最大能力集合；目录外导入仍需独立批准。
    pub fn approve_directory(
        &mut self,
        root: &Path,
        capabilities: BTreeSet<String>,
    ) -> Result<(), TrustError> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        self.grants.insert(
            format!("directory:{}", root.to_string_lossy()),
            Grant {
                hash: [0; 32],
                capabilities,
            },
        );
        self.persist()
    }

    /// 删除全部目录级批准；精确脚本快照批准保持不变。
    pub fn clear_directory_approvals(&mut self) -> Result<(), TrustError> {
        self.grants.retain(|key, _| !key.starts_with("directory:"));
        self.persist()
    }

    fn persist(&self) -> Result<(), TrustError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let mut temporary = NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut temporary, &self.grants)?;
        temporary.write_all(b"\n")?;
        temporary.as_file_mut().sync_all()?;
        temporary
            .persist(&self.path)
            .map_err(|error| TrustError::Io(error.error))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use tempfile::tempdir;

    use super::{TrustDecision, TrustStore};

    #[test]
    fn approvals_are_invalidated_by_content_or_capability_changes() {
        let directory = tempdir().expect("temporary directory");
        let script = directory.path().join("config.teal");
        std::fs::write(&script, "first").expect("Script test data");
        let mut store = TrustStore::load(directory.path().join("trust.json")).expect("trust store");
        let capabilities = BTreeSet::from(["editor.read".to_owned()]);
        assert!(matches!(
            store.check(&script, b"first", &capabilities),
            TrustDecision::ApprovalRequired { .. }
        ));
        store
            .approve(&script, b"first", capabilities.clone())
            .expect("approval");
        assert_eq!(
            store.check(&script, b"first", &capabilities),
            TrustDecision::Trusted
        );
        assert!(matches!(
            store.check(&script, b"second", &capabilities),
            TrustDecision::ApprovalRequired { .. }
        ));
        assert!(matches!(
            store.check(
                &script,
                b"first",
                &BTreeSet::from(["editor.write".to_owned()]),
            ),
            TrustDecision::ApprovalRequired { .. }
        ));
    }

    #[test]
    fn directory_approval_allows_source_edits_but_rejects_capability_or_import_escape() {
        let directory = tempdir().expect("temporary directory");
        let root = directory.path().join("approved");
        let outside = directory.path().join("outside");
        std::fs::create_dir_all(&root).expect("approved root");
        std::fs::create_dir_all(&outside).expect("outside root");
        let script = root.join("config.tl");
        let local_import = root.join("helpers.tl");
        let external_import = outside.join("dependency.tl");
        std::fs::write(&script, "first").expect("script fixture");
        std::fs::write(&local_import, "helper").expect("local import fixture");
        std::fs::write(&external_import, "outside").expect("external import fixture");
        let mut store = TrustStore::load(directory.path().join("trust.json")).expect("trust store");
        let capabilities = BTreeSet::from(["editor.read".to_owned()]);

        store
            .approve_directory(&root, capabilities.clone())
            .expect("directory approval");
        assert_eq!(
            store.check_with_inputs(
                &script,
                b"changed source is still in the approved directory",
                &capabilities,
                [script.as_path(), local_import.as_path()],
            ),
            TrustDecision::Trusted
        );
        assert!(matches!(
            store.check_with_inputs(
                &script,
                b"changed source",
                &BTreeSet::from(["editor.read".to_owned(), "editor.write".to_owned()]),
                [script.as_path(), local_import.as_path()],
            ),
            TrustDecision::ApprovalRequired { .. }
        ));
        assert!(matches!(
            store.check_with_inputs(
                &script,
                b"changed source",
                &capabilities,
                [script.as_path(), external_import.as_path()],
            ),
            TrustDecision::ApprovalRequired { .. }
        ));

        store
            .clear_directory_approvals()
            .expect("directory approvals should clear");
        assert!(matches!(
            store.check_with_inputs(
                &script,
                b"changed source",
                &capabilities,
                [script.as_path()],
            ),
            TrustDecision::ApprovalRequired { .. }
        ));
    }
}
