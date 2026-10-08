use std::sync::Arc;
use teal_agent::{
    ActionContext, ActionSpec, ApprovalResponse, Decision, PermissionEvaluation,
    PermissionProvider, PolicyError, ProviderFuture, ToolCall, WorkspacePermissionPolicy,
};
use tokio::sync::{mpsc, oneshot};
use sha2::{Digest as _, Sha256};
use super::ApprovalPrompt;
/// 仅阻塞智能体任务并保持界面响应的批准处理器。
pub struct ChannelApproval {
    sender: mpsc::Sender<ApprovalPrompt>,
    policy: Arc<WorkspacePermissionPolicy>,
    decision_lock: Arc<tokio::sync::Mutex<()>>,
}

impl ChannelApproval {
    /// 基于应用批准队列创建处理器。
    pub fn new(sender: mpsc::Sender<ApprovalPrompt>) -> Self {
        Self {
            sender,
            policy: Arc::new(WorkspacePermissionPolicy::ephemeral(
                std::env::current_dir().unwrap_or_default(),
            )),
            decision_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// 创建读取用户层工作区授权并可持久保存范围决定的处理器。
    pub fn for_workspace(
        sender: mpsc::Sender<ApprovalPrompt>,
        paths: teal_config::ConfigPaths,
    ) -> Self {
        Self {
            sender,
            policy: Arc::new(WorkspacePermissionPolicy::new(paths)),
            decision_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }
}

impl PermissionProvider for ChannelApproval {
    fn version(&self) -> &str {
        "teal-bench/channel-approval/v3"
    }

    fn authorize(
        &self,
        context: ActionContext,
        action: ActionSpec,
    ) -> ProviderFuture<Result<Decision, PolicyError>> {
        let prompts = self.sender.clone();
        let policy = Arc::clone(&self.policy);
        let decision_lock = Arc::clone(&self.decision_lock);
        Box::pin(async move {
            let event = teal_security::Event::new(
                teal_security::Action::Approval,
                teal_security::Phase::Intent,
                teal_security::Outcome::Pending,
                format!("{:x}", Sha256::digest(action.id.as_bytes())),
            )
            .map_err(|error| PolicyError::Provider(error.to_string()))?
            .with_actor(teal_security::Actor::Agent);
            let audit = teal_security::process_context()
                .begin(event)
                .map_err(|error| {
                    PolicyError::Provider(format!(
                        "Audit unavailable; approval was not processed: {error}"
                    ))
                })?;
            let result = async {
                if action.tool.starts_with("knowledge.") && action.resources.is_empty() {
                    return Ok(Decision::Allow);
                }
                let initial = policy.evaluate(context.clone(), action.clone())?;
                if matches!(initial, PermissionEvaluation::Allow) {
                    return Ok(Decision::Allow);
                }
                if let PermissionEvaluation::Deny(reason) = initial {
                    return Ok(Decision::Deny(reason));
                }
                // 仅串行化需要交互的决定；等待前重新判断可合并并发的相同范围请求。
                let _guard = decision_lock.lock().await;
                let permission = match policy.evaluate(context.clone(), action.clone())? {
                    PermissionEvaluation::Allow => return Ok(Decision::Allow),
                    PermissionEvaluation::Deny(reason) => return Ok(Decision::Deny(reason)),
                    PermissionEvaluation::Prompt(prompt) => prompt,
                };
                let (response, received) = oneshot::channel();
                let call = ToolCall {
                    id: action.id.clone(),
                    name: action.tool.clone(),
                    arguments: action.arguments.clone(),
                };
                prompts
                    .send(ApprovalPrompt {
                        observation: crate::tracing_context::Context::current(),
                        call,
                        permission: permission.clone(),
                        response,
                    })
                    .await
                    .map_err(|_| PolicyError::Provider("approval channel is closed".into()))?;
                let response = received.await.unwrap_or(ApprovalResponse::Deny);
                policy.apply(context, &action, &permission, response)
            }
            .await;
            let outcome = match &result {
                Ok(Decision::Allow) => teal_security::Outcome::Succeeded,
                Ok(Decision::Deny(_)) => teal_security::Outcome::Denied,
                Ok(Decision::ApprovalRequired(_)) => teal_security::Outcome::Deferred,
                Err(_) => teal_security::Outcome::Failed,
            };
            audit.finish(outcome).map_err(|error| {
                PolicyError::Provider(format!(
                    "Audit result unavailable; approval state may be unknown: {error}"
                ))
            })?;
            result
        })
    }
}
