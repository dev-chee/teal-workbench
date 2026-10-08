//! Teal 工作台的文件系统、搜索、Markdown、进程和会话服务。
//!
//! 本库组合文件浏览、搜索、Markdown、进程、剪贴板与会话服务。
//! 服务返回数据或操作结果，面板创建、确认策略及全局状态更新由嵌入应用负责。

mod clipboard;
mod diagnostic;
mod explorer;
mod markdown;
mod process;
mod search;
mod scripts;
mod session;
mod runtime_lock;
mod runtime_state;
#[cfg(feature = "agent-algorithms")]
pub mod agent_algorithms;
pub use runtime_state::{BufferVersionProvider, WorkspaceRuntimeState};
#[cfg(feature = "tools")]
mod instructions;
#[cfg(feature = "tools")]
mod workspace_baseline;
#[cfg(feature = "tools")]
mod program_digest;
#[cfg(feature = "tools")]
mod verified_commit;
#[cfg(feature = "tools")]
mod delivery;
#[cfg(feature = "tools")]
pub use instructions::ProjectInstructions;
#[cfg(feature = "tools")]
pub use workspace_baseline::GitWorkspaceBaseline;
#[cfg(feature = "tools")]
pub use program_digest::WorkspaceProgramDigest;
#[cfg(feature = "tools")]
pub use delivery::{
    DeliveryArtifact, DeliveryReceipt, DeliveryVerification, compile_digest_in_workspace,
    compile_test_names_in_workspace, delivery_is_current, digest, now_ms,
};
#[cfg(feature = "tools")]
pub use verified_commit::{
    VerifiedCommitService, delivery_paths, execute_agent_commit, preflight_agent_commit,
    preflight_verified_agent_commit,
};
#[cfg(feature = "tools")]
mod knowledge;
#[cfg(feature = "tools")]
pub use knowledge::{KnowledgeLayerStatus, WorkbenchKnowledge, within_workspace_layer};

pub use clipboard::{ClipboardBackend, ClipboardError, SystemClipboard};
pub use diagnostic::{
    CargoDiagnostic, DiagnosticContext, DiagnosticSpan, diagnostic_context, parse_cargo_diagnostics,
};
pub use explorer::{ExplorerError, FileEntry, FileExplorer, FileSort, FileTreeEntry, Preview};
pub use markdown::{MarkdownDocument, MarkdownLine, MarkdownStyle};
pub use process::{
    ProcessError, ProcessOutput, run_command, run_command_blocking, run_program,
    run_program_blocking, run_program_with_input_and_environment,
};
pub use search::{SearchError, SearchMatch, SearchService};
pub use scripts::{
    DEFAULT_SCRIPT_DIR, SCRIPT_MANIFEST_PATH, ScriptEntry, ScriptManifest, ScriptManifestError,
};
pub use session::{
    ExplorerSession, PanelSession, PanelSessionState, SessionError, SessionPaths, SessionState,
    TestSessionRow, TestSessionStatus, load_session, save_session,
};
pub use runtime_lock::{
    RuntimeArtifact, RuntimeCache, RuntimeLock, RuntimeLockError, RuntimeLockStatus,
    RuntimePackage, RuntimeKnowledge,
};

/// 后台作业上下文传播。
pub mod tracing_context;
/// 有界日志、脱敏导出和观测资源生命周期。
#[cfg(feature = "observability")]
pub mod observability;
/// 工作区工具、批准通道和受管平台能力。
#[cfg(feature = "tools")]
pub mod tools;
