//! 终端工作台对宿主中立源码 Agent 程序的交互适配。

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use tokio::sync::mpsc;
use sha2::{Digest, Sha256};

use super::{App, Severity};
use crate::{presentation::AgentBlock, runtime_bridge::RuntimeEvent};

static SOURCE_LAUNCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) enum AgentAppInput {
    Text(String),
    Ui(std::collections::BTreeMap<String, String>),
}

struct BenchWorkspaceBaseline {
    output: crate::runtime_bridge::RuntimeSender,
}

struct WorkspaceSourceLoader {
    workspace: PathBuf,
}

impl teal::SourceLoader for WorkspaceSourceLoader {
    fn load(
        &self,
        importer: &std::path::Path,
        import: &str,
    ) -> Result<teal::LoadedSource, teal::SourceLoadError> {
        let loaded = teal::FsSourceLoader.load(importer, import)?;
        if matches!(loaded.source.provenance, teal::SourceProvenance::User)
            && loaded.identity.is_absolute()
            && !loaded.identity.starts_with(&self.workspace)
        {
            return Err(teal::SourceLoadError {
                message: format!("import {import} leaves the workspace"),
            });
        }
        Ok(loaded)
    }
}

impl teal_agent::WorkspaceBaselineProvider for BenchWorkspaceBaseline {
    fn capture(&self) -> teal_agent::ProviderFuture<Result<String, teal_agent::AgentError>> {
        let output = self.output.clone();
        Box::pin(async move {
            let (reply, received) = tokio::sync::oneshot::channel();
            output
                .send(RuntimeEvent::AgentAppBaseline { reply })
                .await
                .map_err(|_| {
                    teal_agent::AgentError::Protocol("Agent UI channel is closed".into())
                })?;
            received
                .await
                .map_err(|_| {
                    teal_agent::AgentError::Protocol(
                        "workspace baseline reply is unavailable".into(),
                    )
                })?
                .map_err(teal_agent::AgentError::Protocol)
        })
    }
}

impl App {
    /// 把输入交给同一持久源码程序；Bench 不解释 Agent 命令或流程阶段。
    pub(super) fn submit_source_agent_input(&mut self, input: String) {
        if self.safe_mode {
            self.message(
                Severity::Warning,
                "agent",
                "Safe mode disables source Agent execution",
            );
            return;
        }
        if let (Some(services), Some(context)) =
            (&self.agent_app_services, &self.agent_app_active_context)
        {
            // 普通文本只 steering 当前模型调用；Bench 控制命令必须回到 Teal
            // 应用输入通道，由源码 Agent 在安全边界解释。
            if input.starts_with(':') {
                if let Some(sender) = &self.agent_app_tx {
                    if let Err(error) = sender.try_send(AgentAppInput::Text(input)) {
                        self.message(Severity::Warning, "agent", error.to_string());
                    }
                } else {
                    self.message(
                        Severity::Warning,
                        "agent",
                        "Agent input channel is unavailable",
                    );
                }
                return;
            }
            let result = if input == "stop" {
                services.cancel(context)
            } else {
                services.steer(context, input.clone())
            };
            match result {
                Ok(()) => self.message(Severity::Info, "agent", "Input sent to the active model"),
                Err(error) => self.message(Severity::Warning, "agent", error.to_string()),
            }
            return;
        }
        if let Some(sender) = &self.agent_app_tx {
            if let Err(error) = sender.try_send(AgentAppInput::Text(input)) {
                self.message(Severity::Warning, "agent", error.to_string());
            }
            return;
        }
        let (sender, receiver) = mpsc::channel(32);
        if let Err(error) = sender.try_send(AgentAppInput::Text(input)) {
            self.message(Severity::Error, "agent", error.to_string());
            return;
        }
        self.agent_app_tx = Some(sender);
        let workspace = self.workspace.clone();
        let script = self
            .active_agent
            .as_ref()
            .and_then(|name| self.agent_app_scripts.get(name))
            .cloned()
            .unwrap_or_default();
        let agent_name = self
            .active_agent
            .clone()
            .unwrap_or_else(|| "default".into());
        let access = self.model_access.clone();
        let tools = Arc::clone(&self.workspace_tools);
        let permissions = Arc::clone(&self.agent_permissions);
        let memory = Arc::clone(&self.agent_memory);
        let mcp_servers = self
            .mcp_settings
            .values()
            .map(|(_, settings)| settings.clone())
            .collect::<Vec<_>>();
        let output = self.runtime_tx.clone();
        let model_calls_enabled = self.connectivity
            != teal_workbench_services::agent_algorithms::ConnectivityMode::Offline;
        self.tasks.spawn(async move {
            let result = drive_default_agent(
                agent_name,
                workspace,
                script,
                access,
                tools,
                permissions,
                memory,
                mcp_servers,
                receiver,
                output.clone(),
                model_calls_enabled,
            )
            .await;
            let _ = output.send(RuntimeEvent::AgentAppStopped(result)).await;
        });
    }

    pub(super) fn handle_source_agent_event(&mut self, event: teal_agent::RuntimeAgentEvent) {
        if let teal_agent::AgentEvent::Application { kind, payload } = &event.event {
            if kind == "view" {
                self.render_source_agent_view(payload);
                return;
            }
            if kind == "delivery" {
                self.open_report_window(
                    teal_workbench_core::WindowKind::Delivery,
                    teal_workbench_core::WindowKind::Delivery.default_title(),
                    payload,
                );
                return;
            }
        }
        match &event.event {
            teal_agent::AgentEvent::Session(_)
            | teal_agent::AgentEvent::TextDelta { .. }
            | teal_agent::AgentEvent::ToolStarted(_) => {
                self.agent_app_active_context = Some(event.context.clone());
            }
            teal_agent::AgentEvent::Completed
            | teal_agent::AgentEvent::Cancelled
            | teal_agent::AgentEvent::Failed(_)
            | teal_agent::AgentEvent::WaitingForInput { .. } => {
                self.agent_app_active_context = None;
            }
            _ => {}
        }
        let panel = self.ensure_agent_panel();
        let Some(output) = self
            .panels
            .get_mut(panel)
            .and_then(|panel| panel.as_text_mut())
        else {
            return;
        };
        match event.event {
            teal_agent::AgentEvent::Application { kind, payload } if kind == "text" => {
                output.finalize_agent_markdown();
                output.append_agent_markdown(&payload);
            }
            teal_agent::AgentEvent::Application { kind, payload } if kind == "status" => {
                output.finalize_agent_markdown();
                output.push_agent_block(AgentBlock::Status(payload));
            }
            teal_agent::AgentEvent::Application { kind, payload } => {
                output.finalize_agent_markdown();
                output.push_agent_block(AgentBlock::Status(format!("{kind}: {payload}")));
            }
            // 模型增量是执行活动；正文由 Teal 的 emit/render 决定，避免完成时重复显示。
            teal_agent::AgentEvent::TextDelta { .. } => {}
            teal_agent::AgentEvent::ToolStarted(call) => {
                output.finalize_agent_markdown();
                output.push_agent_block(AgentBlock::Status(format!("Tool: {}", call.name)));
            }
            teal_agent::AgentEvent::WaitingForInput { request_id } => {
                output.push_agent_block(AgentBlock::Status(format!(
                    "Waiting for input: {request_id}"
                )));
            }
            _ => {}
        }
    }

    pub(super) fn show_source_agent_waiting(&mut self, request: teal_agent::InteractionRequest) {
        self.agent_app_active_context = None;
        let panel = self.ensure_agent_panel();
        let Some(output) = self
            .panels
            .get_mut(panel)
            .and_then(|panel| panel.as_text_mut())
        else {
            return;
        };
        if let teal_agent::UserQuestion::Choice { choices, .. } = &request.question {
            for choice in choices {
                output.push_agent_block(AgentBlock::Status(format!(
                    "[{}] {}",
                    choice.id, choice.label
                )));
            }
        }
        output.push_agent_block(AgentBlock::Status(request.question.prompt().to_owned()));
    }

    pub(super) fn finish_source_agent_app(&mut self, result: Result<(), String>) {
        self.agent_app_tx = None;
        self.agent_app_services = None;
        self.agent_app_active_context = None;
        if let Some(panel) = self.agent_app_view_panel {
            if let Some(custom) = self
                .panels
                .get_mut(panel)
                .and_then(|panel| panel.as_custom_mut())
            {
                *custom = crate::presentation::CustomPanel::new("Agent view".into());
            } else {
                self.agent_app_view_panel = None;
            }
        }
        match result {
            Ok(()) => self.message(Severity::Info, "agent", "Agent application completed"),
            Err(error) => self.message(Severity::Error, "agent", error),
        }
    }

    fn render_source_agent_view(&mut self, payload: &str) {
        let view = match crate::workbench::ui::DeclarativePanel::from_json(payload) {
            Ok(view) => view,
            Err(error) => {
                self.message(Severity::Error, "agent", error.to_string());
                return;
            }
        };
        let panel = self
            .agent_app_view_panel
            .filter(|id| self.panels.get(*id).is_some())
            .unwrap_or_else(|| {
                self.add_panel(
                    crate::presentation::CustomPanel::new("Agent view".into()),
                    teal_workbench_core::SplitAxis::Columns,
                )
            });
        self.agent_app_view_panel = Some(panel);
        if let Some(custom) = self
            .panels
            .get_mut(panel)
            .and_then(|panel| panel.as_custom_mut())
        {
            custom.ui = view;
        }
    }
}

/// 按源码 Agent 创建路径重建 Host grant；恢复路径不得使用空 grant。
pub(super) fn source_agent_grant(
    tools: &teal_workbench_services::tools::WorkspaceToolRuntime,
    program: &teal_agent::AgentProgram,
    model_calls_enabled: bool,
) -> Arc<teal::HostExecutionGrant> {
    let grant = (*tools.agent_grant())
        .clone()
        .allow_capability("filesystem.inspect")
        .allow_tools(program.inspection().source_tools.iter().cloned());
    Arc::new(if model_calls_enabled {
        grant
    } else {
        grant.restrict_capabilities([
            "editor.read",
            "editor.write",
            "agent.plan",
            "filesystem.inspect",
        ])
    })
}

#[allow(clippy::too_many_arguments)]
async fn drive_default_agent(
    agent_name: String,
    workspace: PathBuf,
    script: PathBuf,
    access: teal_model::ModelAccess,
    tools: Arc<teal_workbench_services::tools::WorkspaceToolRuntime>,
    permissions: Arc<teal_workbench_services::tools::ChannelApproval>,
    memory: Arc<teal_agent::MemoryStore>,
    mcp_servers: Vec<teal_workbench_automation::McpSettings>,
    mut input: mpsc::Receiver<AgentAppInput>,
    output: crate::runtime_bridge::RuntimeSender,
    model_calls_enabled: bool,
) -> Result<(), String> {
    let app_identity = if script.as_os_str().is_empty() {
        format!("{agent_name}:builtin/default")
    } else {
        format!(
            "{agent_name}:{}",
            script
                .canonicalize()
                .map_err(|error| error.to_string())?
                .display()
        )
    };
    let (source_name, source) = if script.as_os_str().is_empty() {
        (
            "runtime/agents/default.tl".to_owned(),
            include_str!("../../../../runtime/agents/default.tl").to_owned(),
        )
    } else {
        (
            script.to_string_lossy().into_owned(),
            std::fs::read_to_string(&script).map_err(|error| error.to_string())?,
        )
    };
    let compiled = teal::compile_source(
        teal::Source::new(source_name, source),
        &WorkspaceSourceLoader {
            workspace: workspace.clone(),
        },
        teal::CompileOptions::default().with_runtime_profile(teal::RuntimeProfile::agent()),
    )
    .map_err(|error| error.to_string())?;
    let program =
        teal_agent::AgentProgram::from_compiled(&compiled).map_err(|error| error.to_string())?;
    let store =
        Arc::new(teal_agent::RuntimeStore::open(&workspace).map_err(|error| error.to_string())?);
    let mut registry = tools
        .persistent_tool_registry()
        .map_err(|error| error.to_string())?;
    if !model_calls_enabled {
        registry.retain(|definition| {
            teal_workbench_services::agent_algorithms::allows_offline_model_tool(&definition.name)
        });
    }
    let mut grant = (*source_agent_grant(&tools, &program, model_calls_enabled)).clone();
    if model_calls_enabled {
        for settings in mcp_servers {
            let transport: Arc<dyn teal_agent::McpTransport> = match settings.transport.as_str() {
                "http" => Arc::new(
                    teal_agent::HttpMcpTransport::new(&settings.target)
                        .map_err(|error| error.to_string())?,
                ),
                "stdio" => {
                    let target = PathBuf::from(&settings.target);
                    let command = if target.is_absolute() {
                        target
                    } else {
                        workspace.join(target)
                    };
                    Arc::new(teal_agent::StdioMcpTransport::new(
                        teal_agent::McpStdioConfig {
                            command,
                            arguments: settings.arguments,
                            environment: std::collections::BTreeMap::new(),
                            cwd: workspace.clone(),
                        },
                    ))
                }
                _ => return Err("Unknown MCP transport".into()),
            };
            let client = teal_agent::McpClient::new(settings.name, transport)
                .map_err(|error| error.to_string())?;
            let names = client
                .list_tools()
                .await
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(|tool| format!("mcp__{}__{}", client.server(), tool.name))
                .collect::<Vec<_>>();
            grant = grant.allow_tools(names);
            teal_agent::register_mcp_tools(client, &grant, &mut registry)
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    let grant = Arc::new(grant);
    let (events, mut activity) = mpsc::channel(128);
    let activity_output = output.clone();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let forward = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    while let Ok(event) = activity.try_recv() {
                        if activity_output.send(RuntimeEvent::AgentAppEvent(event)).await.is_err() {
                            break;
                        }
                    }
                    break;
                }
                event = activity.recv() => {
                    let Some(event) = event else { break; };
                    if activity_output.send(RuntimeEvent::AgentAppEvent(event)).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    let review_permissions = Arc::clone(&permissions);
    let mut proposal_store = None;
    let mut proposal_workspace = String::new();
    let mut proposal_root = PathBuf::new();
    let mut services = teal_agent::RuntimeServices::new()
        .with_model_access(access.clone())
        .with_workspace_baseline(Arc::new(BenchWorkspaceBaseline {
            output: output.clone(),
        }))
        .with_program_digest(Arc::new(
            teal_workbench_services::WorkspaceProgramDigest::new(&workspace)?,
        ))
        .with_file_evidence(Arc::new(
            teal_workbench_services::WorkspaceProgramDigest::new(&workspace)?,
        ))
        .with_tools(Arc::new(registry))
        .with_workspace_transactions(tools.clone())
        .with_grant(Arc::clone(&grant))
        .with_permissions(permissions)
        .with_memory(memory)
        .with_events(events)
        .with_agent_ui(true)
        .with_model_calls_enabled(model_calls_enabled)
        .with_offline_local_chat(!model_calls_enabled)
        .with_audit_context(teal_security::process_context())
        .with_agent_factories(program.inspection().source_roles.keys().cloned())
        .map_err(|error| error.to_string())?;
    if model_calls_enabled {
        let config =
            teal_config::load_declarations(Some(&workspace)).map_err(|error| error.to_string())?;
        if config.workspace_trusted() && config.knowledge().enabled {
            let store = teal_knowledge::KnowledgeProposalStore::open_with_audit(
                &config
                    .paths()
                    .teal_home
                    .join("state/knowledge-proposals.sqlite3"),
                teal_security::process_context(),
            )
            .map_err(|error| error.to_string())?;
            let workspace = workspace
                .canonicalize()
                .map_err(|error| error.to_string())?;
            let identity = format!(
                "{:x}",
                Sha256::digest(workspace.to_string_lossy().as_bytes())
            );
            let store = Arc::new(store);
            proposal_store = Some(Arc::clone(&store));
            proposal_workspace = identity.clone();
            proposal_root = config.knowledge().workspace_layer_root(&workspace);
            services = services.with_knowledge_proposals(store, identity);
        }
    }
    if model_calls_enabled {
        services = services.with_verified_commit(Arc::new(
            teal_workbench_services::VerifiedCommitService::new(
                &workspace,
                teal_security::process_context(),
            )?,
        ));
    }
    let mut host = teal::NativeHostRuntime::builder()
        .trusted()
        .trusted()
        .trusted()
        .trusted()
        .authorization(grant.clone())
        .console(false)
        .file_system(true)
        .build()
        .map_err(|error| error.to_string())?;
    // 未完成的源码程序以 RuntimeStore 为事实来源；重开 Bench 继续同一个 VM 检查点。
    let mut existing = None;
    for run in store.runs().map_err(|error| error.to_string())? {
        if run.origin != teal_agent::RunOrigin::Program
            || run.program_digest != program.digest()
            || !matches!(
                run.state,
                teal_agent::RunState::Running | teal_agent::RunState::NeedsAttention
            )
        {
            continue;
        }
        let identity = store
            .checkpoint(&run.id, "agent-app-identity")
            .map_err(|error| error.to_string())?;
        if identity.as_deref() == Some(app_identity.as_bytes()) {
            let prepared = store
                .prepared_run(&run.id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| {
                    format!(
                        "prepared source Agent run `{}` is missing its launch record; explicit recovery is required",
                        run.id
                    )
                })?;
            if !prepared.started {
                return Err(format!(
                    "prepared source Agent run `{}` was not started; explicit recovery is required",
                    run.id
                ));
            }
            if existing.is_some() {
                return Err(
                    "multiple active source Agent runs match the same program identity".into(),
                );
            }
            existing = Some(run);
        }
    }
    let launch_identity = format!("{app_identity}:{}", program.digest());
    let context_digest = format!("{:x}", Sha256::digest(launch_identity.as_bytes()));
    let (run_id, mut execution) = match existing {
        Some(run) => {
            let run_id = run.id.clone();
            let execution = teal::restore_execution(
                &run_id,
                &mut host,
                access.clone().auth(),
                access.clone().models(),
                Arc::clone(&store),
                Arc::clone(&grant),
                services.clone(),
                false,
            )
            .map_err(|error| error.to_string())?;
            (run_id, execution)
        }
        None => {
            let launch_key = format!(
                "agent-app:{launch_identity}:launch:{}-{}-{}",
                teal_workbench_services::now_ms(),
                std::process::id(),
                SOURCE_LAUNCH_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            );
            let prepared = teal::prepare_execution(
                &compiled,
                store.as_ref(),
                &launch_key,
                &context_digest,
                teal::ExecutionBindings::default(),
            )
            .map_err(|error| error.to_string())?;
            let run_id = prepared.run_id.clone();
            store
                .save_checkpoint(&run_id, "agent-app-identity", app_identity.as_bytes())
                .map_err(|error| error.to_string())?;
            let execution = prepared
                .bind_with_access(
                    &mut host,
                    access,
                    Arc::clone(&store),
                    Arc::clone(&grant),
                    services.clone(),
                )
                .map_err(|error| error.to_string())?;
            (run_id, execution)
        }
    };
    output
        .send(RuntimeEvent::AgentAppReady(services.clone()))
        .await
        .map_err(|_| "Agent application event channel is closed".to_owned())?;
    let result: Result<(), String> = async {
        let mut step = execution.execute_step(&mut host).await;
        loop {
            match step {
                Ok(_) => break Ok(()),
                Err(teal_agent::AgentRuntimeError::NeedsInteraction(request)) => {
                    if matches!(
                        &request.question,
                        teal_agent::UserQuestion::AgentInput { .. }
                    ) && let Some(proposals) = &proposal_store
                    {
                        for proposal in proposals
                            .list(&proposal_workspace)
                            .map_err(|error| error.to_string())?
                            .into_iter()
                            .filter(|proposal| {
                                proposal.source_run == run_id
                                    && matches!(
                                        proposal.status,
                                        teal_knowledge::ProposalStatus::Pending
                                            | teal_knowledge::ProposalStatus::Reviewing
                                            | teal_knowledge::ProposalStatus::Unknown
                                    )
                            })
                        {
                            let _ = teal_workbench_automation::process_knowledge_proposal(
                                proposals,
                                &proposal.id,
                                &workspace,
                                &proposal_root,
                                None,
                                review_permissions.as_ref(),
                                teal_workbench_automation::KnowledgePublicationWriter::Workspace(
                                    tools.as_ref(),
                                ),
                                teal_security::process_context(),
                            )
                            .await;
                        }
                    }
                    output
                        .send(RuntimeEvent::AgentAppWaiting((*request).clone()))
                        .await
                        .map_err(|_| "Agent application event channel is closed".to_owned())?;
                    let value = input
                        .recv()
                        .await
                        .ok_or("Agent application input channel is closed")?;
                    let answer = match &request.question {
                        teal_agent::UserQuestion::AgentInput { .. } => {
                            let fields = match value {
                                AgentAppInput::Text(value) => {
                                    let (kind, value) =
                                        if let Some(command) = value.strip_prefix(':') {
                                            ("command", command.to_owned())
                                        } else {
                                            ("text", value)
                                        };
                                    std::collections::BTreeMap::from([
                                        ("kind".into(), kind.into()),
                                        ("value".into(), value),
                                    ])
                                }
                                AgentAppInput::Ui(fields) => fields,
                            };
                            teal_agent::UserAnswer::AgentInput { fields }
                        }
                        teal_agent::UserQuestion::Text { .. } => {
                            let AgentAppInput::Text(value) = value else {
                                return Err("UI event cannot answer a text question".into());
                            };
                            teal_agent::UserAnswer::Text { value }
                        }
                        teal_agent::UserQuestion::Choice { .. } => {
                            let AgentAppInput::Text(value) = value else {
                                return Err("UI event cannot answer a choice question".into());
                            };
                            teal_agent::UserAnswer::Selected { id: value }
                        }
                    };
                    store
                        .answer_interaction(
                            &run_id,
                            &request.id,
                            request.context.generation,
                            answer,
                        )
                        .map_err(|error| error.to_string())?;
                    step = execution.execute_step(&mut host).await;
                }
                Err(error) => break Err(error.to_string()),
            }
        }
    }
    .await;
    drop(execution);
    let _ = shutdown_tx.send(());
    let _ = forward.await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    struct AuditSink;

    impl teal_security::AuditSink for AuditSink {
        fn append(
            &self,
            _: &teal_security::Event,
        ) -> Result<teal_security::Receipt, teal_security::AuditError> {
            Ok(teal_security::Receipt {
                sequence: 1,
                hash: format!("{:064x}", 1),
            })
        }
    }

    fn source_test_dependencies(
        root: &Path,
    ) -> (
        Arc<teal_workbench_services::tools::WorkspaceToolRuntime>,
        Arc<teal_workbench_services::tools::ChannelApproval>,
        Arc<teal_agent::MemoryStore>,
        teal_model::ModelAccess,
    ) {
        let (mutation_tx, _) = mpsc::channel(8);
        let (changeset_tx, _) = mpsc::channel(8);
        let (approval_tx, _) = mpsc::channel(8);
        let tools = Arc::new(
            teal_workbench_services::tools::WorkspaceToolRuntime::new(
                root,
                root.join("checkpoints"),
                mutation_tx,
                changeset_tx,
                teal_lsp_client::DiagnosticStore::default(),
            )
            .unwrap(),
        );
        let permissions = Arc::new(teal_workbench_services::tools::ChannelApproval::new(
            approval_tx,
        ));
        let memory = Arc::new(
            teal_agent::MemoryStore::open(
                root.join("user-memory.sqlite3"),
                root.join("workspace-memory.sqlite3"),
            )
            .unwrap(),
        );
        let access =
            teal_model::ModelAccess::open(Arc::new(teal_model::ModelConfiguration::Empty)).unwrap();
        (tools, permissions, memory, access)
    }

    async fn run_source_once(
        agent_name: &str,
        root: &Path,
        script: PathBuf,
        access: teal_model::ModelAccess,
        tools: Arc<teal_workbench_services::tools::WorkspaceToolRuntime>,
        permissions: Arc<teal_workbench_services::tools::ChannelApproval>,
        memory: Arc<teal_agent::MemoryStore>,
    ) -> Result<(), String> {
        let (input_tx, input_rx) = mpsc::channel(8);
        input_tx
            .send(AgentAppInput::Text(":quit".into()))
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(256);
        drive_default_agent(
            agent_name.into(),
            root.to_path_buf(),
            script,
            access,
            tools,
            permissions,
            memory,
            Vec::new(),
            input_rx,
            crate::runtime_bridge::RuntimeSender::new(event_tx),
            true,
        )
        .await
    }

    #[tokio::test]
    async fn default_agent_exits_without_a_model_or_bench_flow() {
        let _ = teal_security::install_process_sink(Arc::new(AuditSink));
        let root = tempfile::tempdir().unwrap();
        let _environment = crate::app::test_support::TestHomeGuard::set(&root.path().join("home"));
        let (mutation_tx, _) = mpsc::channel(8);
        let (changeset_tx, _) = mpsc::channel(8);
        let (approval_tx, _) = mpsc::channel(8);
        let tools = Arc::new(
            teal_workbench_services::tools::WorkspaceToolRuntime::new(
                root.path(),
                root.path().join("checkpoints"),
                mutation_tx,
                changeset_tx,
                teal_lsp_client::DiagnosticStore::default(),
            )
            .unwrap(),
        );
        let access =
            teal_model::ModelAccess::open(Arc::new(teal_model::ModelConfiguration::Empty)).unwrap();
        let memory = Arc::new(
            teal_agent::MemoryStore::open(
                root.path().join("user-memory.sqlite3"),
                root.path().join("workspace-memory.sqlite3"),
            )
            .unwrap(),
        );
        let (input_tx, input_rx) = mpsc::channel(8);
        input_tx
            .send(AgentAppInput::Text(":evidence".into()))
            .await
            .unwrap();
        input_tx
            .send(AgentAppInput::Text(":quit".into()))
            .await
            .unwrap();
        let (event_tx, mut event_rx) = mpsc::channel(16);
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            drive_default_agent(
                "default".into(),
                root.path().to_path_buf(),
                PathBuf::new(),
                access,
                tools,
                Arc::new(teal_workbench_services::tools::ChannelApproval::new(
                    approval_tx,
                )),
                memory,
                Vec::new(),
                input_rx,
                crate::runtime_bridge::RuntimeSender::new(event_tx),
                false,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let mut saw_view = false;
        let mut saw_missing_delivery = false;
        while let Ok(event) = event_rx.try_recv() {
            if let RuntimeEvent::Observed { event, .. } = event
                && let RuntimeEvent::AgentAppEvent(teal_agent::RuntimeAgentEvent {
                    event: teal_agent::AgentEvent::Application { kind, payload },
                    ..
                }) = *event
            {
                if kind == "view" {
                    saw_view = true;
                }
                if kind == "text" && payload == "No verified delivery is available" {
                    saw_missing_delivery = true;
                }
            }
        }
        assert!(
            saw_view,
            "Bench must receive the Teal application's view event"
        );
        assert!(
            saw_missing_delivery,
            "delivery status must come from the Teal application"
        );
    }

    #[tokio::test]
    async fn source_application_approval_round_trips_through_bench_input() {
        let _ = teal_security::install_process_sink(Arc::new(AuditSink));
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("approval.tl");
        std::fs::write(
            &script,
            r#"
            import "std/agent.tl" as app;
            import "std/approval.tl" as approval;
            task main() {
                let receipt = approval::request("Approve a stage", "stage.run", "resource-1").await;
                if approval::valid(receipt) { app::emit("text", "approved"); }
            }
        "#,
        )
        .unwrap();
        let (mutation_tx, _) = mpsc::channel(8);
        let (changeset_tx, _) = mpsc::channel(8);
        let (approval_tx, _) = mpsc::channel(8);
        let tools = Arc::new(
            teal_workbench_services::tools::WorkspaceToolRuntime::new(
                root.path(),
                root.path().join("checkpoints"),
                mutation_tx,
                changeset_tx,
                teal_lsp_client::DiagnosticStore::default(),
            )
            .unwrap(),
        );
        let access =
            teal_model::ModelAccess::open(Arc::new(teal_model::ModelConfiguration::Empty)).unwrap();
        let memory = Arc::new(
            teal_agent::MemoryStore::open(
                root.path().join("user-memory.sqlite3"),
                root.path().join("workspace-memory.sqlite3"),
            )
            .unwrap(),
        );
        let (input_tx, input_rx) = mpsc::channel(8);
        input_tx
            .send(AgentAppInput::Text("allow".into()))
            .await
            .unwrap();
        let (event_tx, mut event_rx) = mpsc::channel(16);
        drive_default_agent(
            "approval".into(),
            root.path().to_path_buf(),
            script,
            access,
            tools,
            Arc::new(teal_workbench_services::tools::ChannelApproval::new(
                approval_tx,
            )),
            memory,
            Vec::new(),
            input_rx,
            crate::runtime_bridge::RuntimeSender::new(event_tx),
            false,
        )
        .await
        .unwrap();
        let mut saw_question = false;
        let mut saw_result = false;
        while let Ok(event) = event_rx.try_recv() {
            if let RuntimeEvent::Observed { event, .. } = event {
                match *event {
                    RuntimeEvent::AgentAppWaiting(request)
                        if matches!(request.question, teal_agent::UserQuestion::Choice { .. }) =>
                    {
                        saw_question = true;
                    }
                    RuntimeEvent::AgentAppEvent(teal_agent::RuntimeAgentEvent {
                        event: teal_agent::AgentEvent::Application { kind, payload },
                        ..
                    }) if kind == "text" && payload == "approved" => {
                        saw_result = true;
                    }
                    _ => {}
                }
            }
        }
        assert!(saw_question && saw_result);
    }

    #[tokio::test]
    async fn bench_reopens_the_same_source_run_after_input_wait() {
        let _ = teal_security::install_process_sink(Arc::new(AuditSink));
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let _home_guard = crate::app::test_support::TestHomeGuard::set(&home);
        let (mutation_tx, _) = mpsc::channel(8);
        let (changeset_tx, _) = mpsc::channel(8);
        let (approval_tx, _) = mpsc::channel(8);
        let tools = Arc::new(
            teal_workbench_services::tools::WorkspaceToolRuntime::new(
                root.path(),
                root.path().join("checkpoints"),
                mutation_tx,
                changeset_tx,
                teal_lsp_client::DiagnosticStore::default(),
            )
            .unwrap(),
        );
        let access =
            teal_model::ModelAccess::open(Arc::new(teal_model::ModelConfiguration::Empty)).unwrap();
        let memory = Arc::new(
            teal_agent::MemoryStore::open(
                root.path().join("user-memory.sqlite3"),
                root.path().join("workspace-memory.sqlite3"),
            )
            .unwrap(),
        );
        let permissions = Arc::new(teal_workbench_services::tools::ChannelApproval::new(
            approval_tx,
        ));
        let (held_input, first_input) = mpsc::channel(8);
        let (first_events, mut output) = mpsc::channel(32);
        let mut first = tokio::spawn(drive_default_agent(
            "default".into(),
            root.path().to_path_buf(),
            PathBuf::new(),
            access.clone(),
            Arc::clone(&tools),
            Arc::clone(&permissions),
            Arc::clone(&memory),
            Vec::new(),
            first_input,
            crate::runtime_bridge::RuntimeSender::new(first_events),
            true,
        ));
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    finished = &mut first => panic!("Agent app stopped before waiting: {finished:?}"),
                    event = output.recv() => match event {
                        Some(RuntimeEvent::Observed { event, .. })
                            if matches!(*event, RuntimeEvent::AgentAppWaiting(_)) => break,
                        Some(_) => {},
                        None => panic!("Agent app event channel closed before input wait"),
                    },
                }
            }
        }).await.unwrap();
        first.abort();
        let _ = first.await;
        drop(held_input);
        drop(output);

        let store = teal_agent::RuntimeStore::open(root.path()).unwrap();
        let before = store.runs().unwrap();
        assert_eq!(before.len(), 1);
        let (next_input_tx, next_input) = mpsc::channel(8);
        next_input_tx
            .send(AgentAppInput::Text(":quit".into()))
            .await
            .unwrap();
        let (next_events, _next_output) = mpsc::channel(32);
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            drive_default_agent(
                "default".into(),
                root.path().to_path_buf(),
                PathBuf::new(),
                access.clone(),
                Arc::clone(&tools),
                Arc::clone(&permissions),
                Arc::clone(&memory),
                Vec::new(),
                next_input,
                crate::runtime_bridge::RuntimeSender::new(next_events),
                true,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let after = store.runs().unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, before[0].id);
        assert_eq!(after[0].state, teal_agent::RunState::Completed);

        let (other_input_tx, other_input) = mpsc::channel(8);
        other_input_tx
            .send(AgentAppInput::Text(":quit".into()))
            .await
            .unwrap();
        let (other_events, _other_output) = mpsc::channel(32);
        drive_default_agent(
            "other".into(),
            root.path().to_path_buf(),
            PathBuf::new(),
            access,
            tools,
            permissions,
            memory,
            Vec::new(),
            other_input,
            crate::runtime_bridge::RuntimeSender::new(other_events),
            true,
        )
        .await
        .unwrap();
        assert_eq!(
            store.runs().unwrap().len(),
            2,
            "another application needs its own run"
        );
    }

    #[tokio::test]
    async fn completed_source_agent_starts_a_new_run_for_the_next_invocation() {
        let _ = teal_security::install_process_sink(Arc::new(AuditSink));
        let root = tempfile::tempdir().unwrap();
        let _home_guard = crate::app::test_support::TestHomeGuard::set(&root.path().join("home"));
        let (tools, permissions, memory, access) = source_test_dependencies(root.path());

        run_source_once(
            "default",
            root.path(),
            PathBuf::new(),
            access.clone(),
            Arc::clone(&tools),
            Arc::clone(&permissions),
            Arc::clone(&memory),
        )
        .await
        .unwrap();
        run_source_once(
            "default",
            root.path(),
            PathBuf::new(),
            access,
            tools,
            permissions,
            memory,
        )
        .await
        .unwrap();

        let runs = teal_agent::RuntimeStore::open(root.path())
            .unwrap()
            .runs()
            .unwrap();
        assert_eq!(runs.len(), 2, "each completed invocation needs a new run");
        assert!(
            runs.iter()
                .all(|run| run.state == teal_agent::RunState::Completed)
        );
    }

    #[tokio::test]
    async fn changing_a_source_agent_creates_a_new_program_run() {
        let _ = teal_security::install_process_sink(Arc::new(AuditSink));
        let root = tempfile::tempdir().unwrap();
        let _home_guard = crate::app::test_support::TestHomeGuard::set(&root.path().join("home"));
        let script = root.path().join("agent.tl");
        let source = |marker: &str| {
            format!(
                "import \"std/agent.tl\" as app;\nconst marker: string = \"{marker}\";\ntask main() {{ let input = app::receive().await; }}\n"
            )
        };
        std::fs::write(&script, source("one")).unwrap();
        let (tools, permissions, memory, access) = source_test_dependencies(root.path());
        run_source_once(
            "custom",
            root.path(),
            script.clone(),
            access.clone(),
            Arc::clone(&tools),
            Arc::clone(&permissions),
            Arc::clone(&memory),
        )
        .await
        .unwrap();
        std::fs::write(&script, source("two")).unwrap();
        run_source_once(
            "custom",
            root.path(),
            script,
            access,
            tools,
            permissions,
            memory,
        )
        .await
        .unwrap();

        let runs = teal_agent::RuntimeStore::open(root.path())
            .unwrap()
            .runs()
            .unwrap();
        assert_eq!(runs.len(), 2, "a changed source must not reuse the old run");
        assert!(
            runs.iter()
                .all(|run| run.state == teal_agent::RunState::Completed)
        );
        assert_ne!(runs[0].program_digest, runs[1].program_digest);
    }

    #[tokio::test]
    async fn ambiguous_active_source_runs_are_rejected() {
        let _ = teal_security::install_process_sink(Arc::new(AuditSink));
        let root = tempfile::tempdir().unwrap();
        let _home_guard = crate::app::test_support::TestHomeGuard::set(&root.path().join("home"));
        let (tools, permissions, memory, access) = source_test_dependencies(root.path());

        let (held_first, first_input) = mpsc::channel(8);
        let (first_events, mut first_output) = mpsc::channel(32);
        let mut first = tokio::spawn(drive_default_agent(
            "default".into(),
            root.path().to_path_buf(),
            PathBuf::new(),
            access.clone(),
            Arc::clone(&tools),
            Arc::clone(&permissions),
            Arc::clone(&memory),
            Vec::new(),
            first_input,
            crate::runtime_bridge::RuntimeSender::new(first_events),
            true,
        ));
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    finished = &mut first => panic!("first Agent stopped before waiting: {finished:?}"),
                    event = first_output.recv() => match event {
                        Some(RuntimeEvent::Observed { event, .. }) if matches!(*event, RuntimeEvent::AgentAppWaiting(_)) => break,
                        Some(_) => {},
                        None => panic!("first Agent event channel closed"),
                    },
                }
            }
        }).await.unwrap();
        first.abort();
        let _ = first.await;
        drop(held_first);
        drop(first_output);

        let (held_second, second_input) = mpsc::channel(8);
        let (second_events, mut second_output) = mpsc::channel(32);
        let mut second = tokio::spawn(drive_default_agent(
            "other".into(),
            root.path().to_path_buf(),
            PathBuf::new(),
            access.clone(),
            Arc::clone(&tools),
            Arc::clone(&permissions),
            Arc::clone(&memory),
            Vec::new(),
            second_input,
            crate::runtime_bridge::RuntimeSender::new(second_events),
            true,
        ));
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    finished = &mut second => panic!("second Agent stopped before waiting: {finished:?}"),
                    event = second_output.recv() => match event {
                        Some(RuntimeEvent::Observed { event, .. }) if matches!(*event, RuntimeEvent::AgentAppWaiting(_)) => break,
                        Some(_) => {},
                        None => panic!("second Agent event channel closed"),
                    },
                }
            }
        }).await.unwrap();
        second.abort();
        let _ = second.await;
        drop(held_second);
        drop(second_output);

        let store = teal_agent::RuntimeStore::open(root.path()).unwrap();
        let runs = store.runs().unwrap();
        assert_eq!(runs.len(), 2);
        let other_identity = b"other:builtin/default";
        let other = runs
            .iter()
            .find(|run| {
                store
                    .checkpoint(&run.id, "agent-app-identity")
                    .unwrap()
                    .as_deref()
                    == Some(other_identity)
            })
            .expect("the second application run should be identifiable");
        store
            .save_checkpoint(&other.id, "agent-app-identity", b"default:builtin/default")
            .unwrap();

        let (input_tx, input_rx) = mpsc::channel(8);
        drop(input_tx);
        let (event_tx, _event_rx) = mpsc::channel(256);
        let error = drive_default_agent(
            "default".into(),
            root.path().to_path_buf(),
            PathBuf::new(),
            access,
            tools,
            permissions,
            memory,
            Vec::new(),
            input_rx,
            crate::runtime_bridge::RuntimeSender::new(event_tx),
            true,
        )
        .await
        .unwrap_err();
        assert!(error.contains("multiple active source Agent runs"));
    }

    #[tokio::test]
    async fn unstarted_prepared_source_run_is_not_silently_replaced() {
        let _ = teal_security::install_process_sink(Arc::new(AuditSink));
        let root = tempfile::tempdir().unwrap();
        let _home_guard = crate::app::test_support::TestHomeGuard::set(&root.path().join("home"));
        let script = root.path().join("prepared.tl");
        let source =
            "import \"std/agent.tl\" as app; task main() { let input = app::receive().await; }\n";
        std::fs::write(&script, source).unwrap();
        let source_name = script.to_string_lossy().into_owned();
        let compiled = teal::compile_source(
            teal::Source::new(source_name, source),
            &WorkspaceSourceLoader {
                workspace: root.path().to_path_buf(),
            },
            teal::CompileOptions::default().with_runtime_profile(teal::RuntimeProfile::agent()),
        )
        .unwrap();
        let program = teal_agent::AgentProgram::from_compiled(&compiled).unwrap();
        let store = teal_agent::RuntimeStore::open(root.path()).unwrap();
        let launch_identity = format!(
            "custom:{}:{}",
            script.canonicalize().unwrap().display(),
            program.digest()
        );
        let context_digest = format!("{:x}", Sha256::digest(launch_identity.as_bytes()));
        let prepared = teal::prepare_execution(
            &compiled,
            &store,
            "agent-app:unstarted",
            &context_digest,
            teal::ExecutionBindings::default(),
        )
        .unwrap();
        store
            .save_checkpoint(
                &prepared.run_id,
                "agent-app-identity",
                format!("custom:{}", script.canonicalize().unwrap().display()).as_bytes(),
            )
            .unwrap();

        let (tools, permissions, memory, access) = source_test_dependencies(root.path());
        let (input_tx, input_rx) = mpsc::channel(8);
        drop(input_tx);
        let (event_tx, _event_rx) = mpsc::channel(256);
        let error = drive_default_agent(
            "custom".into(),
            root.path().to_path_buf(),
            script,
            access,
            tools,
            permissions,
            memory,
            Vec::new(),
            input_rx,
            crate::runtime_bridge::RuntimeSender::new(event_tx),
            true,
        )
        .await
        .unwrap_err();
        assert!(error.contains("prepared source Agent run") && error.contains("not started"));
        assert_eq!(store.runs().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn safe_mode_rejects_source_agent_input_without_creating_runtime_state() {
        let _ = teal_security::install_process_sink(Arc::new(AuditSink));
        let root = tempfile::tempdir().unwrap();
        let _home_guard = crate::app::test_support::TestHomeGuard::set(&root.path().join("home"));
        let state = root.path().join("bench-state");
        let mut app = App::new_with_session_paths(
            crate::app::AppOptions {
                start_file: None,
                path: Some(root.path().to_path_buf()),
                safe_mode: true,
                no_config: true,
                session: None,
                model_protocol_capture: None,
            },
            None,
            false,
            teal_workbench_services::agent_algorithms::ConnectivityMode::Online,
            Some(teal_workbench_services::SessionPaths {
                config: state.join("config"),
                state: state.clone(),
                cache: state.join("cache"),
            }),
        )
        .await
        .unwrap();
        app.submit_source_agent_input("should not start".into());
        assert!(app.agent_app_tx.is_none());
        assert!(app.agent_app_services.is_none());
        assert!(!root.path().join(".teal/runtime/agent.sqlite3").exists());
        app.tasks.shutdown().await;
    }

    #[tokio::test]
    async fn closing_and_restarting_the_source_agent_view_does_not_duplicate_panels() {
        let _ = teal_security::install_process_sink(Arc::new(AuditSink));
        let root = tempfile::tempdir().unwrap();
        let _home_guard = crate::app::test_support::TestHomeGuard::set(&root.path().join("home"));
        let state = root.path().join("bench-state");
        let mut app = App::new_with_session_paths(
            crate::app::AppOptions {
                start_file: None,
                path: Some(root.path().to_path_buf()),
                safe_mode: true,
                no_config: true,
                session: None,
                model_protocol_capture: None,
            },
            None,
            false,
            teal_workbench_services::agent_algorithms::ConnectivityMode::Online,
            Some(teal_workbench_services::SessionPaths {
                config: state.join("config"),
                state: state.clone(),
                cache: state.join("cache"),
            }),
        )
        .await
        .unwrap();
        let view = serde_json::json!({
            "version": 1,
            "root": {
                "id": "message",
                "width": {"fill": 1},
                "height": "auto",
                "content": {"kind": "text", "text": "view", "style": 0, "wrap": true}
            }
        })
        .to_string();
        app.render_source_agent_view(&view);
        let first = app.agent_app_view_panel.expect("first view panel");
        app.close_panel(first, true);
        app.render_source_agent_view(&view);
        let second = app
            .agent_app_view_panel
            .expect("view should recreate after close");
        assert_ne!(first, second);
        app.finish_source_agent_app(Ok(()));
        app.render_source_agent_view(&view);
        assert_eq!(app.agent_app_view_panel, Some(second));
        app.tasks.shutdown().await;
    }
}
