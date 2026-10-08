//! 与 UI 无关的分支对话快照和工作区绑定。

use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use teal_agent::{AgentSession, AgentSessionStatus};

const BRANCH_STORE_VERSION: u16 = 2;
const MAX_BRANCH_STORE_BYTES: usize = 16 * 1024 * 1024;

/// 创建分支对话时固定的工作区和 Git 事实。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationBaseline {
    /// 规范化的工作台工作区路径。
    pub workspace: String,
    /// Git 仓库根目录。
    pub git_root: String,
    /// 当前分支名；分离 HEAD 使用 `HEAD`。
    pub git_branch: String,
    /// 当前提交。
    pub git_head: String,
    /// HEAD、跟踪文件差异和未跟踪文件内容共同形成的摘要。
    pub workspace_digest: String,
}

impl ConversationBaseline {
    /// 从当前工作区捕获只读 Git 快照。
    ///
    /// 此操作会调用本机 `git`，但不会修改索引、分支或工作树。未跟踪普通文件的路径和
    /// 内容也进入摘要，因此同一路径下的晚到修改不能复用旧分支对话。
    ///
    /// # Errors
    /// 工作区无法规范化、不是 Git 工作树、Git 查询失败，或未跟踪路径逃逸工作区时返回
    /// 错误。
    pub fn capture(workspace: impl AsRef<Path>) -> Result<Self, ConversationBranchError> {
        Self::capture_with_overlays(workspace, &[])
    }

    /// 捕获 Git 工作树，并把宿主当前打开的命名缓冲区叠加到摘要。
    ///
    /// overlay 路径必须位于工作区内；调用方应传入尚未写盘的共享缓冲区文本。排序后再
    /// 摘要可确保面板遍历顺序不改变同一工作区身份。
    ///
    /// # Errors
    /// 返回与 [`Self::capture`] 相同的 Git/路径错误；overlay 越界时也会失败。
    pub fn capture_with_overlays(
        workspace: impl AsRef<Path>,
        overlays: &[(PathBuf, String)],
    ) -> Result<Self, ConversationBranchError> {
        let workspace = workspace.as_ref().canonicalize()?;
        let git_root = git_text(&workspace, &["rev-parse", "--show-toplevel"])?;
        let git_root = PathBuf::from(git_root.trim()).canonicalize()?;
        if !workspace.starts_with(&git_root) {
            return Err(ConversationBranchError::Invalid(
                "workspace is outside the reported Git root".into(),
            ));
        }
        let git_head = git_text(&workspace, &["rev-parse", "HEAD"])?;
        let git_branch = git_text(&workspace, &["branch", "--show-current"])?;
        let git_branch = if git_branch.trim().is_empty() {
            "HEAD".to_owned()
        } else {
            git_branch.trim().to_owned()
        };
        let tracked = git_bytes(
            &workspace,
            &[
                "diff",
                "--binary",
                "HEAD",
                "--",
                ".",
                ":(exclude).teal/bench/**",
            ],
        )?;
        let untracked = git_bytes(
            &workspace,
            &[
                "ls-files",
                "--others",
                "--exclude-standard",
                "-z",
                "--",
                ".",
            ],
        )?;

        let mut digest = Sha256::new();
        digest_field(&mut digest, git_head.trim().as_bytes());
        digest_field(&mut digest, &tracked);
        for relative in untracked
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            let relative = std::str::from_utf8(relative).map_err(|_| {
                ConversationBranchError::Invalid("untracked Git path is not UTF-8".into())
            })?;
            let normalized = relative.replace('\\', "/");
            if normalized == ".teal/bench" || normalized.starts_with(".teal/bench/") {
                continue;
            }
            let path = workspace.join(relative);
            let resolved = path.canonicalize()?;
            if !resolved.starts_with(&workspace) || !resolved.is_file() {
                return Err(ConversationBranchError::Invalid(
                    "untracked Git path escapes the workspace or is not a file".into(),
                ));
            }
            digest_field(&mut digest, normalized.as_bytes());
            digest_field(&mut digest, &fs::read(resolved)?);
        }
        let mut overlays = overlays.to_vec();
        overlays.sort_by(|left, right| left.0.cmp(&right.0));
        for (path, text) in overlays {
            let path = path.canonicalize()?;
            if !path.starts_with(&workspace) || !path.is_file() {
                return Err(ConversationBranchError::Invalid(
                    "conversation overlay must be a workspace file".into(),
                ));
            }
            let relative = path
                .strip_prefix(&workspace)
                .expect("overlay was verified inside the workspace")
                .to_string_lossy()
                .replace('\\', "/");
            digest_field(&mut digest, relative.as_bytes());
            digest_field(&mut digest, text.as_bytes());
        }

        Ok(Self {
            workspace: workspace.to_string_lossy().into_owned(),
            git_root: git_root.to_string_lossy().into_owned(),
            git_branch,
            git_head: git_head.trim().to_owned(),
            workspace_digest: format!("{:x}", digest.finalize()),
        })
    }
}

/// 分支对话相对于其固定基线的状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationBranchState {
    /// 工作区仍与创建时基线一致。
    Active,
    /// Git 或工作树事实已经改变，禁止载入为可继续会话。
    Stale,
}

/// 一份可独立继续的 Agent 会话分支。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversationBranch {
    /// 用户可见且在工作区内唯一的分支名。
    pub id: String,
    /// 可选来源分支；根对话为空。
    pub parent: Option<String>,
    /// 创建时使用的命名 Agent。
    pub agent: String,
    /// 固定工作区、Git 分支和内容摘要。
    pub baseline: ConversationBaseline,
    /// 与来源会话分离的完整历史副本。
    pub session: AgentSession,
    /// 当前基线状态。
    pub state: ConversationBranchState,
    /// 创建时间，Unix 毫秒。
    pub created_at_ms: i64,
    /// 最近保存时间，Unix 毫秒。
    pub updated_at_ms: i64,
}

/// 分支对话存储错误。
#[derive(Debug, thiserror::Error)]
pub enum ConversationBranchError {
    /// 文件或 Git 进程操作失败。
    #[error("Conversation branch I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// 持久 JSON 无效。
    #[error("Conversation branch JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    /// 名称、版本、工作区或基线不符合契约。
    #[error("Conversation branch is invalid: {0}")]
    Invalid(String),
    /// 请求的分支不存在。
    #[error("Conversation branch `{0}` does not exist")]
    NotFound(String),
    /// 当前工作区不再匹配分支基线。
    #[error("Conversation branch `{0}` is stale")]
    Stale(String),
}

#[derive(Serialize, Deserialize)]
struct BranchDocument {
    version: u16,
    workspace: String,
    branches: Vec<ConversationBranch>,
}

/// 保存在工作区 `.teal/bench` 下的分支对话目录。
pub struct ConversationBranchStore {
    path: PathBuf,
    workspace: String,
    branches: Vec<ConversationBranch>,
}

impl ConversationBranchStore {
    /// 打开工作区的分支对话目录；文件不存在时返回空目录。
    ///
    /// # Errors
    /// 工作区无法规范化、文件过大、版本/工作区不匹配或 JSON 无效时返回错误。
    pub fn open(workspace: impl AsRef<Path>) -> Result<Self, ConversationBranchError> {
        let workspace = workspace.as_ref().canonicalize()?;
        let identity = workspace.to_string_lossy().into_owned();
        let path = workspace.join(".teal/bench/conversations.json");
        if !path.exists() {
            return Ok(Self {
                path,
                workspace: identity,
                branches: Vec::new(),
            });
        }
        let bytes = fs::read(&path)?;
        if bytes.len() > MAX_BRANCH_STORE_BYTES {
            return Err(ConversationBranchError::Invalid(
                "branch store exceeds 16 MiB".into(),
            ));
        }
        let mut document: BranchDocument = serde_json::from_slice(&bytes)?;
        if document.version != BRANCH_STORE_VERSION {
            return Err(ConversationBranchError::Invalid(format!(
                "unsupported branch store version {}",
                document.version
            )));
        }
        if document.workspace != identity {
            return Err(ConversationBranchError::Invalid(
                "branch store workspace identity does not match".into(),
            ));
        }
        // 分支对话不携带有效审批或活跃终端句柄：加载时只保留完整历史，
        // 去掉循环续接载荷，且不会重新解释在途效果。
        for branch in &mut document.branches {
            branch.session = sanitize_branch_session(&branch.session);
        }
        let mut ids = BTreeSet::new();
        for branch in &document.branches {
            validate_name(&branch.id)?;
            if !ids.insert(branch.id.as_str()) {
                return Err(ConversationBranchError::Invalid(
                    "branch store contains duplicate names".into(),
                ));
            }
            if branch.baseline.workspace != identity {
                return Err(ConversationBranchError::Invalid(
                    "branch baseline belongs to another workspace".into(),
                ));
            }
        }
        if document.branches.iter().any(|branch| {
            branch
                .parent
                .as_deref()
                .is_some_and(|parent| parent == branch.id || !ids.contains(parent))
        }) {
            return Err(ConversationBranchError::Invalid(
                "branch store contains an invalid parent".into(),
            ));
        }
        Ok(Self {
            path,
            workspace: identity,
            branches: document.branches,
        })
    }

    /// 返回按创建顺序保存的全部分支。
    #[must_use]
    pub fn branches(&self) -> &[ConversationBranch] {
        &self.branches
    }

    /// 从一份会话快照创建独立分支。
    ///
    /// # Errors
    /// 名称无效或重复、父分支不存在、基线属于其他工作区，或原子保存失败时返回错误。
    pub fn fork(
        &mut self,
        id: impl Into<String>,
        parent: Option<&str>,
        agent: impl Into<String>,
        baseline: ConversationBaseline,
        session: &AgentSession,
        now_ms: i64,
    ) -> Result<ConversationBranch, ConversationBranchError> {
        let id = id.into();
        let agent = agent.into();
        validate_name(&id)?;
        if self.branches.iter().any(|branch| branch.id == id) {
            return Err(ConversationBranchError::Invalid(format!(
                "branch `{id}` already exists"
            )));
        }
        if let Some(parent) = parent {
            let source = self
                .branches
                .iter()
                .find(|branch| branch.id == parent)
                .ok_or_else(|| ConversationBranchError::NotFound(parent.into()))?;
            if source.state == ConversationBranchState::Stale || source.baseline != baseline {
                return Err(ConversationBranchError::Invalid(
                    "parent branch does not match the workspace baseline".into(),
                ));
            }
        }
        if baseline.workspace != self.workspace {
            return Err(ConversationBranchError::Invalid(
                "branch baseline belongs to another workspace".into(),
            ));
        }
        let branch = ConversationBranch {
            id,
            parent: parent.map(str::to_owned),
            agent,
            baseline,
            session: sanitize_branch_session(session),
            state: ConversationBranchState::Active,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        self.branches.push(branch.clone());
        self.persist()?;
        Ok(branch)
    }

    /// 在当前工作区仍与基线一致时返回独立会话副本。
    ///
    /// # Errors
    /// 分支不存在、基线已变化或状态保存失败时返回错误。发现变化时会先把分支持久标记
    /// 为 `Stale`，不会返回旧会话供调用方继续执行。
    pub fn checkout(
        &mut self,
        id: &str,
        current: &ConversationBaseline,
    ) -> Result<AgentSession, ConversationBranchError> {
        let Some(index) = self.branches.iter().position(|branch| branch.id == id) else {
            return Err(ConversationBranchError::NotFound(id.into()));
        };
        if self.branches[index].state == ConversationBranchState::Stale
            || self.branches[index].baseline != *current
        {
            self.branches[index].state = ConversationBranchState::Stale;
            self.persist()?;
            return Err(ConversationBranchError::Stale(id.into()));
        }
        Ok(self.branches[index].session.clone())
    }

    /// 保存继续后的会话，但要求调用方再次提供相同工作区基线。
    ///
    /// # Errors
    /// 分支不存在、基线过期或原子保存失败时返回错误。
    pub fn save_session(
        &mut self,
        id: &str,
        current: &ConversationBaseline,
        session: &AgentSession,
        now_ms: i64,
    ) -> Result<(), ConversationBranchError> {
        let Some(index) = self.branches.iter().position(|branch| branch.id == id) else {
            return Err(ConversationBranchError::NotFound(id.into()));
        };
        if self.branches[index].state == ConversationBranchState::Stale
            || self.branches[index].baseline != *current
        {
            self.branches[index].state = ConversationBranchState::Stale;
            self.persist()?;
            return Err(ConversationBranchError::Stale(id.into()));
        }
        self.branches[index].session = sanitize_branch_session(session);
        self.branches[index].updated_at_ms = now_ms;
        self.persist()
    }

    fn persist(&self) -> Result<(), ConversationBranchError> {
        let parent = self.path.parent().ok_or_else(|| {
            ConversationBranchError::Invalid("branch store path has no parent".into())
        })?;
        fs::create_dir_all(parent)?;
        let bytes = serde_json::to_vec_pretty(&BranchDocument {
            version: BRANCH_STORE_VERSION,
            workspace: self.workspace.clone(),
            branches: self.branches.clone(),
        })?;
        if bytes.len() > MAX_BRANCH_STORE_BYTES {
            return Err(ConversationBranchError::Invalid(
                "branch store exceeds 16 MiB".into(),
            ));
        }
        let mut temporary = NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.flush()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        Ok(())
    }
}

/// 去掉分支快照中的循环续接载荷。
///
/// 分支只保存已经发生的完整历史：审批、凭证和宿主终端会话句柄都不属于会话快照，
/// 而在途循环状态可能引用它们，因此必须在保存和迁移时清除。运行中的快照清除续接后
/// 无法安全继续，明确降级为需要检查的状态。
fn sanitize_branch_session(session: &AgentSession) -> AgentSession {
    let mut session = session.clone();
    let in_flight = session.continuation.is_some() || session.status == AgentSessionStatus::Running;
    session.continuation = None;
    if in_flight {
        session.status = AgentSessionStatus::NeedsAttention;
    }
    session
}

fn validate_name(name: &str) -> Result<(), ConversationBranchError> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ConversationBranchError::Invalid(
            "branch name must use 1-64 ASCII letters, digits, '-' or '_'".into(),
        ));
    }
    Ok(())
}

fn git_text(workspace: &Path, arguments: &[&str]) -> Result<String, ConversationBranchError> {
    let bytes = git_bytes(workspace, arguments)?;
    String::from_utf8(bytes)
        .map_err(|_| ConversationBranchError::Invalid("Git output is not UTF-8".into()))
}

fn git_bytes(workspace: &Path, arguments: &[&str]) -> Result<Vec<u8>, ConversationBranchError> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(workspace)
        .output()?;
    if !output.status.success() {
        return Err(ConversationBranchError::Invalid(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(output.stdout)
}

fn digest_field(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}
