//! 源码 Agent 使用共享版本化修改队列的宿主中立端口。

use std::sync::Arc;

use sha2::{Digest, Sha256};
use teal_agent::{
    AgentError, WorkspaceEditReceipt, WorkspaceFileSnapshot, WorkspaceTransactionProvider,
};
use tokio::sync::oneshot;

use super::{MutationKind, MutationRequest, WorkspaceToolRuntime};

impl WorkspaceTransactionProvider for WorkspaceToolRuntime {
    fn create_new(
        &self,
        input: String,
        text: String,
    ) -> teal_agent::ProviderFuture<Result<WorkspaceEditReceipt, AgentError>> {
        let tools = Arc::clone(&self.tools);
        Box::pin(async move {
            if text.len() > 1024 * 1024 {
                return Err(AgentError::Protocol(
                    "workspace text exceeds the 1 MiB transaction limit".into(),
                ));
            }
            let path = tokio::task::spawn_blocking({
                let tools = Arc::clone(&tools);
                move || {
                    tools
                        .resolve_prepared(&input, true)
                        .map_err(|error| AgentError::Protocol(error.to_string()))
                }
            })
            .await
            .map_err(|error| AgentError::Protocol(error.to_string()))??;
            let (response, received) = oneshot::channel();
            tools
                .mutations
                .send(MutationRequest {
                    observation: crate::tracing_context::Context::current(),
                    path: path.clone(),
                    kind: MutationKind::WriteNew(text.clone()),
                    expected_buffer_version: None,
                    expected_content_sha256: None,
                    response,
                })
                .await
                .map_err(|_| AgentError::Protocol("workspace mutation channel is closed".into()))?;
            received
                .await
                .map_err(|_| {
                    AgentError::Protocol("workspace mutation result is unavailable".into())
                })?
                .map_err(AgentError::Protocol)?;
            Ok(WorkspaceEditReceipt {
                path: path.to_string_lossy().into_owned(),
                before_sha256: String::new(),
                after_sha256: format!("{:x}", Sha256::digest(text.as_bytes())),
            })
        })
    }

    fn snapshot(
        &self,
        input: String,
    ) -> teal_agent::ProviderFuture<Result<WorkspaceFileSnapshot, AgentError>> {
        let tools = Arc::clone(&self.tools);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let path = tools
                    .resolve_prepared(&input, true)
                    .map_err(|error| AgentError::Protocol(error.to_string()))?;
                let text = tools
                    .document_text(&path)
                    .map_err(|error| AgentError::Protocol(error.to_string()))?;
                if text.len() > 1024 * 1024 {
                    return Err(AgentError::Protocol(
                        "workspace text exceeds the 1 MiB transaction limit".into(),
                    ));
                }
                let buffer_version = tools
                    .mutation_buffer_version(&path)
                    .map_err(|error| AgentError::Protocol(error.to_string()))?;
                Ok(WorkspaceFileSnapshot {
                    path: path.to_string_lossy().into_owned(),
                    sha256: format!("{:x}", Sha256::digest(text.as_bytes())),
                    text,
                    buffer_version,
                })
            })
            .await
            .map_err(|error| AgentError::Protocol(error.to_string()))?
        })
    }

    fn replace(
        &self,
        expected: WorkspaceFileSnapshot,
        text: String,
    ) -> teal_agent::ProviderFuture<Result<WorkspaceEditReceipt, AgentError>> {
        let tools = Arc::clone(&self.tools);
        Box::pin(async move {
            if text.len() > 1024 * 1024 {
                return Err(AgentError::Protocol(
                    "workspace text exceeds the 1 MiB transaction limit".into(),
                ));
            }
            let (response, received) = oneshot::channel();
            tools
                .mutations
                .send(MutationRequest {
                    observation: crate::tracing_context::Context::current(),
                    path: std::path::PathBuf::from(&expected.path),
                    kind: MutationKind::Write(text.clone()),
                    expected_buffer_version: expected.buffer_version.clone(),
                    expected_content_sha256: Some(expected.sha256.clone()),
                    response,
                })
                .await
                .map_err(|_| AgentError::Protocol("workspace mutation channel is closed".into()))?;
            received
                .await
                .map_err(|_| {
                    AgentError::Protocol("workspace mutation result is unavailable".into())
                })?
                .map_err(AgentError::Protocol)?;
            Ok(WorkspaceEditReceipt {
                path: expected.path,
                before_sha256: expected.sha256,
                after_sha256: format!("{:x}", Sha256::digest(text.as_bytes())),
            })
        })
    }
}
