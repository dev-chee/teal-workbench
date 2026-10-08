//! 工作区脚本清单：入口、运行配置档、权限文件与测试文件。
//!
//! 清单是工作区内的显式登记处，源码目录本身不强制约定；读取时校验版本、名称、
//! 工作区内路径与唯一性，写入使用原子替换。清单只描述脚本，不运行脚本，也不授予权限。

use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use thiserror::Error;
use teal_workbench_core::{BufferStore, WorkspaceMutationCoordinator, WorkspaceIntent};

/// 清单在工作区内的相对路径。
pub const SCRIPT_MANIFEST_PATH: &str = ".teal/scripts.toml";

/// 新脚本的默认目录。
pub const DEFAULT_SCRIPT_DIR: &str = "scripts";

/// 当前清单格式版本。
const MANIFEST_VERSION: u16 = 1;

/// 一个脚本的登记项。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptEntry {
    /// 工作区内唯一的脚本名称。
    pub name: String,

    /// 相对工作区的入口 `.tl` 路径。
    pub entry: PathBuf,

    /// 运行时配置档名称。
    pub profile: String,

    /// 可选的相对工作区权限文件。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<PathBuf>,

    /// 相对工作区的测试入口。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<PathBuf>,

    /// 相对工作区的交付产物。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<PathBuf>,
}

impl ScriptEntry {
    /// 创建使用默认配置档的登记项。
    #[must_use]
    pub fn new(name: impl Into<String>, entry: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            entry: entry.into(),
            profile: "core".to_owned(),
            permissions: None,
            tests: Vec::new(),
            artifacts: Vec::new(),
        }
    }
}

/// 清单文档。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestDocument {
    version: u16,
    #[serde(default)]
    scripts: Vec<ScriptEntry>,
}

/// 工作区脚本清单。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptManifest {
    scripts: Vec<ScriptEntry>,
}

/// 清单的读取、校验或写入失败。
#[derive(Debug, Error)]
pub enum ScriptManifestError {
    /// 文件系统访问失败。
    #[error("Script manifest access failed: {0}")]
    Io(#[from] std::io::Error),

    /// TOML 内容无效。
    #[error("Script manifest is invalid: {0}")]
    Toml(#[from] toml::de::Error),

    /// 编码失败。
    #[error("Script manifest could not be encoded: {0}")]
    Encode(#[from] toml::ser::Error),

    /// 版本或内容不满足当前契约。
    #[error("Script manifest is invalid: {0}")]
    Invalid(String),

    /// 共享工作区事务拒绝或无法提交清单修改。
    #[error("Script manifest workspace transaction failed: {0}")]
    Workspace(String),
}

impl ScriptManifest {
    /// 返回一个空清单。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 从工作区读取清单；文件不存在时返回空清单。
    ///
    /// # Errors
    /// 文件不可读、TOML 无效、版本不受支持或登记项越界时返回错误。
    pub fn load(workspace: &Path) -> Result<Self, ScriptManifestError> {
        let path = workspace.join(SCRIPT_MANIFEST_PATH);
        if !path.exists() {
            return Ok(Self::new());
        }
        let text = fs::read_to_string(&path)?;
        let document: ManifestDocument = toml::from_str(&text)?;
        if document.version != MANIFEST_VERSION {
            return Err(ScriptManifestError::Invalid(format!(
                "unsupported manifest version {}",
                document.version
            )));
        }
        let manifest = Self {
            scripts: document.scripts,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// 把清单原子写入工作区。
    ///
    /// # Errors
    /// 写入失败、目录无法创建或内容不满足契约时返回错误。
    pub fn save(&self, workspace: &Path) -> Result<(), ScriptManifestError> {
        self.validate()?;
        let document = ManifestDocument {
            version: MANIFEST_VERSION,
            scripts: self.scripts.clone(),
        };
        let text = toml::to_string_pretty(&document)?;
        let path = workspace.join(SCRIPT_MANIFEST_PATH);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = NamedTempFile::new_in(
            path.parent()
                .ok_or_else(|| ScriptManifestError::Invalid("manifest has no parent".into()))?,
        )?;
        file.write_all(text.as_bytes())?;
        file.flush()?;
        file.persist(&path).map_err(|error| error.error)?;
        Ok(())
    }

    /// 通过共享工作区事务保存清单。
    ///
    /// 工作台调用此入口，使清单写入与脚本、LSP 和 Agent 文件修改共用路径校验、审计和版本边界。
    ///
    /// # Errors
    /// 清单无效、父目录创建失败、审计失败或工作区版本检查失败时返回错误。
    pub fn save_with_workspace_transaction(
        &self,
        workspace: &Path,
        buffers: &BufferStore,
        coordinator: &WorkspaceMutationCoordinator,
    ) -> Result<(), ScriptManifestError> {
        self.validate()?;
        let document = ManifestDocument {
            version: MANIFEST_VERSION,
            scripts: self.scripts.clone(),
        };
        let text = toml::to_string_pretty(&document)?;
        let path = workspace.join(SCRIPT_MANIFEST_PATH);
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            coordinator
                .execute(
                    buffers,
                    WorkspaceIntent::Create {
                        path: parent.to_path_buf(),
                        directory: true,
                    },
                    true,
                )
                .map_err(ScriptManifestError::Workspace)?;
        }
        coordinator
            .write(buffers, &path, &text, true)
            .map_err(ScriptManifestError::Workspace)
    }

    /// 返回全部登记项，按名称排序。
    #[must_use]
    pub fn scripts(&self) -> &[ScriptEntry] {
        &self.scripts
    }

    /// 按名称查找登记项。
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&ScriptEntry> {
        self.scripts.iter().find(|entry| entry.name == name)
    }

    /// 插入或替换登记项，并按名称保持稳定顺序。
    pub fn upsert(&mut self, entry: ScriptEntry) {
        match self
            .scripts
            .iter_mut()
            .find(|existing| existing.name == entry.name)
        {
            Some(existing) => *existing = entry,
            None => self.scripts.push(entry),
        }
        self.scripts
            .sort_by(|left, right| left.name.cmp(&right.name));
    }

    /// 删除登记项；返回是否删除了内容。
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.scripts.len();
        self.scripts.retain(|entry| entry.name != name);
        before != self.scripts.len()
    }

    /// 校验名称、路径与配置档。
    ///
    /// # Errors
    /// 名称非法或重复、路径为绝对路径或指向工作区外、配置档为空时返回错误。
    pub fn validate(&self) -> Result<(), ScriptManifestError> {
        let mut names = BTreeSet::new();
        for entry in &self.scripts {
            validate_name(&entry.name)?;
            if !names.insert(entry.name.as_str()) {
                return Err(ScriptManifestError::Invalid(format!(
                    "script name `{}` is duplicated",
                    entry.name
                )));
            }
            validate_relative("entry", &entry.entry)?;
            if entry.entry.extension().and_then(|value| value.to_str()) != Some("tl") {
                return Err(ScriptManifestError::Invalid(format!(
                    "script `{}` entry must be a .tl file",
                    entry.name
                )));
            }
            if entry.profile.trim().is_empty() {
                return Err(ScriptManifestError::Invalid(format!(
                    "script `{}` profile must not be empty",
                    entry.name
                )));
            }
            if let Some(permissions) = &entry.permissions {
                validate_relative("permissions", permissions)?;
            }
            for test in &entry.tests {
                validate_relative("tests", test)?;
            }
            for artifact in &entry.artifacts {
                validate_relative("artifacts", artifact)?;
            }
        }
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<(), ScriptManifestError> {
    if name.trim().is_empty() || name.trim() != name {
        return Err(ScriptManifestError::Invalid(
            "script name must not be empty or padded".into(),
        ));
    }
    if name.len() > 64 {
        return Err(ScriptManifestError::Invalid(
            "script name must not exceed 64 bytes".into(),
        ));
    }
    if !name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        return Err(ScriptManifestError::Invalid(format!(
            "script name `{name}` accepts ASCII letters, digits, `-`, `_` and `.` only"
        )));
    }
    Ok(())
}

fn validate_relative(field: &str, path: &Path) -> Result<(), ScriptManifestError> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(ScriptManifestError::Invalid(format!(
            "{field} path must be relative to the workspace"
        )));
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(ScriptManifestError::Invalid(format!(
            "{field} path must stay inside the workspace"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_SCRIPT_DIR, SCRIPT_MANIFEST_PATH, ScriptEntry, ScriptManifest, ScriptManifestError,
    };

    #[test]
    fn round_trips_entries_without_creating_a_file_for_an_empty_manifest() {
        let workspace = tempfile::tempdir().expect("workspace");
        let empty = ScriptManifest::load(workspace.path()).expect("missing manifest loads empty");
        assert!(empty.scripts().is_empty());
        assert!(!workspace.path().join(SCRIPT_MANIFEST_PATH).exists());

        let mut manifest = ScriptManifest::new();
        let mut entry = ScriptEntry::new("hello", format!("{DEFAULT_SCRIPT_DIR}/hello.tl"));
        entry.tests.push("scripts/hello_test.tl".into());
        entry.artifacts.push("target/hello.txt".into());
        manifest.upsert(entry);
        manifest.save(workspace.path()).expect("save");

        let loaded = ScriptManifest::load(workspace.path()).expect("load");
        assert_eq!(loaded, manifest);
        assert_eq!(
            loaded.find("hello").map(|entry| entry.entry.clone()),
            Some("scripts/hello.tl".into())
        );
        assert_eq!(
            loaded.find("hello").map(|entry| entry.profile.as_str()),
            Some("core")
        );
    }

    #[test]
    fn upsert_replaces_and_sorts_entries() {
        let mut manifest = ScriptManifest::new();
        manifest.upsert(ScriptEntry::new("zeta", "scripts/zeta.tl"));
        manifest.upsert(ScriptEntry::new("alpha", "scripts/alpha.tl"));
        assert_eq!(
            manifest
                .scripts()
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );
        let mut replacement = ScriptEntry::new("alpha", "scripts/alpha2.tl");
        replacement.profile = "agent".into();
        manifest.upsert(replacement);
        assert_eq!(manifest.scripts().len(), 2);
        assert_eq!(
            manifest.find("alpha").map(|entry| entry.entry.clone()),
            Some("scripts/alpha2.tl".into())
        );
        assert!(manifest.remove("alpha"));
        assert!(!manifest.remove("alpha"));
        assert_eq!(manifest.scripts().len(), 1);
    }

    #[test]
    fn rejects_unknown_versions_duplicates_and_escaping_paths() {
        let workspace = tempfile::tempdir().expect("workspace");
        let path = workspace.path().join(SCRIPT_MANIFEST_PATH);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("directory");
        std::fs::write(
            &path,
            "version = 2\n[[scripts]]\nname = \"hello\"\nentry = \"scripts/hello.tl\"\nprofile = \"core\"\n",
        )
        .expect("fixture");
        assert!(matches!(
            ScriptManifest::load(workspace.path()),
            Err(ScriptManifestError::Invalid(reason)) if reason.contains("unsupported")
        ));

        std::fs::write(
            &path,
            "version = 1\n[[scripts]]\nname = \"hello\"\nentry = \"scripts/hello.tl\"\nprofile = \"core\"\n\
             [[scripts]]\nname = \"hello\"\nentry = \"scripts/other.tl\"\nprofile = \"core\"\n",
        )
        .expect("fixture");
        assert!(matches!(
            ScriptManifest::load(workspace.path()),
            Err(ScriptManifestError::Invalid(reason)) if reason.contains("duplicated")
        ));

        let mut manifest = ScriptManifest::new();
        manifest.upsert(ScriptEntry::new("escape", "../outside.tl"));
        assert!(manifest.validate().is_err());
        let mut manifest = ScriptManifest::new();
        manifest.upsert(ScriptEntry::new("bad name", "scripts/ok.tl"));
        assert!(manifest.validate().is_err());
        let mut manifest = ScriptManifest::new();
        manifest.upsert(ScriptEntry::new("binary", "scripts/thing.txt"));
        assert!(manifest.validate().is_err());
        assert!(ScriptManifest::new().validate().is_ok());
    }
}
