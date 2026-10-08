//! 持久 Agent run 的工作台管理视图。

use std::sync::Arc;

use super::{App, AppPanel, PanelKind, Severity, SplitAxis, TextPanel};
use teal_agent::{RunManagementProvider as _, RunState};

#[derive(Clone)]
struct BenchRunDriver {
    workspace: std::path::PathBuf,
    store: Arc<teal_agent::RuntimeStore>,
    tools: Arc<teal_workbench_services::tools::WorkspaceToolRuntime>,
    access: teal_model::ModelAccess,
    model_calls_enabled: bool,
    permissions: Arc<dyn teal_agent::PermissionProvider>,
    knowledge: Arc<teal_workbench_services::WorkbenchKnowledge>,
    memory: Arc<teal_agent::MemoryStore>,
}

impl teal::RunManagementDriver for BenchRunDriver {
    fn drive(
        &self,
        run_id: String,
        reconcile_only: bool,
    ) -> teal_agent::ProviderFuture<
        Result<teal_agent::RunManagementOutcome, teal_agent::RunManagementError>,
    > {
        let store = Arc::clone(&self.store);
        let workspace = self.workspace.clone();
        let tools = Arc::clone(&self.tools);
        let access = self.access.clone();
        let model_calls_enabled = self.model_calls_enabled;
        let permissions = Arc::clone(&self.permissions);
        let knowledge = Arc::clone(&self.knowledge);
        let memory = Arc::clone(&self.memory);
        Box::pin(async move {
            let saved = store
                .run(&run_id)
                .map_err(provider_error)?
                .ok_or_else(|| teal_agent::RunManagementError::NotFound(run_id.clone()))?;
            let terminal = matches!(
                saved.state,
                RunState::Completed
                    | RunState::CompletedWithErrors
                    | RunState::Failed
                    | RunState::Cancelled
                    | RunState::Abandoned
            );
            if terminal && !reconcile_only {
                return Ok(teal_agent::RunManagementOutcome {
                    run_id,
                    state: saved.state,
                    exit_code: Some(0),
                    output: String::new(),
                    needs_attention: false,
                });
            }
            let bytes = store
                .program(&run_id)
                .map_err(provider_error)?
                .ok_or_else(|| teal_agent::RunManagementError::NotFound(run_id.clone()))?;
            let program = teal_agent::AgentProgram::from_bytes(bytes)
                .map_err(|error| teal_agent::RunManagementError::Provider(error.to_string()))?;
            let grant = super::agent_app::source_agent_grant(&tools, &program, model_calls_enabled);
            let mut registry = tools
                .persistent_tool_registry()
                .map_err(|error| teal_agent::RunManagementError::Provider(error.to_string()))?;
            if !model_calls_enabled {
                registry.retain(|definition| {
                    teal_workbench_services::agent_algorithms::allows_offline_model_tool(
                        &definition.name,
                    )
                });
            }
            let bindings = store
                .execution_bindings(&run_id)
                .map_err(provider_error)?
                .unwrap_or_default();
            let requirements = program
                .authentication_requirements(&bindings)
                .map_err(|error| teal_agent::RunManagementError::Provider(error.to_string()))?
                .into_iter()
                .map(|requirement| {
                    teal_model::ModelAuthenticationRequirement::parse(
                        requirement.provider,
                        requirement.profile,
                        requirement.model,
                    )
                    .map_err(|error| teal_agent::RunManagementError::Provider(error.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            access
                .prepare_models(requirements, teal_model::Cancellation::new())
                .await
                .map_err(|error| teal_agent::RunManagementError::Provider(error.to_string()))?;
            let mut host = teal::NativeHostRuntime::builder()
                .trusted()
                .trusted()
                .authorization(grant.clone())
                .echo(false)
                .dylib_policy(teal::DylibPolicy::Deny)
                .build()
                .map_err(|error| teal_agent::RunManagementError::Provider(error.to_string()))?;
            let nested_driver = Arc::new(BenchRunDriver {
                workspace: workspace.clone(),
                store: Arc::clone(&store),
                tools: Arc::clone(&tools),
                access: access.clone(),
                model_calls_enabled,
                permissions: Arc::clone(&permissions),
                knowledge: Arc::clone(&knowledge),
                memory: Arc::clone(&memory),
            });
            let nested_manager = Arc::new(teal::PersistentRunManagement::new(
                workspace.clone(),
                Arc::clone(&store),
                Arc::clone(&permissions),
                nested_driver,
            ));
            let digest: Arc<dyn teal_agent::ProgramDigestProvider> = Arc::new(
                teal_workbench_services::WorkspaceProgramDigest::new(&workspace)
                    .map_err(|error| teal_agent::RunManagementError::Provider(error.to_string()))?,
            );
            let file_evidence: Arc<dyn teal_agent::FileEvidenceProvider> = Arc::new(
                teal_workbench_services::WorkspaceProgramDigest::new(&workspace)
                    .map_err(|error| teal_agent::RunManagementError::Provider(error.to_string()))?,
            );
            let transactions: Arc<dyn teal_agent::WorkspaceTransactionProvider> = tools.clone();
            let (events, mut activity) = tokio::sync::mpsc::channel(128);
            tokio::spawn(async move { while activity.recv().await.is_some() {} });
            let mut services = teal::RuntimeServices::new()
                .with_model_access(access.clone())
                .with_workspace_baseline(Arc::new(
                    teal_workbench_services::GitWorkspaceBaseline::new(workspace.clone()),
                ))
                .with_program_digest(Arc::clone(&digest))
                .with_file_evidence(file_evidence)
                .with_tools(Arc::new(registry))
                .with_workspace_transactions(transactions)
                .with_grant(Arc::clone(&grant))
                .with_permissions(permissions)
                .with_knowledge(knowledge)
                .with_memory(memory)
                .with_run_management(nested_manager)
                .with_events(events)
                .with_agent_ui(false)
                .with_model_calls_enabled(model_calls_enabled)
                .with_offline_local_chat(!model_calls_enabled)
                .with_audit_context(teal_security::process_context())
                .with_agent_factories(program.inspection().source_roles.keys().cloned())
                .map_err(|error| teal_agent::RunManagementError::Provider(error.to_string()))?;
            if model_calls_enabled {
                services = services.with_verified_commit(Arc::new(
                    teal_workbench_services::VerifiedCommitService::new(
                        &workspace,
                        teal_security::process_context(),
                    )
                    .map_err(|error| teal_agent::RunManagementError::Provider(error.to_string()))?,
                ));
            }
            let execution = teal::restore_execution_preflighted_with_access(
                &run_id,
                &mut host,
                access,
                Arc::clone(&store),
                grant,
                services,
                terminal && reconcile_only,
            )
            .map_err(|error| teal_agent::RunManagementError::Provider(error.to_string()))?;
            let exit_code = if reconcile_only {
                let mut report = execution
                    .runtime()
                    .reconcile_with_host(&mut host)
                    .await
                    .map_err(|error| teal_agent::RunManagementError::Provider(error.to_string()))?;
                let detached = execution
                    .runtime()
                    .reconcile_detached()
                    .await
                    .map_err(|error| teal_agent::RunManagementError::Provider(error.to_string()))?;
                report.pending = report.pending.saturating_add(detached.pending);
                report.unsupported = report.unsupported.saturating_add(detached.unsupported);
                report.failed = report.failed.saturating_add(detached.failed);
                if report.failed > 0 {
                    if !terminal && let Some(claim) = execution.runtime().run_claim() {
                        claim
                            .set_run_state(&run_id, RunState::Failed)
                            .map_err(provider_error)?;
                    }
                    Some(1)
                } else if report.pending > 0 || report.unsupported > 0 {
                    if !terminal && let Some(claim) = execution.runtime().run_claim() {
                        claim
                            .set_run_state(&run_id, RunState::NeedsAttention)
                            .map_err(provider_error)?;
                    }
                    Some(2)
                } else {
                    Some(0)
                }
            } else {
                Some(
                    execution.execute(&mut host).await.map_err(|error| {
                        teal_agent::RunManagementError::Provider(error.to_string())
                    })?,
                )
            };
            let run = store
                .run(&run_id)
                .map_err(provider_error)?
                .ok_or_else(|| teal_agent::RunManagementError::NotFound(run_id.clone()))?;
            Ok(teal_agent::RunManagementOutcome {
                run_id,
                state: run.state,
                exit_code,
                output: host.output().to_owned(),
                needs_attention: run.state == RunState::NeedsAttention,
            })
        })
    }
}

impl App {
    pub(super) fn runs_command(&mut self, arguments: &[String]) {
        let store = match teal_agent::RuntimeStore::connect(&self.workspace) {
            Ok(store) => Arc::new(store),
            Err(error) => {
                self.message(Severity::Error, "runs", error.to_string());
                return;
            }
        };
        let rendered: Result<String, String> = match arguments {
            [] => {
                let provider = self.run_management_provider(Arc::clone(&store));
                let output = self.runtime_tx.clone();
                self.tasks.spawn(async move {
                    let result = provider
                        .list(run_context("list"))
                        .await
                        .map(render_list)
                        .map(|text| TextPanel::plain(PanelKind::Output, "Runs", text))
                        .map_err(|error| error.to_string());
                    let _ = output
                        .send(crate::runtime_bridge::RuntimeEvent::TextLoaded {
                            result,
                            axis: SplitAxis::Columns,
                        })
                        .await;
                });
                return;
            }
            [action] if action == "list" => {
                self.runs_command(&[]);
                return;
            }
            [action, run_id] if action == "show" => {
                let provider = self.run_management_provider(Arc::clone(&store));
                let target = run_id.clone();
                let output = self.runtime_tx.clone();
                self.tasks.spawn(async move {
                    let result = provider
                        .inspect(run_context("show"), target)
                        .await
                        .map(render_details)
                        .map(|text| TextPanel::plain(PanelKind::Output, "Runs", text))
                        .map_err(|error| error.to_string());
                    let _ = output
                        .send(crate::runtime_bridge::RuntimeEvent::TextLoaded {
                            result,
                            axis: SplitAxis::Columns,
                        })
                        .await;
                });
                return;
            }
            [action, run_id, output] if action == "profile" => {
                let provider = self.run_management_provider(Arc::clone(&store));
                let context = run_context("profile");
                let target = run_id.clone();
                let destination = output.clone();
                let output = self.runtime_tx.clone();
                self.tasks.spawn(async move {
                    let result = provider
                        .export_profile(context, target, destination)
                        .await
                        .map(|report| {
                            TextPanel::plain(
                                PanelKind::Output,
                                "Runs",
                                format!(
                                    "run={}\noutput={}\nwall_samples={}\nmanaged_allocations={}\ndropped_samples={}",
                                    report.run_id,
                                    report.output,
                                    report.wall_samples,
                                    report.managed_allocations,
                                    report.dropped_samples,
                                ),
                            )
                        })
                        .map_err(|error| error.to_string());
                    let _ = output
                        .send(crate::runtime_bridge::RuntimeEvent::TextLoaded {
                            result,
                            axis: SplitAxis::Columns,
                        })
                        .await;
                });
                return;
            }
            [action, run_id, rest @ ..] if action == "answer" => {
                let mut answer = vec![run_id.clone()];
                answer.extend_from_slice(rest);
                self.open_durable_question(&answer);
                return;
            }
            [action, run_id] if action == "resume" || action == "reconcile" => {
                let provider = self.run_management_provider(store);
                let context = run_context(action);
                let target = run_id.clone();
                let reconcile = action == "reconcile";
                let output = self.runtime_tx.clone();
                self.tasks.spawn(async move {
                    let result = if reconcile {
                        provider.reconcile(context, target).await
                    } else {
                        provider.resume(context, target).await
                    }
                    .map(|outcome| {
                        TextPanel::plain(
                            PanelKind::Output,
                            "Runs",
                            format!(
                                "run={}\nstate={:?}\nexit_code={:?}\nneeds_attention={}\n{}",
                                outcome.run_id,
                                outcome.state,
                                outcome.exit_code,
                                outcome.needs_attention,
                                outcome.output,
                            ),
                        )
                    })
                    .map_err(|error| error.to_string());
                    let _ = output
                        .send(crate::runtime_bridge::RuntimeEvent::TextLoaded {
                            result,
                            axis: SplitAxis::Columns,
                        })
                        .await;
                });
                return;
            }
            _ => Err(
                "Usage: :runs [list|show RUN_ID|answer RUN_ID [REQUEST_ID]|reconcile RUN_ID|resume RUN_ID|profile RUN_ID FILE]"
                    .into(),
            ),
        };
        match rendered {
            Ok(text) => {
                let panel = TextPanel::plain(PanelKind::Output, "Runs", text);
                self.add_panel(AppPanel::from(panel), SplitAxis::Columns);
            }
            Err(error) => self.message(Severity::Error, "runs", error),
        }
    }

    fn run_management_provider(
        &self,
        store: Arc<teal_agent::RuntimeStore>,
    ) -> teal::PersistentRunManagement {
        let permissions: Arc<dyn teal_agent::PermissionProvider> = self.agent_permissions.clone();
        let driver = Arc::new(BenchRunDriver {
            workspace: self.workspace.clone(),
            store: Arc::clone(&store),
            tools: Arc::clone(&self.workspace_tools),
            access: self.model_access.clone(),
            model_calls_enabled: self.connectivity
                != teal_workbench_services::agent_algorithms::ConnectivityMode::Offline,
            permissions: Arc::clone(&permissions),
            knowledge: Arc::clone(&self.agent_knowledge),
            memory: Arc::clone(&self.agent_memory),
        });
        teal::PersistentRunManagement::new(self.workspace.clone(), store, permissions, driver)
    }
}

fn run_context(operation: &str) -> teal_agent::RunManagementContext {
    teal_agent::RunManagementContext {
        action: teal_agent::ActionContext {
            run_id: "teal-bench/runs".into(),
            task_id: format!("runs/{operation}"),
            generation: 0,
            policy_version: String::new(),
        },
        current_run: None,
    }
}

fn provider_error(error: teal_agent::RuntimeStoreError) -> teal_agent::RunManagementError {
    teal_agent::RunManagementError::Provider(error.to_string())
}

fn render_list(runs: Vec<teal_agent::RunSummary>) -> String {
    let mut output = String::from("# Persistent runs\n\n");
    if runs.is_empty() {
        output.push_str("No persistent runs are recorded in this workspace.\n");
        return output;
    }
    output.push_str("| Run | State | Bytecode | Updated |\n| --- | --- | ---: | ---: |\n");
    for run in runs {
        output.push_str(&format!(
            "| `{}` | `{:?}` | {} | {} |\n",
            run.id, run.state, run.program_version, run.updated_at_ms
        ));
    }
    output
}

fn render_details(details: teal_agent::RunDetails) -> String {
    let value = serde_json::from_str::<serde_json::Value>(&details.details_json)
        .unwrap_or(serde_json::Value::Null);
    let graph_agents = value
        .pointer("/graph/agents")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    let graph_tasks = value
        .pointer("/graph/tasks")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    format!(
        "# Run `{}`\n\n- State: `{:?}`\n- Program: `{}`\n- Bytecode: {}\n- Attention: {}\n- Interactions: {}\n- External operations: {}\n- Managed effects: {}\n- Graph agents: {}\n- Graph tasks: {}\n",
        details.summary.id,
        details.summary.state,
        details.summary.program_digest,
        details.summary.program_version,
        if details.attention.is_empty() {
            "none"
        } else {
            &details.attention
        },
        details.interactions.len(),
        details.external_operations,
        details.managed_effects,
        graph_agents,
        graph_tasks,
    )
}
