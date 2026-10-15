//! 工作区项目指令发现。每次读取都通过宿主权限端口，并拒绝目录链接逃逸。

use sha2::{Digest, Sha256};
use std::io::Read;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};
use teal_agent::{
    ActionContext, ActionResourceAccess, ActionSpec, AgentError, Decision, InstructionFragment,
    InstructionsProvider, InstructionsSnapshot, PermissionProvider, ProviderFuture,
    ResourceVersion,
};

/// 从受信任工作区的根到已访问路径发现 AGENTS.md，保持来源与目录作用域。
pub struct ProjectInstructions {
    root: PathBuf,
    trusted: bool,
    permissions: Arc<dyn PermissionProvider>,
    configuration: Option<teal_config::ConfigPaths>,
}

impl ProjectInstructions {
    /// 创建一个不执行文件读取的指令来源。工作区未受信任时返回空快照。
    ///
    /// # Errors
    /// 根目录不存在、不是目录或无法规范化时拒绝。
    pub fn new(
        root: &Path,
        trusted: bool,
        permissions: Arc<dyn PermissionProvider>,
    ) -> Result<Self, AgentError> {
        let root = root.canonicalize().map_err(io_error)?;
        if !root.is_dir() {
            return Err(AgentError::Protocol(
                "instruction root is not a directory".into(),
            ));
        }
        Ok(Self {
            root,
            trusted,
            permissions,
            configuration: None,
        })
    }

    /// 逐请求重新读取工作区信任；撤销信任立即停止注入项目指令。
    #[must_use]
    pub fn with_configuration(mut self, paths: teal_config::ConfigPaths) -> Self {
        self.configuration = Some(paths);
        self
    }
}

impl InstructionsProvider for ProjectInstructions {
    fn snapshot(
        &self,
        context: ActionContext,
        paths: Vec<String>,
    ) -> ProviderFuture<Result<InstructionsSnapshot, AgentError>> {
        let root = self.root.clone();
        let trusted = self.trusted;
        let permissions = Arc::clone(&self.permissions);
        let configuration = self.configuration.clone();
        Box::pin(async move {
            if !is_trusted(trusted, configuration.as_ref())? {
                return InstructionsSnapshot::new(Vec::new());
            }
            let mut directories = BTreeSet::from([root.clone()]);
            for path in paths {
                let path = PathBuf::from(path);
                let path = if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                };
                if path
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
                {
                    return Err(AgentError::Policy(
                        "instruction target escapes workspace".into(),
                    ));
                }
                let mut current = path.as_path();
                while !current.exists() {
                    current = current.parent().ok_or_else(|| {
                        AgentError::Policy("instruction target has no workspace ancestor".into())
                    })?;
                }
                let current = if current.is_dir() {
                    current
                } else {
                    current.parent().unwrap_or(&root)
                };
                // Windows 普通盘符路径与 canonicalize 的扩展前缀必须先归一化再比较。
                if !current.canonicalize().map_err(io_error)?.starts_with(&root) {
                    return Err(AgentError::Policy(
                        "instruction target escapes workspace".into(),
                    ));
                }
                for directory in current.ancestors() {
                    let canonical = directory.canonicalize().map_err(io_error)?;
                    if !canonical.starts_with(&root) {
                        break;
                    }
                    if std::fs::symlink_metadata(directory)
                        .map_err(io_error)?
                        .file_type()
                        .is_symlink()
                    {
                        return Err(AgentError::Policy(
                            "project instruction directory is linked".into(),
                        ));
                    }
                    directories.insert(canonical);
                    if directories.len() > 128 {
                        return Err(AgentError::Protocol("too many instruction scopes".into()));
                    }
                }
            }
            let mut directories = directories.into_iter().collect::<Vec<_>>();
            directories.sort_by(|left, right| {
                left.components()
                    .count()
                    .cmp(&right.components().count())
                    .then(left.cmp(right))
            });
            let mut fragments = Vec::new();
            for directory in directories {
                let path = directory.join("AGENTS.md");
                let metadata = match std::fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(io_error(error)),
                };
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || metadata.len() > 32 * 1024
                {
                    return Err(AgentError::Policy(
                        "project instructions are linked, non-regular, or oversized".into(),
                    ));
                }
                let canonical = path.canonicalize().map_err(io_error)?;
                if !canonical.starts_with(&root)
                    || directory
                        .ancestors()
                        .take_while(|path| path.starts_with(&root))
                        .any(|path| {
                            std::fs::symlink_metadata(path)
                                .is_ok_and(|metadata| metadata.file_type().is_symlink())
                        })
                {
                    return Err(AgentError::Policy(
                        "project instructions escape workspace".into(),
                    ));
                }
                let resource = format!("file://{}", canonical.display());
                let original = read_bounded(&canonical)?;
                let version = format!("{:x}", Sha256::digest(original.as_bytes()));
                let action = ActionSpec {
                    id: format!("instructions:{}", canonical.display()),
                    tool: "workspace.read_range".into(),
                    arguments: serde_json::json!({"path":canonical}),
                    resources: vec![ResourceVersion {
                        access: ActionResourceAccess::Read,
                        resource,
                        version: version.clone(),
                    }],
                };
                match permissions
                    .authorize(context.clone(), action)
                    .await
                    .map_err(|error| AgentError::Policy(error.to_string()))?
                {
                    Decision::Allow => {}
                    Decision::Deny(reason) => return Err(AgentError::Policy(reason)),
                    Decision::ApprovalRequired(prompt) => {
                        return Err(AgentError::NeedsApproval(prompt.binding.digest));
                    }
                }
                // 批准后的路径仍须指向原文件，避免在等待期间把指令切换到范围外。
                if path.canonicalize().map_err(io_error)? != canonical
                    || std::fs::symlink_metadata(&path)
                        .map_err(io_error)?
                        .file_type()
                        .is_symlink()
                {
                    return Err(AgentError::Policy(
                        "project instruction path changed during approval".into(),
                    ));
                }
                if !is_trusted(trusted, configuration.as_ref())? {
                    return InstructionsSnapshot::new(Vec::new());
                }
                let text = read_bounded(&canonical)?;
                if format!("{:x}", Sha256::digest(text.as_bytes())) != version {
                    return Err(AgentError::Policy(
                        "project instructions changed during approval".into(),
                    ));
                }
                fragments.push(InstructionFragment {
                    source: canonical.to_string_lossy().into_owned(),
                    scope: directory.to_string_lossy().into_owned(),
                    text,
                });
            }
            InstructionsSnapshot::new(fragments)
        })
    }
}

fn io_error(error: std::io::Error) -> AgentError {
    AgentError::Protocol(format!("project instructions: {error}"))
}

fn read_bounded(path: &Path) -> Result<String, AgentError> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(io_error)?
        .take(32 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > 32 * 1024 {
        return Err(AgentError::Protocol(
            "project instructions exceed 32 KiB".into(),
        ));
    }
    String::from_utf8(bytes).map_err(|error| AgentError::Protocol(error.to_string()))
}

fn is_trusted(
    fixed: bool,
    configuration: Option<&teal_config::ConfigPaths>,
) -> Result<bool, AgentError> {
    match configuration {
        Some(paths) => {
            teal_security::trust_of(paths, &teal_config::FileConfigSource::new(paths.clone()))
                .map(|trust| trust == teal_security::TrustLevel::Trusted)
                .map_err(|error| AgentError::Policy(error.to_string()))
        }
        None => Ok(fixed),
    }
}
