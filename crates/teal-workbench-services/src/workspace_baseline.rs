//! 无界面宿主可复用的只读 Git 工作区基线。

use std::path::PathBuf;

/// 捕获 Git HEAD、已跟踪差异和未跟踪文件内容的宿主提供器。
pub struct GitWorkspaceBaseline {
    workspace: PathBuf,
}

impl GitWorkspaceBaseline {
    /// 绑定一个工作区目录；实际 Git 状态在每次请求时重新读取。
    #[must_use]
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }
}

impl teal_agent::WorkspaceBaselineProvider for GitWorkspaceBaseline {
    fn capture(&self) -> teal_agent::ProviderFuture<Result<String, teal_agent::AgentError>> {
        let workspace = self.workspace.clone();
        Box::pin(async move {
            let baseline = tokio::task::spawn_blocking(move || {
                crate::agent_algorithms::ConversationBaseline::capture(workspace)
            })
            .await
            .map_err(|error| teal_agent::AgentError::Protocol(error.to_string()))?
            .map_err(|error| teal_agent::AgentError::Protocol(error.to_string()))?;
            serde_json::to_string(&baseline)
                .map_err(|error| teal_agent::AgentError::Protocol(error.to_string()))
        })
    }
}
