//! 独立 Teal 审核 Agent 的宿主驱动；模型结论不执行知识写入。

use std::{path::Path, sync::Arc};
use sha2::{Digest, Sha256};

use teal_agent::{
    AgentRuntimeError, RootBudgetLimits, RuntimeServices, RuntimeStore, UserAnswer, UserQuestion,
    ModelToolState, WorkspaceTransactionProvider, PermissionProvider, ActionContext, ActionSpec,
    ActionResourceAccess, ResourceVersion, Decision,
};
use teal_core::ExecutionGrant;
use teal_knowledge::{
    KnowledgeProposal, KnowledgeProposalStore, KnowledgeStore, ProposalReview, ProposalStatus,
    PublicationReceipt,
};
use teal_model::{
    ModelAccess, ModelConfiguration, ModelMessage, ModelSelector, ProfileName,
    TomlModelConfiguration,
};

/// 审核包、模型配置或独立 Agent 执行失败。
#[derive(Debug, thiserror::Error)]
pub enum KnowledgeReviewError {
    /// 审核包身份、大小或模型结果无效。
    #[error("Knowledge review input or result is invalid: {0}")]
    Invalid(String),
    /// 来源存在但无法形成可安全披露的有界审核包。
    #[error("Knowledge review needs more evidence: {0}")]
    NeedsEvidence(String),
    /// 用户层审核模型尚未配置或不可用。
    #[error("Knowledge review model is unavailable: {0}")]
    Model(String),
    /// 审核运行状态未知，不能自动重试。
    #[error("Knowledge review outcome is unknown: {0}")]
    Unknown(String),
}

/// 使用仅从用户配置解析的 `knowledge-review` 预设运行内置审核 Agent。
///
/// `packet` 必须由宿主从可复核来源构造；函数只验证身份和上限。审核 Agent
/// 没有文件、知识、记忆或外部工具，返回的结论仍须由发布服务复核。
///
/// # Errors
/// 配置缺失、模型执行失败、审核包或结构化事件不可信时返回错误。
pub async fn review_knowledge_proposal(
    proposals: &KnowledgeProposalStore,
    workspace: &Path,
    proposal: &KnowledgeProposal,
    packet: &str,
    audit: teal_security::AuditContext,
) -> Result<ProposalReview, KnowledgeReviewError> {
    if packet.len() > 16 * 1024 {
        return Err(KnowledgeReviewError::Invalid(
            "review packet exceeds 16 KiB".into(),
        ));
    }
    let parsed: serde_json::Value = serde_json::from_str(packet)
        .map_err(|error| KnowledgeReviewError::Invalid(error.to_string()))?;
    if parsed["proposal_id"] != proposal.id || parsed["source_digest"] != proposal.source_digest {
        return Err(KnowledgeReviewError::Invalid(
            "review packet identity differs from the proposal".into(),
        ));
    }
    let configuration = TomlModelConfiguration::discover(None)
        .map_err(|error| KnowledgeReviewError::Model(error.to_string()))?;
    let access = ModelAccess::open(Arc::new(ModelConfiguration::toml(configuration)))
        .map_err(|error| KnowledgeReviewError::Model(error.to_string()))?;
    let profile = ProfileName::new("knowledge-review")
        .map_err(|error| KnowledgeReviewError::Model(error.to_string()))?;
    access
        .configuration()
        .map_err(|error| KnowledgeReviewError::Model(error.to_string()))?
        .document
        .resolve(&ModelSelector::Profile {
            profile,
            model: None,
        })
        .map_err(|error| KnowledgeReviewError::Model(error.to_string()))?;

    let compiled = teal::compile_source(
        teal::Source::new(
            "runtime/agents/knowledge-review.tl",
            include_str!("../../../runtime/agents/knowledge-review.tl"),
        ),
        &teal::FsSourceLoader,
        teal::CompileOptions::default().with_runtime_profile(teal::RuntimeProfile::agent()),
    )
    .map_err(|error| KnowledgeReviewError::Invalid(error.to_string()))?;
    let store = Arc::new(
        RuntimeStore::open(workspace)
            .map_err(|error| KnowledgeReviewError::Invalid(error.to_string()))?,
    );
    let context_digest = format!("{:x}", Sha256::digest(packet.as_bytes()));
    let prepared = teal::prepare_execution(
        &compiled,
        store.as_ref(),
        &format!("knowledge-review:{}", proposal.id),
        &context_digest,
        teal::ExecutionBindings::default(),
    )
    .map_err(|error| KnowledgeReviewError::Invalid(error.to_string()))?;
    let run_id = prepared.run_id.clone();
    proposals
        .attach_review_run(&proposal.id, &run_id)
        .map_err(|error| KnowledgeReviewError::Unknown(error.to_string()))?;
    let services = RuntimeServices::new()
        .with_model_access(access.clone())
        .with_root_budget(RootBudgetLimits {
            model_rounds: 1,
            tool_calls: 1,
            input_tokens: 16_384,
            output_tokens: 1_024,
        })
        .map_err(|error| KnowledgeReviewError::Invalid(error.to_string()))?
        .with_audit_context(audit);
    let grant = Arc::new(ExecutionGrant::core_only("knowledge-review/v1"));
    let mut host = teal::NativeHostRuntime::builder()
        .trusted()
        .trusted()
        .trusted()
        .trusted()
        .authorization(grant.clone())
        .console(false)
        .file_system(false)
        .build()
        .map_err(|error| KnowledgeReviewError::Invalid(error.to_string()))?;
    let mut execution = prepared
        .bind_with_access(
            &mut host,
            access,
            Arc::clone(&store),
            Arc::clone(&grant),
            services,
        )
        .map_err(|error| KnowledgeReviewError::Invalid(error.to_string()))?;
    let first = execution.execute_step(&mut host).await;
    let Err(AgentRuntimeError::NeedsInteraction(request)) = first else {
        return Err(KnowledgeReviewError::Unknown(
            "reviewer did not request its one input".into(),
        ));
    };
    if !matches!(request.question, UserQuestion::AgentInput { .. }) {
        return Err(KnowledgeReviewError::Unknown(
            "reviewer requested an unexpected interaction".into(),
        ));
    }
    store
        .answer_interaction(
            &run_id,
            &request.id,
            request.context.generation,
            UserAnswer::AgentInput {
                fields: std::collections::BTreeMap::from([
                    ("kind".into(), "text".into()),
                    ("value".into(), packet.to_owned()),
                ]),
            },
        )
        .map_err(|error| KnowledgeReviewError::Unknown(error.to_string()))?;
    let second = execution.execute_step(&mut host).await;
    second.map_err(|error| KnowledgeReviewError::Unknown(error.to_string()))?;
    let events = store
        .agent_events(&run_id, 0, 32)
        .map_err(|error| KnowledgeReviewError::Unknown(error.to_string()))?;
    parse_verdict(&events, proposal)
}

/// 从当前 RuntimeStore 的已交付工具事实生成有界审核包。
///
/// 只接受同一运行中可回查的工具调用引用；未交付结果、秘密样式文本和超长结果失败关闭。
///
/// # Errors
/// 来源调用或证据缺失、身份变化或结果不宜披露时返回错误。
pub fn build_knowledge_review_packet(
    store: &RuntimeStore,
    proposal: &KnowledgeProposal,
    knowledge: Option<&dyn teal_agent::KnowledgeProvider>,
) -> Result<String, KnowledgeReviewError> {
    if looks_sensitive(&proposal.title) || looks_sensitive(&proposal.body) {
        return Err(KnowledgeReviewError::NeedsEvidence(
            "proposal contains sensitive text and must be revised locally".into(),
        ));
    }
    let operations = store
        .model_tool_operations(&proposal.source_run)
        .map_err(|error| KnowledgeReviewError::Invalid(error.to_string()))?;
    let proposal_operation = operations
        .iter()
        .find(|operation| {
            let Ok(raw) = serde_json::to_string(&operation.call) else {
                return false;
            };
            teal_agent::ReconcileRequest::from_model_tool(&operation.id, &proposal.source_run, &raw)
                .idempotency_key
                == proposal.source_event
        })
        .ok_or_else(|| {
            KnowledgeReviewError::Invalid("proposal has no source tool receipt".into())
        })?;
    if proposal_operation.call.name != "knowledge.propose"
        || proposal_operation.state != ModelToolState::Delivered
        || !matches!(&proposal_operation.result,
            Some(ModelMessage::ToolResult { value, is_error: false, .. })
                if value["id"] == proposal.id)
    {
        return Err(KnowledgeReviewError::Invalid(
            "proposal tool was not delivered successfully".into(),
        ));
    }
    let expected_digest = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(&proposal.source_event, &proposal_operation.call.arguments))
                .map_err(|error| KnowledgeReviewError::Invalid(error.to_string()))?
        )
    );
    if expected_digest != proposal.source_digest {
        return Err(KnowledgeReviewError::Invalid(
            "proposal source digest changed".into(),
        ));
    }
    let mut references = Vec::new();
    for reference in &proposal.source_refs {
        let operation = operations
            .iter()
            .find(|operation| {
                operation.call.id == *reference && operation.id != proposal_operation.id
            })
            .ok_or_else(|| {
                KnowledgeReviewError::Invalid("proposal references an unknown tool call".into())
            })?;
        if operation.state != ModelToolState::Delivered {
            return Err(KnowledgeReviewError::Invalid(
                "proposal references an unfinished tool call".into(),
            ));
        }
        let Some(ModelMessage::ToolResult {
            value,
            is_error: false,
            ..
        }) = &operation.result
        else {
            return Err(KnowledgeReviewError::Invalid(
                "proposal references a failed tool call".into(),
            ));
        };
        let content = serde_json::to_string(value)
            .map_err(|error| KnowledgeReviewError::Invalid(error.to_string()))?;
        if content.len() > 2048 || looks_sensitive(&content) {
            return Err(KnowledgeReviewError::NeedsEvidence(
                "source evidence needs a bounded safe excerpt".into(),
            ));
        }
        references.push(serde_json::json!({
            "id": reference, "tool": operation.call.name, "result": value,
        }));
    }
    let similar = if let Some(knowledge) = knowledge {
        knowledge
            .search(
                &proposal.title,
                Some(teal_knowledge::KnowledgeScope::Project),
                3,
            )
            .map_err(|error| {
                KnowledgeReviewError::NeedsEvidence(format!(
                    "similar knowledge search unavailable: {error}"
                ))
            })?
            .into_iter()
            .map(|hit| {
                serde_json::json!({
                    "id": hit.id, "title": hit.title, "digest": hit.digest,
                    "snippet": hit.snippet,
                })
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let packet = serde_json::to_string(&serde_json::json!({
        "proposal_id": proposal.id,
        "source_digest": proposal.source_digest,
        "title": proposal.title,
        "body": proposal.body,
        "source_refs": references,
        "similar": similar,
    }))
    .map_err(|error| KnowledgeReviewError::Invalid(error.to_string()))?;
    if packet.len() > 16 * 1024 {
        return Err(KnowledgeReviewError::Invalid(
            "review packet exceeds 16 KiB".into(),
        ));
    }
    Ok(packet)
}

fn looks_sensitive(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "api_key",
        "apikey",
        "password",
        "access_token",
        "client_secret",
        "private key",
        "authorization:",
        "bearer ",
        "sk-",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

/// 受工作台共享事务或 CLI 原子文档写入边界约束的发布端口。
pub enum KnowledgePublicationWriter<'a> {
    /// 工作台共享编辑事务；目标必须不存在。
    Workspace(&'a dyn WorkspaceTransactionProvider),
    /// CLI 直接写入当前已验证的工作区知识层。
    Headless(&'a KnowledgeStore),
}

/// 对一份草稿执行审核，并在确定通过后交给宿主发布端口。
///
/// 调用方仅在来源调用到达安全边界时触发。模型配置缺失时保持待审；
/// 来源、模型或发布状态未知时写回相应状态并停止自动重试。
///
/// # Errors
/// 来源、持久状态、审核或发布失败时返回错误；返回前尽量保存可恢复状态。
#[allow(
    clippy::too_many_arguments,
    reason = "Review binds the proposal, source, authorization, writer, and audit independently"
)]
pub async fn process_knowledge_proposal(
    proposals: &KnowledgeProposalStore,
    id: &str,
    workspace: &Path,
    knowledge_root: &Path,
    knowledge: Option<&dyn teal_agent::KnowledgeProvider>,
    permissions: &dyn PermissionProvider,
    writer: KnowledgePublicationWriter<'_>,
    audit: teal_security::AuditContext,
) -> Result<ProposalStatus, String> {
    let mut proposal = proposals
        .get(id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "knowledge proposal is missing".to_owned())?;
    if proposal.status == ProposalStatus::Unknown
        && matches!(proposal.review, Some(ProposalReview::Approve { .. }))
    {
        let source = RuntimeStore::open(workspace).map_err(|error| error.to_string())?;
        build_knowledge_review_packet(&source, &proposal, knowledge)
            .map_err(|error| error.to_string())?;
        let slug = format!("agent-{}", &proposal.id[..16]);
        let target = knowledge_root.join(format!("{slug}.md"));
        let expected = format!(
            "# {}\n\n{}\n",
            proposal.title.trim(),
            proposal.body.trim_end()
        );
        let Ok(bytes) = std::fs::read(&target) else {
            return Ok(ProposalStatus::Unknown);
        };
        if bytes != expected.as_bytes() {
            return Ok(ProposalStatus::Unknown);
        }
        let receipt = PublicationReceipt {
            path: target
                .strip_prefix(workspace)
                .map_err(|_| "knowledge target escapes workspace".to_owned())?
                .to_string_lossy()
                .replace('\\', "/"),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        proposals
            .reconcile_publication(id, receipt)
            .map_err(|error| error.to_string())?;
        return Ok(ProposalStatus::Published);
    }
    if matches!(
        proposal.status,
        ProposalStatus::Reviewing | ProposalStatus::Unknown
    ) && proposal.review.is_none()
    {
        let Some(review_run) = proposal.review_run.as_deref() else {
            if proposal.status == ProposalStatus::Reviewing {
                proposals
                    .defer_review(id)
                    .map_err(|error| error.to_string())?;
                return Ok(ProposalStatus::Pending);
            }
            return Ok(proposal.status);
        };
        let store = RuntimeStore::open(workspace).map_err(|error| error.to_string())?;
        let run = store
            .run(review_run)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "review run is missing".to_owned())?;
        if run.state != teal_agent::RunState::Completed {
            return Ok(proposal.status);
        }
        let events = store
            .agent_events(review_run, 0, 32)
            .map_err(|error| error.to_string())?;
        let review = parse_verdict(&events, &proposal).map_err(|error| error.to_string())?;
        if proposal.status == ProposalStatus::Reviewing {
            proposals
                .record_review(id, review)
                .map_err(|error| error.to_string())?;
        } else {
            proposals
                .reconcile_review(id, review)
                .map_err(|error| error.to_string())?;
        }
        proposal = proposals
            .get(id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                "knowledge proposal disappeared after review reconciliation".to_owned()
            })?;
    }
    if proposal.status == ProposalStatus::Pending {
        let store = RuntimeStore::open(workspace).map_err(|error| error.to_string())?;
        let packet = match build_knowledge_review_packet(&store, &proposal, knowledge) {
            Ok(packet) => packet,
            Err(KnowledgeReviewError::NeedsEvidence(reason)) => {
                proposals
                    .mark_needs_evidence(id, &reason)
                    .map_err(|error| error.to_string())?;
                return Ok(ProposalStatus::NeedsEvidence);
            }
            Err(error) => {
                proposals
                    .mark_stale(id)
                    .map_err(|error| error.to_string())?;
                return Err(error.to_string());
            }
        };
        if !proposals
            .claim_review(id)
            .map_err(|error| error.to_string())?
        {
            return Err("knowledge proposal was claimed by another reviewer".into());
        }
        let review = match review_knowledge_proposal(
            proposals,
            workspace,
            &proposal,
            &packet,
            audit.clone(),
        )
        .await
        {
            Ok(review) => review,
            Err(KnowledgeReviewError::Model(_)) => {
                proposals
                    .defer_review(id)
                    .map_err(|error| error.to_string())?;
                return Ok(ProposalStatus::Pending);
            }
            Err(KnowledgeReviewError::Invalid(error)) => {
                proposals
                    .mark_stale(id)
                    .map_err(|error| error.to_string())?;
                return Err(error);
            }
            Err(KnowledgeReviewError::NeedsEvidence(reason)) => {
                proposals
                    .mark_unknown(id)
                    .map_err(|error| error.to_string())?;
                return Err(reason);
            }
            Err(error @ KnowledgeReviewError::Unknown(_)) => {
                proposals
                    .mark_unknown(id)
                    .map_err(|error| error.to_string())?;
                return Err(error.to_string());
            }
        };
        if let Err(error) = proposals.record_review(id, review) {
            proposals
                .mark_unknown(id)
                .map_err(|error| error.to_string())?;
            return Err(error.to_string());
        }
        proposal = proposals
            .get(id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "knowledge proposal disappeared after review".to_owned())?;
    }
    if !matches!(
        proposal.status,
        ProposalStatus::Approved | ProposalStatus::WaitingApproval
    ) {
        return Ok(proposal.status);
    }
    let store = RuntimeStore::open(workspace).map_err(|error| error.to_string())?;
    if let Err(error) = build_knowledge_review_packet(&store, &proposal, knowledge) {
        proposals
            .mark_stale(id)
            .map_err(|error| error.to_string())?;
        return Err(error.to_string());
    }
    match publish_approved_knowledge(
        proposals,
        &proposal,
        workspace,
        knowledge_root,
        permissions,
        writer,
        audit,
    )
    .await
    {
        Ok(_) => Ok(ProposalStatus::Published),
        Err(KnowledgePublicationError::ApprovalRequired) => {
            if proposal.status == ProposalStatus::Approved {
                proposals
                    .wait_for_approval(id)
                    .map_err(|error| error.to_string())?;
            }
            Ok(ProposalStatus::WaitingApproval)
        }
        Err(error @ KnowledgePublicationError::Stale(_)) => {
            proposals
                .mark_stale(id)
                .map_err(|error| error.to_string())?;
            Err(error.to_string())
        }
        Err(error @ KnowledgePublicationError::Unknown(_)) => {
            proposals
                .mark_unknown(id)
                .map_err(|error| error.to_string())?;
            Err(error.to_string())
        }
    }
}

/// 发布前核对、授权或写入失败。
#[derive(Debug, thiserror::Error)]
pub enum KnowledgePublicationError {
    /// 工作区写入还需要用户配置的权限决定。
    #[error("Knowledge publication requires workspace write approval")]
    ApprovalRequired,
    /// 草稿来源或目标已变化。
    #[error("Knowledge publication is stale: {0}")]
    Stale(String),
    /// 写入或结果审计状态未知，不能自动重试。
    #[error("Knowledge publication effect is unknown: {0}")]
    Unknown(String),
}

/// 对已通过独立审核的原草稿执行宿主发布；模型没有写入端口。
///
/// 同一草稿生成固定的新文件名。已有内容与预期摘要完全相同时视为幂等恢复，
/// 不一致时拒绝覆盖。调用方需在本函数前后核对来源收据并刷新知识层。
///
/// # Errors
/// 状态、工作区授权、路径、审计或写入不满足契约时返回错误。
pub async fn publish_approved_knowledge(
    proposals: &KnowledgeProposalStore,
    proposal: &KnowledgeProposal,
    workspace: &Path,
    knowledge_root: &Path,
    permissions: &dyn PermissionProvider,
    writer: KnowledgePublicationWriter<'_>,
    audit: teal_security::AuditContext,
) -> Result<PublicationReceipt, KnowledgePublicationError> {
    if !matches!(
        proposal.status,
        ProposalStatus::Approved | ProposalStatus::WaitingApproval
    ) {
        return Err(KnowledgePublicationError::Stale(
            "proposal is not approved".into(),
        ));
    }
    if !matches!(proposal.review, Some(ProposalReview::Approve { .. })) {
        return Err(KnowledgePublicationError::Stale(
            "approved verdict is missing".into(),
        ));
    }
    let current = proposals
        .get(&proposal.id)
        .map_err(|error| KnowledgePublicationError::Unknown(error.to_string()))?
        .ok_or_else(|| KnowledgePublicationError::Stale("proposal is missing".into()))?;
    if &current != proposal {
        return Err(KnowledgePublicationError::Stale(
            "proposal changed after review".into(),
        ));
    }
    let relative_root = knowledge_root
        .strip_prefix(workspace)
        .map_err(|_| KnowledgePublicationError::Stale("knowledge root escapes workspace".into()))?;
    if relative_root
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(KnowledgePublicationError::Stale(
            "knowledge root has invalid components".into(),
        ));
    }
    let workspace = workspace
        .canonicalize()
        .map_err(|error| KnowledgePublicationError::Stale(error.to_string()))?;
    let canonical_root = workspace.join(relative_root);
    let mut checked = workspace.clone();
    for component in relative_root.components() {
        checked.push(component.as_os_str());
        if std::fs::symlink_metadata(&checked)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(KnowledgePublicationError::Stale(
                "knowledge root contains a symbolic link".into(),
            ));
        }
    }
    let slug = format!("agent-{}", &proposal.id[..16]);
    let target = canonical_root.join(format!("{slug}.md"));
    let text = format!(
        "# {}\n\n{}\n",
        proposal.title.trim(),
        proposal.body.trim_end()
    );
    if looks_sensitive(&text) {
        return Err(KnowledgePublicationError::Stale(
            "proposal contains sensitive text".into(),
        ));
    }
    let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
    let receipt = PublicationReceipt {
        path: target
            .strip_prefix(&workspace)
            .map_err(|_| {
                KnowledgePublicationError::Stale("publication path escapes workspace".into())
            })?
            .to_string_lossy()
            .replace('\\', "/"),
        sha256: digest.clone(),
    };
    if let KnowledgePublicationWriter::Headless(store) = &writer
        && store.root() != knowledge_root
    {
        return Err(KnowledgePublicationError::Stale(
            "headless knowledge root changed".into(),
        ));
    }
    let action = ActionSpec {
        id: format!("knowledge-publish:{}", proposal.id),
        tool: "workspace.write_file".into(),
        arguments: serde_json::json!({"path": target}),
        resources: vec![ResourceVersion {
            access: ActionResourceAccess::Write,
            resource: format!("file://{}", target.display()),
            version: "absent".into(),
        }],
    };
    let context = ActionContext {
        run_id: format!("knowledge-review-{}", &proposal.id[..16]),
        task_id: proposal.id.clone(),
        generation: 1,
        policy_version: permissions.version().to_owned(),
    };
    match permissions
        .authorize(context, action)
        .await
        .map_err(|error| KnowledgePublicationError::Unknown(error.to_string()))?
    {
        Decision::Allow => {}
        Decision::ApprovalRequired(_) => return Err(KnowledgePublicationError::ApprovalRequired),
        Decision::Deny(reason) => return Err(KnowledgePublicationError::Stale(reason)),
    }
    let existing = match std::fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(KnowledgePublicationError::Stale(
                "publication target is a symbolic link".into(),
            ));
        }
        Ok(_) => Some(
            std::fs::read(&target)
                .map_err(|error| KnowledgePublicationError::Unknown(error.to_string()))?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(KnowledgePublicationError::Unknown(error.to_string())),
    };
    let event = teal_security::Event::new(
        teal_security::Action::KnowledgeMutation,
        teal_security::Phase::Intent,
        teal_security::Outcome::Pending,
        teal_security::new_correlation_id(),
    )
    .map_err(|error| KnowledgePublicationError::Unknown(error.to_string()))?
    .with_actor(teal_security::Actor::Host);
    let audit = audit
        .begin(event)
        .map_err(|error| KnowledgePublicationError::Unknown(error.to_string()))?;
    let result = if let Some(existing) = existing {
        if format!("{:x}", Sha256::digest(&existing)) == digest {
            Ok(())
        } else {
            Err(KnowledgePublicationError::Stale(
                "target file already exists with different content".into(),
            ))
        }
    } else {
        match writer {
            KnowledgePublicationWriter::Workspace(transaction) => transaction
                .create_new(receipt.path.clone(), text)
                .await
                .map_err(|error| KnowledgePublicationError::Unknown(error.to_string()))
                .and_then(|written| {
                    if written.after_sha256 == digest && Path::new(&written.path) == target {
                        Ok(())
                    } else {
                        Err(KnowledgePublicationError::Unknown(
                            "workspace receipt digest differs from written content".into(),
                        ))
                    }
                }),
            KnowledgePublicationWriter::Headless(store) => store
                .create_named(&slug, text.as_bytes())
                .map(|_| ())
                .map_err(|error| match error {
                    teal_knowledge::KnowledgeError::Io(io)
                        if io.kind() == std::io::ErrorKind::AlreadyExists =>
                    {
                        KnowledgePublicationError::Stale(
                            "publication target was created concurrently".into(),
                        )
                    }
                    other => KnowledgePublicationError::Unknown(other.to_string()),
                }),
        }
    };
    audit
        .finish(if result.is_ok() {
            teal_security::Outcome::Succeeded
        } else {
            teal_security::Outcome::Failed
        })
        .map_err(|error| KnowledgePublicationError::Unknown(error.to_string()))?;
    result?;
    let recorded = proposals
        .record_publication(&proposal.id, receipt.clone())
        .map_err(|error| KnowledgePublicationError::Unknown(error.to_string()))?;
    if !recorded
        && proposals
            .get(&proposal.id)
            .map_err(|error| KnowledgePublicationError::Unknown(error.to_string()))?
            .is_none_or(|saved| {
                saved.status != ProposalStatus::Published || saved.receipt != Some(receipt.clone())
            })
    {
        return Err(KnowledgePublicationError::Unknown(
            "publication state changed concurrently".into(),
        ));
    }
    Ok(receipt)
}

fn parse_verdict(
    events: &[teal_agent::AgentLogEvent],
    proposal: &KnowledgeProposal,
) -> Result<ProposalReview, KnowledgeReviewError> {
    let mut fields = std::collections::BTreeMap::new();
    let mut cited_refs = Vec::new();
    for event in events.iter().filter(|event| event.kind == "application") {
        let Some(kind) = event.payload["kind"].as_str() else {
            continue;
        };
        let Some(value) = event.payload["payload"].as_str() else {
            continue;
        };
        if kind == "cited_ref" {
            cited_refs.push(value.to_owned());
        } else if fields.insert(kind.to_owned(), value.to_owned()).is_some() {
            return Err(KnowledgeReviewError::Invalid(
                "review event was repeated".into(),
            ));
        }
    }
    if fields.get("proposal_id") != Some(&proposal.id)
        || fields.get("source_digest") != Some(&proposal.source_digest)
    {
        return Err(KnowledgeReviewError::Invalid(
            "review result identity differs from the proposal".into(),
        ));
    }
    match fields.get("decision").map(String::as_str) {
        Some("approve")
            if !cited_refs.is_empty()
                && cited_refs
                    .iter()
                    .all(|item| proposal.source_refs.contains(item)) =>
        {
            Ok(ProposalReview::Approve { cited_refs })
        }
        Some("reject") => Ok(ProposalReview::Reject {
            reason: fields.get("reason").cloned().unwrap_or_default(),
        }),
        Some("needs_evidence") => Ok(ProposalReview::NeedsEvidence {
            reason: fields.get("reason").cloned().unwrap_or_default(),
        }),
        _ => Err(KnowledgeReviewError::Invalid(
            "review decision is missing or unsupported".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use teal_knowledge::{KnowledgeProposalInput, KnowledgeScope, LayerOptions};

    struct TestAudit(AtomicU64);

    impl teal_security::AuditSink for TestAudit {
        fn append(
            &self,
            _: &teal_security::Event,
        ) -> Result<teal_security::Receipt, teal_security::AuditError> {
            let sequence = self.0.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(teal_security::Receipt {
                sequence,
                hash: format!("{sequence:064x}"),
            })
        }
    }

    fn audit() -> teal_security::AuditContext {
        teal_security::AuditContext::new(Arc::new(TestAudit(AtomicU64::new(0))))
    }

    #[tokio::test]
    async fn approved_proposal_publishes_once_and_never_overwrites_a_conflict() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path();
        let root = workspace.join(".teal/knowledge");
        let proposals =
            KnowledgeProposalStore::open_with_audit(&workspace.join("proposals.db"), audit())
                .unwrap();
        let created = proposals
            .propose(KnowledgeProposalInput {
                workspace: "a".repeat(64),
                title: "Stable fact".into(),
                body: "Verified source fact".into(),
                source_run: "run".into(),
                source_event: "event".into(),
                source_digest: "b".repeat(64),
                source_refs: vec!["tool-1".into()],
            })
            .unwrap();
        assert!(proposals.claim_review(&created.id).unwrap());
        assert!(
            proposals
                .record_review(
                    &created.id,
                    ProposalReview::Approve {
                        cited_refs: vec!["tool-1".into()],
                    }
                )
                .unwrap()
        );
        let approved = proposals.get(&created.id).unwrap().unwrap();
        let knowledge =
            KnowledgeStore::open(&root, KnowledgeScope::Project, LayerOptions::default())
                .unwrap()
                .with_audit_context(audit());
        let receipt = publish_approved_knowledge(
            &proposals,
            &approved,
            workspace,
            &root,
            &teal_agent::AllowAllPermissions,
            KnowledgePublicationWriter::Headless(&knowledge),
            audit(),
        )
        .await
        .unwrap();
        assert_eq!(
            proposals.get(&created.id).unwrap().unwrap().status,
            ProposalStatus::Published
        );
        let published = workspace.join(&receipt.path);
        assert!(published.is_file());
        let before = std::fs::read(&published).unwrap();
        assert!(
            knowledge
                .create_named(&format!("agent-{}", &created.id[..16]), b"different")
                .is_err()
        );
        assert_eq!(std::fs::read(&published).unwrap(), before);
    }

    #[tokio::test]
    async fn fabricated_source_receipt_never_reaches_the_reviewer_or_publisher() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path();
        let root = workspace.join(".teal/knowledge");
        let proposals =
            KnowledgeProposalStore::open_with_audit(&workspace.join("proposals.db"), audit())
                .unwrap();
        let created = proposals
            .propose(KnowledgeProposalInput {
                workspace: "a".repeat(64),
                title: "Claim".into(),
                body: "Unproven fact".into(),
                source_run: "invented-run".into(),
                source_event: "invented-event".into(),
                source_digest: "b".repeat(64),
                source_refs: vec!["invented-call".into()],
            })
            .unwrap();
        let knowledge =
            KnowledgeStore::open(&root, KnowledgeScope::Project, LayerOptions::default())
                .unwrap()
                .with_audit_context(audit());
        assert!(
            process_knowledge_proposal(
                &proposals,
                &created.id,
                workspace,
                &root,
                None,
                &teal_agent::AllowAllPermissions,
                KnowledgePublicationWriter::Headless(&knowledge),
                audit(),
            )
            .await
            .is_err()
        );
        assert_eq!(
            proposals.get(&created.id).unwrap().unwrap().status,
            ProposalStatus::Stale
        );
        assert!(!root.exists());
    }

    #[tokio::test]
    async fn sensitive_proposal_is_held_before_model_disclosure() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path();
        let root = workspace.join(".teal/knowledge");
        let proposals =
            KnowledgeProposalStore::open_with_audit(&workspace.join("proposals.db"), audit())
                .unwrap();
        let created = proposals
            .propose(KnowledgeProposalInput {
                workspace: "a".repeat(64),
                title: "Claim".into(),
                body: "api_key=secret-value".into(),
                source_run: "run".into(),
                source_event: "event".into(),
                source_digest: "b".repeat(64),
                source_refs: vec!["call".into()],
            })
            .unwrap();
        let knowledge =
            KnowledgeStore::open(&root, KnowledgeScope::Project, LayerOptions::default())
                .unwrap()
                .with_audit_context(audit());
        let status = process_knowledge_proposal(
            &proposals,
            &created.id,
            workspace,
            &root,
            None,
            &teal_agent::AllowAllPermissions,
            KnowledgePublicationWriter::Headless(&knowledge),
            audit(),
        )
        .await
        .unwrap();
        assert_eq!(status, ProposalStatus::NeedsEvidence);
        assert!(
            proposals
                .get(&created.id)
                .unwrap()
                .unwrap()
                .review_run
                .is_none()
        );
        assert!(!root.exists());
    }
}
