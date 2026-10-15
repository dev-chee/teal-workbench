//! 交付收据的可信 Git 提交服务。

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde_json::Value;
use teal_agent::{
    AgentError, InteractionStatus, UserAnswer, VerifiedArtifact, VerifiedCommitProvider,
    VerifiedCommitRequest,
};
use teal_model::{ContentItem, ModelMessage};

/// 将源码 Agent 的已验收交付绑定到工作区、真实工具历史和隔离 Git index。
pub struct VerifiedCommitService {
    workspace: PathBuf,
    audit: teal_security::AuditContext,
}

impl VerifiedCommitService {
    /// 建立可信提交服务；提交前仍会重新读取所有路径和收据。
    ///
    /// # Errors
    /// 工作区不存在或无法规范化时返回错误。
    pub fn new(workspace: &Path, audit: teal_security::AuditContext) -> Result<Self, String> {
        Ok(Self {
            workspace: workspace
                .canonicalize()
                .map_err(|error| error.to_string())?,
            audit,
        })
    }
}

impl VerifiedCommitProvider for VerifiedCommitService {
    fn commit(
        &self,
        request: VerifiedCommitRequest,
    ) -> teal_agent::ProviderFuture<Result<String, AgentError>> {
        let workspace = self.workspace.clone();
        let audit = self.audit.clone();
        Box::pin(async move {
            let receipt = tokio::task::spawn_blocking({
                let workspace = workspace.clone();
                let request = request.clone();
                move || verify_commit_request(&workspace, &request)
            })
            .await
            .map_err(|error| AgentError::Protocol(error.to_string()))?
            .map_err(AgentError::Protocol)?;
            let event = teal_security::Event::new(
                teal_security::Action::Process,
                teal_security::Phase::Intent,
                teal_security::Outcome::Pending,
                teal_security::new_correlation_id(),
            )
            .map_err(|error| AgentError::Protocol(error.to_string()))?
            .with_actor(teal_security::Actor::Agent)
            .with_workspace(&workspace)
            .with_run_id(request.run_id.clone())
            .map_err(|error| AgentError::Protocol(error.to_string()))?;
            let attempt = audit
                .begin(event)
                .map_err(|error| AgentError::Protocol(error.to_string()))?;
            let result = execute_agent_commit(&workspace, &request.message, &receipt).await;
            match result {
                Ok(commit) => {
                    attempt.finish(teal_security::Outcome::Succeeded)
                        .map_err(|error| AgentError::Protocol(format!(
                            "commit {commit} may have succeeded but result audit failed: {error}"
                        )))?;
                    Ok(commit)
                }
                Err(error) => {
                    attempt.finish(teal_security::Outcome::Unknown)
                        .map_err(|audit_error| AgentError::Protocol(format!(
                            "commit effect may be unknown: {error}; result audit failed: {audit_error}"
                        )))?;
                    Err(AgentError::Protocol(format!(
                        "commit effect may be unknown: {error}"
                    )))
                }
            }
        })
    }
}

#[derive(Clone)]
struct ToolFact {
    name: String,
    arguments: Value,
    value: Value,
    sequence: usize,
    success: bool,
}

fn verify_commit_request(
    workspace: &Path,
    request: &VerifiedCommitRequest,
) -> Result<crate::DeliveryReceipt, String> {
    if request.message.trim().is_empty()
        || request.message.len() > 4096
        || request.summary.trim().is_empty()
        || request.summary.len() > 16384
        || request.artifacts.len() > 64
        || request.verification_ids.len() > 64
    {
        return Err("verified commit request is invalid".into());
    }
    let mut paths = BTreeSet::new();
    for artifact in std::iter::once(&request.script).chain(&request.artifacts) {
        if !safe_artifact_path(&artifact.path)
            || !paths.insert(artifact.path.clone())
            || artifact.sha256.len() != 64
            || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("verified commit artifact identity is invalid".into());
        }
    }
    if !request.script.path.ends_with(".tl") {
        return Err("delivery entry must be a Teal script".into());
    }
    let store = teal_agent::RuntimeStore::connect(workspace).map_err(|error| error.to_string())?;
    let run = store
        .run(&request.run_id)
        .map_err(|error| error.to_string())?
        .ok_or("verified commit run does not exist")?;
    if run.origin != teal_agent::RunOrigin::Program {
        return Err("verified commit run is not a source program".into());
    }
    let interaction = store
        .interactions(&request.run_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|interaction| interaction.id == request.approval_id)
        .ok_or("verified commit approval is absent")?;
    let resource = format!(
        "{}:{}:{}",
        request.script.sha256, request.program_digest, request.message
    );
    if interaction.source_key != teal_agent::approval_source_key("git.commit", &resource)
        || !matches!(
            interaction.status,
            InteractionStatus::Answered | InteractionStatus::Delivered
        )
        || !matches!(interaction.answer, Some(UserAnswer::Selected { ref id }) if id == "allow")
    {
        return Err("verified commit approval does not match this delivery".into());
    }
    let state = store
        .open_model_session_state(
            &request.run_id,
            request.agent_instance,
            &request.session_key,
        )
        .map_err(|error| error.to_string())?;
    let (facts, duplicate_ids) = collect_tool_facts(&state.history);
    let mut selected = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for id in &request.verification_ids {
        if !seen.insert(id) {
            return Err("verification call IDs are duplicated".into());
        }
        if duplicate_ids.contains(id) {
            return Err("verification call ID is ambiguous".into());
        }
        let fact = facts
            .get(id)
            .ok_or("verification call ID is not in the model history")?;
        if !matches!(
            fact.name.as_str(),
            "teal.fmt_check" | "teal.check" | "teal.test" | "teal.run"
        ) {
            return Err("verification call is not a delivery stage".into());
        }
        if selected.insert(fact.name.clone(), fact).is_some() {
            return Err("verification stage is duplicated".into());
        }
    }
    let script_path = workspace.join(&request.script.path);
    let test_count =
        crate::compile_test_names_in_workspace(workspace, &script_path, &request.tier)?.len();
    let mut required_names = vec!["teal.fmt_check", "teal.check"];
    if test_count != 0 {
        required_names.push("teal.test");
    }
    required_names.push("teal.run");
    let mut last = None;
    for name in required_names {
        let fact = selected
            .get(name)
            .ok_or_else(|| format!("{name} receipt is required"))?;
        if !fact.success
            || fact.value.get("exit_code").and_then(Value::as_i64) != Some(0)
            || last.is_some_and(|sequence| fact.sequence <= sequence)
        {
            return Err("verification stages failed or are out of order".into());
        }
        last = Some(fact.sequence);
        let path = fact
            .value
            .get("path")
            .or_else(|| fact.arguments.get("path"))
            .and_then(Value::as_str)
            .ok_or("verification path is missing")?;
        if workspace
            .join(path)
            .canonicalize()
            .map_err(|error| error.to_string())?
            != workspace
                .join(&request.script.path)
                .canonicalize()
                .map_err(|error| error.to_string())?
        {
            return Err("verification path differs from delivery script".into());
        }
    }
    let field = |fact: &ToolFact, key: &str| -> Option<String> {
        fact.value
            .get(key)
            .or_else(|| fact.arguments.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    if field(selected["teal.fmt_check"], "source_digest").as_deref()
        != Some(request.script.sha256.as_str())
    {
        return Err("format receipt source digest differs from delivery".into());
    }
    for name in ["teal.check", "teal.test", "teal.run"] {
        let Some(fact) = selected.get(name) else {
            continue;
        };
        if field(fact, "program_digest").as_deref() != Some(request.program_digest.as_str())
            || field(fact, "tier").as_deref() != Some(request.tier.as_str())
        {
            return Err("verification program or profile differs from delivery".into());
        }
    }
    let receipt = crate::DeliveryReceipt {
        summary: request.summary.clone(),
        script: to_delivery_artifact(&request.script),
        artifacts: request.artifacts.iter().map(to_delivery_artifact).collect(),
        program_digest: request.program_digest.clone(),
        tier: request.tier.clone(),
        evidence: request
            .verification_ids
            .iter()
            .map(|id| crate::DeliveryVerification {
                call_id: id.clone(),
                claim: "verified by source Agent".into(),
            })
            .collect(),
        accepted_at_ms: crate::now_ms(),
    };
    if !crate::delivery_is_current(workspace, &receipt) {
        return Err("verified delivery changed before commit".into());
    }
    Ok(receipt)
}

fn collect_tool_facts(history: &[ModelMessage]) -> (BTreeMap<String, ToolFact>, BTreeSet<String>) {
    let mut pending = BTreeMap::new();
    let mut facts = BTreeMap::new();
    let mut duplicate_ids = BTreeSet::new();
    let mut seen_call_ids = BTreeSet::new();
    let mut seen_result_ids = BTreeSet::new();
    let mut sequence = 0;
    for message in history {
        match message {
            ModelMessage::Assistant { items } => {
                for item in items {
                    if let ContentItem::ToolCall(call) = item {
                        if !seen_call_ids.insert(call.id.clone()) {
                            duplicate_ids.insert(call.id.clone());
                        }
                        if pending.contains_key(&call.id) || facts.contains_key(&call.id) {
                            duplicate_ids.insert(call.id.clone());
                        } else {
                            pending.insert(
                                call.id.clone(),
                                (call.name.clone(), call.arguments.clone()),
                            );
                        }
                    }
                }
            }
            ModelMessage::ToolResult {
                call_id,
                name,
                value,
                is_error,
                ..
            } => {
                if !seen_result_ids.insert(call_id.clone()) {
                    duplicate_ids.insert(call_id.clone());
                }
                if let Some((called_name, arguments)) = pending.remove(call_id)
                    && called_name == *name
                {
                    if facts
                        .insert(
                            call_id.clone(),
                            ToolFact {
                                name: name.clone(),
                                arguments,
                                value: value.clone(),
                                sequence,
                                success: !is_error,
                            },
                        )
                        .is_some()
                    {
                        duplicate_ids.insert(call_id.clone());
                    }
                    sequence += 1;
                }
            }
            _ => {}
        }
    }
    (facts, duplicate_ids)
}

fn safe_artifact_path(path: &str) -> bool {
    let candidate = Path::new(path);
    !path.is_empty()
        && !path.starts_with(".teal/")
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && candidate
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

fn to_delivery_artifact(artifact: &VerifiedArtifact) -> crate::DeliveryArtifact {
    crate::DeliveryArtifact {
        path: artifact.path.clone(),
        bytes: artifact.bytes,
        sha256: artifact.sha256.clone(),
    }
}

/// 返回交付收据中去重排序后的精确工作区路径。
#[must_use]
pub fn delivery_paths(delivery: &crate::DeliveryReceipt) -> Vec<String> {
    std::iter::once(&delivery.script)
        .chain(&delivery.artifacts)
        .map(|artifact| artifact.path.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// 核对收据仍与当前文件和 Git 工作树一致。
///
/// # Errors
/// 文件、程序或工作树已变化，或 Git 查询失败时返回错误。
pub async fn preflight_verified_agent_commit(
    workspace: &Path,
    delivery: &crate::DeliveryReceipt,
) -> Result<(), String> {
    if !crate::delivery_is_current(workspace, delivery) {
        return Err("Verified Task delivery changed before Git preflight".into());
    }
    preflight_agent_commit(workspace, &delivery_paths(delivery)).await
}

/// 拒绝暂存区中已有内容，以及交付集合以外的工作树变化。
///
/// # Errors
/// Git 不可用、暂存区非空或路径集合不匹配时返回错误。
pub async fn preflight_agent_commit(workspace: &Path, expected: &[String]) -> Result<(), String> {
    let cached = crate::run_program(
        &PathBuf::from("git"),
        &["diff".into(), "--cached".into(), "--name-only".into()],
        workspace,
    )
    .await
    .map_err(|error| error.to_string())?;
    if cached.status != 0 {
        return Err(cached.output);
    }
    if !cached.output.trim().is_empty() {
        return Err("Git index is not empty; commit was blocked".into());
    }
    let tracked = crate::run_program(
        &PathBuf::from("git"),
        &["diff".into(), "--name-only".into()],
        workspace,
    )
    .await
    .map_err(|error| error.to_string())?;
    let untracked = crate::run_program(
        &PathBuf::from("git"),
        &[
            "ls-files".into(),
            "--others".into(),
            "--exclude-standard".into(),
        ],
        workspace,
    )
    .await
    .map_err(|error| error.to_string())?;
    if tracked.status != 0 || untracked.status != 0 {
        return Err("Unable to inspect Git worktree changes".into());
    }
    let actual = tracked
        .output
        .lines()
        .chain(untracked.output.lines())
        .filter(|path| !path.trim().is_empty())
        .map(|path| path.trim().replace('\\', "/"))
        // Evidence、提交收据以及回退平台上的会话数据是工作台自身状态，
        // Task 完成后会先于提交预检产生，不能因此冒充未交付的工作区修改。
        // expected 不做同样过滤，使模型仍不能把内部状态声明成交付产物。
        .filter(|path| !is_agent_internal_state_path(path))
        .collect::<BTreeSet<_>>();
    let expected = expected
        .iter()
        .map(|path| path.replace('\\', "/"))
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(format!(
            "Git worktree paths do not match verified Task artifacts: actual={actual:?} expected={expected:?}"
        ));
    }
    Ok(())
}

/// 判断 Git 路径是否属于工作台自身的动态工作区状态。
fn is_agent_internal_state_path(path: &str) -> bool {
    path == ".teal/bench"
        || path.starts_with(".teal/bench/")
        || path == ".teal/runtime"
        || path.starts_with(".teal/runtime/")
}

/// 使用隔离 index 和原 HEAD 比较更新提交精确交付路径。
///
/// # Errors
/// 收据失效、并发 Git 变化或任一 Git 阶段失败时返回错误；引用更新后失败可能已有提交。
pub async fn execute_agent_commit(
    workspace: &Path,
    message: &str,
    delivery: &crate::DeliveryReceipt,
) -> Result<String, String> {
    preflight_verified_agent_commit(workspace, delivery).await?;
    let paths = delivery_paths(delivery);
    let head =
        run_git_for_commit(workspace, &["rev-parse".into(), "HEAD".into()], None, &[]).await?;
    // 隔离 index 让并发的用户暂存不能进入这次交付；之后只同步已提交的任务路径。
    let temporary = tempfile::tempdir().map_err(|error| error.to_string())?;
    let index = temporary.path().join("verified-index");
    let environment = vec![(
        "GIT_INDEX_FILE".to_owned(),
        index.as_os_str().to_os_string(),
    )];
    run_git_for_commit(
        workspace,
        &["read-tree".into(), head.clone()],
        None,
        &environment,
    )
    .await?;

    let mut expected_blobs = Vec::new();
    for artifact in std::iter::once(&delivery.script).chain(&delivery.artifacts) {
        let bytes =
            std::fs::read(workspace.join(&artifact.path)).map_err(|error| error.to_string())?;
        if bytes.len() as u64 != artifact.bytes || crate::digest(&bytes) != artifact.sha256 {
            return Err(format!("Verified Task file changed: {}", artifact.path));
        }
        // 使用已经核对的字节计算 Git blob，同时保留该路径的 clean/filter 规则。
        let object = run_git_for_commit(
            workspace,
            &[
                "hash-object".into(),
                "--stdin".into(),
                format!("--path={}", artifact.path),
            ],
            Some(&bytes),
            &environment,
        )
        .await?;
        expected_blobs.push((artifact.path.clone(), object));
    }

    let mut add = vec!["add".to_owned(), "--".to_owned()];
    add.extend(paths.iter().cloned());
    run_git_for_commit(workspace, &add, None, &environment).await?;
    let staged = run_git_for_commit(
        workspace,
        &[
            "diff".into(),
            "--cached".into(),
            "--name-only".into(),
            "-z".into(),
        ],
        None,
        &environment,
    )
    .await?;
    let staged = staged
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if staged != paths.iter().cloned().collect() {
        return Err("Verified Task paths changed while staging".into());
    }
    for (path, expected) in &expected_blobs {
        let staged = run_git_for_commit(
            workspace,
            &["rev-parse".into(), format!(":{path}")],
            None,
            &environment,
        )
        .await?;
        if &staged != expected {
            return Err(format!(
                "Staged Task file differs from its delivery receipt: {path}"
            ));
        }
    }
    if !crate::delivery_is_current(workspace, delivery) {
        return Err("Verified Task delivery changed while staging".into());
    }
    let current_head = run_git_for_commit(
        workspace,
        &["rev-parse".into(), "HEAD".into()],
        None,
        &environment,
    )
    .await?;
    if current_head != head {
        return Err("Git HEAD changed during Task commit preparation".into());
    }

    let tree = run_git_for_commit(workspace, &["write-tree".into()], None, &environment).await?;
    let commit = run_git_for_commit(
        workspace,
        &[
            "commit-tree".into(),
            tree,
            "-p".into(),
            head.clone(),
            "-m".into(),
            message.into(),
        ],
        None,
        &environment,
    )
    .await?;
    for (path, expected) in &expected_blobs {
        let actual = run_git_for_commit(
            workspace,
            &["rev-parse".into(), format!("{commit}:{path}")],
            None,
            &environment,
        )
        .await?;
        if &actual != expected {
            return Err(format!(
                "Prepared commit differs from verified Task delivery: {path}"
            ));
        }
    }
    // 引用更新以预检时的 HEAD 为比较值；另一提交先发生时不得提交旧基线的树。
    run_git_for_commit(
        workspace,
        &[
            "update-ref".into(),
            "-m".into(),
            "task-commit: verified delivery".into(),
            "HEAD".into(),
            commit.clone(),
            head,
        ],
        None,
        &environment,
    )
    .await?;
    let mut reset = vec!["reset".into(), "--mixed".into(), "HEAD".into(), "--".into()];
    reset.extend(paths);
    run_git_for_commit(workspace, &reset, None, &[])
        .await
        .map_err(|error| {
            format!("Commit {commit} was created, but index synchronization failed: {error}")
        })?;
    Ok(commit)
}

async fn run_git_for_commit(
    workspace: &Path,
    arguments: &[String],
    input: Option<&[u8]>,
    environment: &[(String, std::ffi::OsString)],
) -> Result<String, String> {
    let output = crate::run_program_with_input_and_environment(
        Path::new("git"),
        arguments,
        workspace,
        input,
        environment,
    )
    .await
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    String::from_utf8(output.stdout)
        .map(|stdout| stdout.trim().to_owned())
        .map_err(|error| format!("Git output is not UTF-8: {error}"))
}

#[cfg(test)]
mod tests {
    use std::{
        process::Command,
        sync::{Arc, Mutex},
    };

    use super::*;
    use crate::WorkspaceProgramDigest;
    use teal_agent::{ActionContext, AgentInstanceId, ModelSessionState, UserChoice, UserQuestion};
    use teal_model::ToolCall;

    #[derive(Default)]
    struct AuditSink(Mutex<Vec<teal_security::Event>>);

    impl teal_security::AuditSink for AuditSink {
        fn append(
            &self,
            event: &teal_security::Event,
        ) -> Result<teal_security::Receipt, teal_security::AuditError> {
            let mut events = self.0.lock().unwrap();
            events.push(event.clone());
            Ok(teal_security::Receipt {
                sequence: events.len() as u64,
                hash: format!("{:064x}", events.len()),
            })
        }
    }

    fn git(workspace: &Path, arguments: &[&str]) {
        let status = Command::new("git")
            .args(arguments)
            .current_dir(workspace)
            .status()
            .unwrap();
        assert!(status.success(), "git {arguments:?} failed");
    }

    #[test]
    fn duplicate_tool_call_ids_are_marked_ambiguous() {
        let call = ToolCall {
            id: "call-1".into(),
            name: "teal.check".into(),
            arguments: serde_json::json!({"path": "main.tl"}),
        };
        let history = vec![
            ModelMessage::Assistant {
                items: vec![ContentItem::ToolCall(call.clone())],
            },
            ModelMessage::ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                value: serde_json::json!({"exit_code": 0}),
                is_error: false,
                media: Vec::new(),
            },
            ModelMessage::Assistant {
                items: vec![ContentItem::ToolCall(call)],
            },
        ];
        let (facts, duplicates) = collect_tool_facts(&history);
        assert!(facts.contains_key("call-1"));
        assert!(duplicates.contains("call-1"));
    }

    #[tokio::test]
    async fn verified_commit_rechecks_model_receipts_and_only_commits_delivery_paths() {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "-q"]);
        git(root.path(), &["config", "user.email", "test@example.com"]);
        git(root.path(), &["config", "user.name", "Test"]);
        git(root.path(), &["config", "core.autocrlf", "false"]);
        let path = root.path().join("main.tl");
        std::fs::write(&path, "fn main() { print(1); }\n").unwrap();
        git(root.path(), &["add", "main.tl"]);
        git(root.path(), &["commit", "-qm", "base"]);
        std::fs::write(&path, "fn main() { print(2); }\n").unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let script = VerifiedArtifact {
            path: "main.tl".into(),
            sha256: crate::digest(&bytes),
            bytes: bytes.len() as u64,
        };
        let program_digest =
            crate::compile_digest_in_workspace(root.path(), &path, "hosted").unwrap();
        let compiled = teal::compile_source(
            teal::Source::new("stored.tl", "fn main() {}"),
            &teal::FsSourceLoader,
            teal::CompileOptions::default().with_tier(teal::RuntimeTier::Agentic),
        )
        .unwrap();
        let program =
            teal_agent::AgentProgram::from_shared_bytes(compiled.serialized_bytes().unwrap())
                .unwrap();
        let store = Arc::new(teal_agent::RuntimeStore::open(root.path()).unwrap());
        let run = store.create_run(&program).unwrap();
        let mut history = Vec::new();
        let mut ids = Vec::new();
        for (index, name) in ["teal.fmt_check", "teal.check", "teal.test", "teal.run"]
            .into_iter()
            .enumerate()
        {
            let id = format!("call-{index}");
            let arguments = serde_json::json!({"path":"main.tl", "tier":"hosted"});
            let value = serde_json::json!({
                "path":"main.tl", "exit_code":0,
                "source_digest":script.sha256,
                "program_digest":program_digest,
                "tier":"hosted",
            });
            history.push(ModelMessage::Assistant {
                items: vec![ContentItem::ToolCall(ToolCall {
                    id: id.clone(),
                    name: name.into(),
                    arguments,
                })],
            });
            history.push(ModelMessage::ToolResult {
                call_id: id.clone(),
                name: name.into(),
                value,
                is_error: false,
                media: Vec::new(),
            });
            ids.push(id);
        }
        store
            .open_model_session_state(&run.id, AgentInstanceId::from_raw(1), "task")
            .unwrap();
        store
            .save_model_session_state(
                &run.id,
                AgentInstanceId::from_raw(1),
                "task",
                &ModelSessionState {
                    history: history.clone(),
                    window_checkpoint: None,
                },
            )
            .unwrap();
        let verifier = teal::compile_source(
            teal::Source::new("verify.tl", r#"
                import "agents/delivery.tl" as delivery;
                @model("test-provider", "test-model", "default") agent Verifier {}
                impl Verifier {
                    // 此回归只复核已完成的模型证据，不调用 provider。
                    pub task model_turn(prompt: string) -> string { return self.chat(prompt).await; }
                    pub task check() -> string | Error {
                        let candidate = delivery::TaskDelivery {
                            summary: "done", script: "main.tl", artifacts: [],
                            verification: [
                                delivery::VerificationReference { call_id: "call-0", claim: "format" },
                                delivery::VerificationReference { call_id: "call-1", claim: "check" },
                                delivery::VerificationReference { call_id: "call-2", claim: "test" },
                                delivery::VerificationReference { call_id: "call-3", claim: "run" },
                            ],
                        };
                        let verified = delivery::verify(candidate, "task").await?;
                        match verified {
                            delivery::Validation.accepted { receipt } => { return receipt.program_digest; }
                            delivery::Validation.rejected { reason } => { return "rejected: " + reason; }
                        }
                    }
                }
                task main() -> int {
                    let verifier = Verifier {};
                    let result = verifier.check().await;
                    match result {
                        Error { .. } => { print(result.message); return 1; }
                        _ => { print(result); return 0; }
                    }
                }
            "#),
            &teal::FsSourceLoader,
            teal::CompileOptions::default().with_tier(teal::RuntimeTier::Agentic),
        ).unwrap();
        let verifier =
            teal_agent::AgentProgram::from_shared_bytes(verifier.serialized_bytes().unwrap())
                .unwrap();
        let verifier_program = verifier.clone();
        let verifier_run = store.create_run(&verifier).unwrap();
        store
            .open_model_session_state(&verifier_run.id, AgentInstanceId::from_raw(1), "task")
            .unwrap();
        store
            .save_model_session_state(
                &verifier_run.id,
                AgentInstanceId::from_raw(1),
                "task",
                &ModelSessionState {
                    history: history.clone(),
                    window_checkpoint: None,
                },
            )
            .unwrap();
        let sink = Arc::new(AuditSink::default());
        let _ = teal_security::install_process_sink(sink.clone());
        let access = WorkspaceProgramDigest::new(root.path()).unwrap();
        let services = teal_agent::RuntimeServices::new()
            .with_file_evidence(Arc::new(WorkspaceProgramDigest::new(root.path()).unwrap()))
            .with_program_digest(Arc::new(access))
            .with_permissions(Arc::new(teal_agent::AllowAllPermissions))
            .with_audit_context(teal_security::AuditContext::new(sink.clone()));
        let verifier_services = services.clone();
        let runtime = teal_agent::AgentRuntime::new(
            verifier,
            teal_model::TomlAuthManager::open(root.path(), teal_model::AuthProviderRegistry::new())
                .unwrap(),
            teal_model::ModelFactoryRegistry::new(),
        )
        .unwrap()
        .with_store(Arc::clone(&store), verifier_run.id)
        .unwrap()
        .with_services(services)
        .unwrap();
        let mut host = teal::NativeHostRuntime::builder()
            .audit_context(teal_security::process_context())
            .console(true)
            .build()
            .unwrap();
        runtime.run(&mut host).await.unwrap();
        assert_eq!(host.output(), format!("{program_digest}\n"));
        drop(runtime);
        let message = "verified script";
        let resource = format!("{}:{}:{message}", script.sha256, program_digest);
        let request = store
            .request_interaction(
                &ActionContext {
                    run_id: run.id.clone(),
                    task_id: "task".into(),
                    generation: 1,
                    policy_version: String::new(),
                },
                &teal_agent::approval_source_key("git.commit", &resource),
                UserQuestion::Choice {
                    prompt: "Commit verified script?".into(),
                    choices: vec![UserChoice {
                        id: "allow".into(),
                        label: "Allow".into(),
                    }],
                },
            )
            .unwrap();
        store
            .answer_interaction(
                &run.id,
                &request.id,
                request.context.generation,
                UserAnswer::Selected { id: "allow".into() },
            )
            .unwrap();
        let valid = VerifiedCommitRequest {
            run_id: run.id,
            agent_instance: AgentInstanceId::from_raw(1),
            session_key: "task".into(),
            message: message.into(),
            summary: "done".into(),
            script,
            artifacts: Vec::new(),
            program_digest,
            tier: "hosted".into(),
            verification_ids: ids,
            approval_id: request.id,
        };
        let mut forged = valid.clone();
        forged.verification_ids[0] = "invented".into();
        assert!(
            verify_commit_request(root.path(), &forged)
                .unwrap_err()
                .contains("not in the model history")
        );
        let mut changed_message = valid.clone();
        changed_message.message = "another commit".into();
        assert!(
            verify_commit_request(root.path(), &changed_message)
                .unwrap_err()
                .contains("approval does not match")
        );
        let mut stale = valid.clone();
        stale.script.sha256 = "0".repeat(64);
        assert!(verify_commit_request(root.path(), &stale).is_err());
        std::fs::write(&path, "fn main() { print(3); }\n").unwrap();
        assert!(
            verify_commit_request(root.path(), &valid)
                .unwrap_err()
                .contains("changed before commit")
        );
        let stale_run = store.create_run(&verifier_program).unwrap();
        store
            .open_model_session_state(&stale_run.id, AgentInstanceId::from_raw(1), "task")
            .unwrap();
        store
            .save_model_session_state(
                &stale_run.id,
                AgentInstanceId::from_raw(1),
                "task",
                &ModelSessionState {
                    history,
                    window_checkpoint: None,
                },
            )
            .unwrap();
        let stale_runtime = teal_agent::AgentRuntime::new(
            verifier_program,
            teal_model::TomlAuthManager::open(root.path(), teal_model::AuthProviderRegistry::new())
                .unwrap(),
            teal_model::ModelFactoryRegistry::new(),
        )
        .unwrap()
        .with_store(Arc::clone(&store), stale_run.id)
        .unwrap()
        .with_services(verifier_services)
        .unwrap();
        let mut stale_host = teal::NativeHostRuntime::builder()
            .audit_context(teal_security::process_context())
            .console(true)
            .build()
            .unwrap();
        stale_runtime.run(&mut stale_host).await.unwrap();
        assert!(
            stale_host
                .output()
                .starts_with("rejected: Script content changed")
        );
        std::fs::write(&path, &bytes).unwrap();
        let service =
            VerifiedCommitService::new(root.path(), teal_security::AuditContext::new(sink.clone()))
                .unwrap();
        let commit = service.commit(valid).await.unwrap();
        assert!(!commit.is_empty());
        let changed = Command::new("git")
            .args(["show", "--format=", "--name-only", &commit])
            .current_dir(root.path())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&changed.stdout).trim(), "main.tl");
        assert!(sink.0.lock().unwrap().len() >= 2);
    }
}
