//! 阶段任务运行与终态重投递，业务阶段由扩展事件处理器决定。
use super::*;
use teal_agent::{ExternalOperationState, ManagedEffectState, ModelToolState, RunState, RuntimeStore};
use teal_model::Cancellation;

impl App {
    pub(in crate::app) fn extension_owns_workspace(&self) -> bool {
        self.extensions.loaded.values().any(|l| {
            l.package.manifest.exclusive_workspace
                && (l.instance.values.get("active").is_some_and(|v| v == "true")
                    || l.instance.tasks.values().any(|t| !t.delivered))
        })
    }

    pub(super) fn start_extension_task(&mut self, extension: &str, key: &str) {
        if self.extensions.running.contains_key(key)
            || self.extensions.authenticating.contains_key(key)
        {
            return;
        }
        let requirements = match self.extension_task_authentication_requirements(extension, key) {
            Ok(requirements) => requirements,
            Err(error) => {
                self.message(Severity::Error, "extension.task", error);
                return;
            }
        };
        if requirements.is_empty() {
            self.start_authenticated_extension_task(extension, key, Vec::new());
            return;
        }
        let capability = if requirements
            .iter()
            .all(|requirement| self.local_model_providers.contains(&requirement.provider))
        {
            teal_workbench_services::agent_algorithms::ConnectivityCapability::LocalModel
        } else {
            teal_workbench_services::agent_algorithms::ConnectivityCapability::RemoteModel
        };
        if let Err(error) = self.connectivity.require(capability) {
            self.message(Severity::Error, "extension.task", error.to_string());
            return;
        }

        let model_requirements = requirements
            .iter()
            .map(|requirement| {
                teal_model::ModelAuthenticationRequirement::parse(
                    requirement.provider.clone(),
                    requirement.profile.clone(),
                    requirement.model.clone(),
                )
            })
            .collect::<Result<Vec<_>, _>>();
        let model_requirements = match model_requirements {
            Ok(requirements) => requirements,
            Err(error) => {
                self.message(Severity::Error, "extension.task", error.to_string());
                return;
            }
        };

        let cancellation = Cancellation::new();
        self.extensions
            .authenticating
            .insert(key.to_owned(), (extension.to_owned(), cancellation.clone()));
        let model_access = self.model_access.clone();
        let output = self.runtime_tx.clone();
        let extension = extension.to_owned();
        let key = key.to_owned();
        self.tasks.spawn(async move {
            let result = model_access
                .prepare_models(model_requirements, cancellation)
                .await
                .map(|profiles| {
                    requirements
                        .into_iter()
                        .zip(profiles)
                        .map(
                            |(requirement, profile)| teal_agent::AuthenticatedAgentProfile {
                                requirement,
                                context: profile.authentication_context().clone(),
                                account: profile.session_context().account.clone(),
                            },
                        )
                        .collect()
                })
                .map_err(|error| error.to_string());
            let _ = output
                .send(RuntimeEvent::ExtensionAuthenticationReady {
                    extension,
                    key,
                    result,
                })
                .await;
        });
    }

    pub(in crate::app) fn finish_extension_authentication(
        &mut self,
        extension: &str,
        key: &str,
        result: Result<Vec<teal_agent::AuthenticatedAgentProfile>, String>,
    ) {
        let Some((owner, cancellation)) = self.extensions.authenticating.remove(key) else {
            return;
        };
        if owner != extension || cancellation.is_cancelled() {
            return;
        }
        // 异步认证结果回来时，先核对批准所绑定的源码是否仍有效；即使认证自身失败，
        // 也不能让旧批准被后续重试继续使用，或把源码失效误报成模型配置问题。
        let current = self
            .extensions
            .loaded
            .get(extension)
            .ok_or_else(|| "Unknown extension".to_owned())
            .and_then(|loaded| loaded.package.is_current().map_err(err));
        match current {
            Ok(true) => {}
            Ok(false) => {
                self.message(
                    Severity::Error,
                    "extension.task",
                    "Extension source changed; refusing to execute the old approval",
                );
                return;
            }
            Err(error) => {
                self.message(Severity::Error, "extension.task", error);
                return;
            }
        }
        match result {
            Ok(authenticated) => {
                self.start_authenticated_extension_task(extension, key, authenticated)
            }
            Err(error) => self.message(Severity::Error, "extension.task", error),
        }
    }

    fn start_authenticated_extension_task(
        &mut self,
        extension: &str,
        key: &str,
        authenticated: Vec<teal_agent::AuthenticatedAgentProfile>,
    ) {
        let result = self.prepare_extension_task(extension, key, &authenticated);
        let (prepared, mut host, store, services, grant, cancellation) = match result {
            Ok(value) => value,
            Err(error) => {
                self.message(Severity::Error, "extension.task", error);
                return;
            }
        };
        let model_access = self.model_access.clone();
        let output = self.runtime_tx.clone();
        let extension = extension.to_owned();
        let key = key.to_owned();
        let (events_tx, events_rx) = tokio::sync::mpsc::channel(64);
        let owner = extension.clone();
        self.tasks
            .forward(events_rx, self.runtime_tx.clone(), move |event| {
                RuntimeEvent::ExtensionActivity {
                    extension: owner.clone(),
                    event,
                }
            });
        let services = services.with_events(events_tx);
        self.extensions
            .running
            .insert(key.clone(), (extension.clone(), cancellation.clone()));
        self.tasks.spawn(async move {
            let result = async {
                let execution = prepared
                    .bind_with_access(&mut host, model_access, store, grant, services)
                    .map_err(err)?;
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => Err("Extension task interrupted; inspect its run before resuming".into()),
                    result = execution.execute(&mut host) => result.map(|_| ()).map_err(err),
                }
            }.await;
            let _ = output.send(RuntimeEvent::ExtensionTaskFinished { extension, key, result }).await;
        });
    }

    fn extension_task_authentication_requirements(
        &self,
        extension: &str,
        key: &str,
    ) -> Result<Vec<teal_agent::AgentAuthenticationRequirement>, String> {
        let loaded = self
            .extensions
            .loaded
            .get(extension)
            .ok_or("Unknown extension")?;
        let task = loaded
            .instance
            .tasks
            .get(key)
            .ok_or("Unknown task request")?;
        if let Some(run_id) = &task.run_id {
            let store = RuntimeStore::connect(&self.workspace).map_err(err)?;
            let run = store.run(run_id).map_err(err)?.ok_or("Run is missing")?;
            if run.program_version != teal::BYTECODE_FORMAT_VERSION {
                return Err(format!(
                    "Run `{run_id}` uses bytecode v{}, but this runtime supports only v{}; run state remains unchanged",
                    run.program_version,
                    teal::BYTECODE_FORMAT_VERSION,
                ));
            }
            let bytes = store
                .program(run_id)
                .map_err(err)?
                .ok_or("Run program is missing")?;
            let program = teal_agent::AgentProgram::from_bytes(bytes).map_err(err)?;
            let bindings = store
                .execution_bindings(run_id)
                .map_err(err)?
                .unwrap_or_default();
            return program.authentication_requirements(&bindings).map_err(err);
        }

        let entry = loaded
            .package
            .entry("task", &task.task)
            .ok_or("Task is not declared")?;
        let model_agents = teal::inspect_agent_program(entry.program())
            .map_err(err)?
            .model_agents;
        if model_agents.is_empty() {
            return Ok(Vec::new());
        }
        let agent_name = loaded
            .config
            .get(&format!("agent.{}", task.task))
            .filter(|value| !value.is_empty())
            .or_else(|| loaded.config.get("agent").filter(|value| !value.is_empty()))
            .or(self.active_agent.as_ref())
            .ok_or("Configure a named Bench Agent before starting model tasks")?;
        let (_, settings) = self
            .agent_settings
            .get(agent_name)
            .ok_or("Configured Bench Agent is missing")?;
        Ok(model_agents
            .into_iter()
            .map(|agent| teal_agent::AgentAuthenticationRequirement {
                agent,
                provider: settings.provider.clone(),
                profile: settings.profile.clone(),
                model: settings.model.clone(),
            })
            .collect())
    }

    #[allow(
        clippy::type_complexity,
        reason = "prepared execution retains explicit host and authorization ownership"
    )]
    fn prepare_extension_task(
        &mut self,
        extension: &str,
        key: &str,
        authenticated: &[teal_agent::AuthenticatedAgentProfile],
    ) -> Result<
        (
            teal::PreparedExecution,
            teal::NativeHostRuntime,
            Arc<RuntimeStore>,
            teal::RuntimeServices,
            Arc<teal::HostExecutionGrant>,
            Cancellation,
        ),
        String,
    > {
        if self.agent_app_tx.is_some() {
            return Err("A native Task already owns the workspace".into());
        }
        if self
            .extensions
            .running
            .values()
            .any(|(owner, _)| owner != extension)
        {
            return Err("Another extension task owns the workspace".into());
        }
        let l = self
            .extensions
            .loaded
            .get(extension)
            .ok_or("Unknown extension")?;
        if !l.package.is_current().map_err(err)? {
            return Err("Extension source changed; refusing to execute the old approval".into());
        }
        let task = l
            .instance
            .tasks
            .get(key)
            .ok_or("Unknown task request")?
            .clone();
        for id in &task.approval_ids {
            let receipt = l
                .instance
                .approvals
                .get(id)
                .ok_or("Required approval is missing")?;
            if receipt.approved != Some(true) {
                return Err("Required approval is not accepted".into());
            }
            for artifact in &receipt.artifacts {
                if artifact_receipt(&self.workspace, &artifact.path)? != *artifact {
                    return Err("Approved input changed; revise the affected artifact and request approval again".into());
                }
            }
        }
        let entry = l
            .package
            .entry("task", &task.task)
            .ok_or("Task is not declared")?
            .clone();
        if let Some(run) = &task.run_id {
            let store = RuntimeStore::connect(&self.workspace).map_err(err)?;
            if store
                .run(run)
                .map_err(err)?
                .is_some_and(|r| !matches!(r.state, RunState::Running | RunState::NeedsAttention))
            {
                return Err("Task is terminal; use resume to deliver its existing result".into());
            }
        }
        let mut bindings = if let Some(run_id) = &task.run_id {
            RuntimeStore::connect(&self.workspace)
                .map_err(err)?
                .execution_bindings(run_id)
                .map_err(err)?
                .unwrap_or_default()
        } else {
            teal::ExecutionBindings::default()
        };
        let model_agents = teal::inspect_agent_program(entry.program())
            .map_err(err)?
            .model_agents;
        if task.run_id.is_none() && !model_agents.is_empty() {
            let agent_name = l
                .config
                .get(&format!("agent.{}", task.task))
                .filter(|v| !v.is_empty())
                .or_else(|| l.config.get("agent").filter(|v| !v.is_empty()))
                .or(self.active_agent.as_ref())
                .ok_or("Configure a named Bench Agent before starting model tasks")?;
            let (_, settings) = self
                .agent_settings
                .get(agent_name)
                .ok_or("Configured Bench Agent is missing")?;
            let compaction_profile = settings
                .compaction_profile
                .as_ref()
                .map(|profile| teal_model::ProfileName::new(profile.clone()))
                .transpose()
                .map_err(|error| error.to_string())?;
            for agent in model_agents {
                let expected = teal_agent::AgentAuthenticationRequirement {
                    agent: agent.clone(),
                    provider: settings.provider.clone(),
                    profile: settings.profile.clone(),
                    model: settings.model.clone(),
                };
                let resolved = authenticated
                    .iter()
                    .find(|resolved| resolved.requirement.agent == agent)
                    .ok_or("Extension authentication preflight result is incomplete")?;
                if resolved.requirement != expected {
                    return Err(
                        "Extension Agent configuration changed during authentication preflight"
                            .into(),
                    );
                }
                bindings.source_agents.insert(
                    agent,
                    teal::SourceAgentBinding {
                        provider: resolved.context.provider.to_string(),
                        profile: resolved.context.profile.to_string(),
                        model: resolved.requirement.model.clone(),
                        compaction_profile: compaction_profile.clone(),
                        instructions: settings.instructions.clone(),
                    },
                );
            }
        } else if task.run_id.is_some() {
            for (agent, binding) in &bindings.source_agents {
                let resolved = authenticated
                    .iter()
                    .find(|resolved| resolved.requirement.agent == *agent)
                    .ok_or("Saved extension authentication preflight result is incomplete")?;
                if resolved.context.provider.as_str() != binding.provider
                    || resolved.context.profile.as_str() != binding.profile
                    || resolved.requirement.model != binding.model
                {
                    return Err(
                        "Saved extension Agent binding changed during authentication preflight"
                            .into(),
                    );
                }
            }
        }
        let context = serde_json::to_vec(&(
            &l.package.fingerprint,
            &l.config,
            &task.state,
            &task.input,
            &bindings,
        ))
        .map_err(err)?;
        if let Some(state_key) = l.package.manifest.task_write_state.get(&task.task) {
            let paths: Vec<String> = serde_json::from_str(
                task.state
                    .get(state_key)
                    .ok_or("Task write scope has not been approved")?,
            )
            .map_err(err)?;
            hosts::validate_write_scope(&self.workspace, &l.instance, &paths)?;
        }
        let store = Arc::new(RuntimeStore::open(&self.workspace).map_err(err)?);
        let prepared =
            teal::prepare_execution(entry.program(), &store, key, &sha256(&context), bindings)
                .map_err(err)?;
        let event = ExtensionEvent {
            id: key.into(),
            kind: "task".into(),
            name: task.task.clone(),
            fields: BTreeMap::from([("input".into(), task.input)]),
        };
        let mut snapshot = l.instance.clone();
        snapshot.values = task.state.clone();
        let call = ExtensionCall::new(
            snapshot,
            event,
            l.config.clone(),
            l.package.manifest.tasks.keys().cloned().collect(),
        )
        .with_result_store(store.clone(), prepared.run_id.clone());
        let mut registry = teal::HostRegistry::standard();
        call.register(&mut registry).map_err(err)?;
        if let Some(state_key) = l.package.manifest.task_write_state.get(&task.task) {
            let paths: Vec<String> = serde_json::from_str(
                task.state
                    .get(state_key)
                    .ok_or("Task write scope has not been approved")?,
            )
            .map_err(err)?;
            hosts::validate_write_scope(&self.workspace, &l.instance, &paths)?;
            self.workspace_tools
                .register_scoped_script_hosts(&mut registry, &paths)
                .map_err(err)?;
        } else {
            self.workspace_tools
                .register_script_hosts(&mut registry)
                .map_err(err)?;
        }
        hosts::register(
            &mut registry,
            &self.workspace,
            &l.instance,
            &l.config,
            &self.agent_permissions,
            &prepared.run_id,
        )
        .map_err(err)?;
        if !l.package.is_current().map_err(err)? {
            return Err("Extension source changed; refusing to execute the old approval".into());
        }
        let grant = grant(&l.package, &task.task, true, &self.workspace);
        let services = teal::RuntimeServices::new()
            .with_grant(grant.clone())
            .with_hooks(Arc::new(self.agent_hooks()?))
            .with_permissions(self.agent_permissions.clone())
            .with_memory(self.agent_memory.clone())
            .with_skills(self.agent_skills.clone());
        let host = teal::NativeHostRuntime::builder()
            .trusted()
            .trusted()
            .trusted()
            .trusted()
            .standard_registry(registry)
            .authorization(grant.clone())
            .echo(false)
            .dylib_policy(teal::DylibPolicy::Deny)
            .build()
            .map_err(err)?;
        host.install_service(tracing::Span::current())
            .map_err(err)?;
        let l = self
            .extensions
            .loaded
            .get_mut(extension)
            .expect("known extension");
        let mut next = l.instance.clone();
        next.tasks.get_mut(key).expect("known task").run_id = Some(prepared.run_id.clone());
        // 先保存引用再开始 VM；失败时不会留下已经运行但无法定位的阶段。
        next.save(&l.path).map_err(err)?;
        l.instance = next;
        Ok((prepared, host, store, services, grant, Cancellation::new()))
    }

    pub(in crate::app) fn finish_extension_task(
        &mut self,
        extension: &str,
        key: &str,
        result: Result<(), String>,
    ) {
        self.extensions.running.remove(key);
        if let Err(error) = result {
            self.message(Severity::Warning, "extension.task", error);
        }
        if let Err(error) = self.deliver_extension_task(extension, key) {
            self.message(Severity::Warning, "extension.task", error);
        }
    }

    fn deliver_extension_task(&mut self, extension: &str, key: &str) -> Result<(), String> {
        let l = self
            .extensions
            .loaded
            .get(extension)
            .ok_or("Extension was unloaded")?;
        let task = l.instance.tasks.get(key).ok_or("Unknown task")?;
        if task.delivered {
            return Ok(());
        }
        let run_id = task.run_id.as_ref().ok_or("Task is not prepared")?;
        let store = RuntimeStore::connect(&self.workspace).map_err(err)?;
        let run = store.run(run_id).map_err(err)?.ok_or("Run is missing")?;
        if matches!(run.state, RunState::Running | RunState::NeedsAttention) {
            return Err(format!(
                "Run {run_id} needs explicit resume or reconciliation"
            ));
        }
        let saved = store
            .prepared_run(run_id)
            .map_err(err)?
            .ok_or("Task metadata is missing")?;
        let success = run.state == RunState::Completed && saved.result.is_some();
        let callback = task
            .callback
            .clone()
            .or_else(|| {
                l.package.manifest.modes.keys().next().map(|name| {
                    teal_workbench_automation::extension::ExtensionCallback {
                        kind: "mode".into(),
                        name: name.clone(),
                    }
                })
            })
            .ok_or("Task has no result callback")?;
        let event = ExtensionEvent {
            id: format!("task-result:{key}"),
            kind: callback.kind.clone(),
            name: callback.name.clone(),
            fields: BTreeMap::from([
                (
                    "action".into(),
                    if success {
                        "task_completed"
                    } else {
                        "task_failed"
                    }
                    .into(),
                ),
                ("task".into(), task.task.clone()),
                ("task_key".into(), key.into()),
                ("run_id".into(), run_id.clone()),
                (
                    "result".into(),
                    if success {
                        saved.result.unwrap_or_default()
                    } else {
                        format!("Run finished as {:?} without an accepted result", run.state)
                    },
                ),
            ]),
        };
        if !l.instance.contains_event(&event).map_err(err)?
            && !self
                .extensions
                .queue
                .iter()
                .any(|job| job.event.id == event.id)
        {
            let panel = if callback.kind == "panel" {
                self.panels
                    .iter()
                    .find(|(_, panel)| panel.as_custom().is_some_and(|p| p.name == callback.name))
                    .map(|(id, _)| id)
            } else {
                None
            };
            self.extensions.queue.push_back(Job {
                extension: extension.into(),
                event,
                panel,
            });
            self.pump_extensions();
        }
        Ok(())
    }

    pub(super) fn resume_extension(&mut self, extension: &str) {
        self.apply_extension_controls(extension, None);
        let keys = self
            .extensions
            .loaded
            .get(extension)
            .map(|l| {
                l.instance
                    .tasks
                    .values()
                    .filter(|t| !t.delivered)
                    .map(|t| t.key.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for key in keys {
            let terminal = self
                .extensions
                .loaded
                .get(extension)
                .and_then(|l| l.instance.tasks.get(&key))
                .and_then(|t| t.run_id.as_ref())
                .and_then(|run| {
                    RuntimeStore::connect(&self.workspace)
                        .ok()
                        .and_then(|s| s.run(run).ok().flatten())
                })
                .is_some_and(|r| !matches!(r.state, RunState::Running | RunState::NeedsAttention));
            if terminal {
                if let Err(error) = self.deliver_extension_task(extension, &key) {
                    self.message(Severity::Error, "extension.task", error);
                }
            } else {
                self.start_extension_task(extension, &key);
            }
        }
    }
    pub(super) fn stop_extension(&mut self, extension: &str) {
        let authenticating = self
            .extensions
            .authenticating
            .iter()
            .filter(|(_, (owner, _))| owner == extension)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in authenticating {
            if let Some((_, cancellation)) = self.extensions.authenticating.remove(&key) {
                cancellation.cancel();
            }
        }
        for (owner, cancellation) in self.extensions.running.values() {
            if owner == extension {
                cancellation.cancel();
            }
        }
        self.message(
            Severity::Info,
            "extension",
            "Cancellation requested; unknown operations will not be replayed",
        );
    }

    pub(super) fn apply_extension_controls(
        &mut self,
        extension: &str,
        selected: Option<&std::collections::BTreeSet<String>>,
    ) {
        use teal_workbench_automation::extension::ExtensionTaskControl;
        let controls = self
            .extensions
            .loaded
            .get(extension)
            .map(|l| {
                l.instance
                    .tasks
                    .values()
                    .filter(|t| selected.is_none_or(|keys| keys.contains(&t.key)))
                    .filter_map(|t| t.control.map(|c| (t.key.clone(), t.run_id.clone(), c)))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for (key, run, control) in controls {
            if matches!(control, ExtensionTaskControl::Cancel)
                && let Some(run_id) = run.as_deref()
            {
                let unresolved = (|| -> Result<bool, String> {
                    let store = RuntimeStore::connect(&self.workspace).map_err(err)?;
                    let record = store.run(run_id).map_err(err)?.ok_or("Run is missing")?;
                    if record.state != RunState::NeedsAttention {
                        return Ok(false);
                    }
                    let external = store
                        .operations(run_id)
                        .map_err(err)?
                        .iter()
                        .any(|operation| {
                            matches!(
                                operation.state,
                                ExternalOperationState::Prepared | ExternalOperationState::Unknown
                            )
                        });
                    let tools = store
                        .model_tool_operations(run_id)
                        .map_err(err)?
                        .iter()
                        .any(|operation| {
                            matches!(
                                operation.state,
                                ModelToolState::Prepared | ModelToolState::Unknown
                            )
                        });
                    let effects =
                        store
                            .managed_effects(run_id)
                            .map_err(err)?
                            .iter()
                            .any(|effect| {
                                matches!(
                                    effect.state,
                                    ManagedEffectState::Prepared | ManagedEffectState::Unknown
                                )
                            });
                    Ok(external || tools || effects)
                })();
                match unresolved {
                    Ok(true) => {
                        // 保留控制请求，完成对账后可按同一请求继续取消。
                        self.message(
                            Severity::Warning,
                            "extension",
                            "Run requires reconciliation before cancellation",
                        );
                        continue;
                    }
                    Err(error) => {
                        self.message(Severity::Error, "extension", error);
                        continue;
                    }
                    Ok(false) => {}
                }
            }
            match control {
                ExtensionTaskControl::Cancel => {
                    if let Some((_, token)) = self.extensions.authenticating.remove(&key) {
                        token.cancel();
                    } else if let Some((_, token)) = self.extensions.running.get(&key) {
                        token.cancel();
                    } else if let Some(run) = run {
                        let cancelled = (|| -> Result<(), String> {
                            let store =
                                Arc::new(RuntimeStore::connect(&self.workspace).map_err(err)?);
                            let record = store.run(&run).map_err(err)?.ok_or("Run is missing")?;
                            if matches!(record.state, RunState::Running | RunState::NeedsAttention)
                            {
                                store
                                    .claim_run(&run)
                                    .map_err(err)?
                                    .set_run_state(&run, RunState::Cancelled)
                                    .map_err(err)?;
                            }
                            Ok(())
                        })();
                        if let Err(error) = cancelled {
                            self.message(Severity::Error, "extension", error);
                            continue;
                        }
                    }
                }
                ExtensionTaskControl::Resume => {
                    if !self.extensions.running.contains_key(&key)
                        && !self.extensions.authenticating.contains_key(&key)
                    {
                        self.start_extension_task(extension, &key);
                    }
                }
            }
            if let Some(l) = self.extensions.loaded.get_mut(extension) {
                if let Some(t) = l.instance.tasks.get_mut(&key) {
                    t.control = None;
                }
                if let Err(error) = l.instance.save(&l.path) {
                    self.message(Severity::Error, "extension", error.to_string());
                }
            }
        }
    }

    pub(in crate::app) fn extension_activity(
        &mut self,
        extension: &str,
        activity: teal_agent::RuntimeAgentEvent,
    ) {
        if !self.extensions.loaded.contains_key(extension) {
            return;
        }
        let run_id = activity.context.run_id.clone();
        let event = activity.event.clone();
        let graph_event = event.clone();
        let text = match &event {
            teal_agent::AgentEvent::TextDelta { delta, .. } => delta.clone(),
            teal_agent::AgentEvent::ToolStarted(call) => format!("\nTool: {}\n", call.name),
            teal_agent::AgentEvent::WaitingForInput { request_id } => {
                if self.pending_overlay.is_none() {
                    self.open_durable_question(&[run_id.clone(), request_id.clone()]);
                }
                format!("\nWaiting for input; then use :extension resume {extension}\n")
            }
            teal_agent::AgentEvent::Failed(message) => format!("\n{message}\n"),
            _ => return,
        };
        let buffer = self
            .extensions
            .activity
            .entry(extension.into())
            .or_default();
        if buffer.len() + text.len() > 16384 {
            buffer.clear();
        }
        buffer.push_str(&text);
        if let Some(panel) = self.agent_panel
            && let Some(output) = self
                .panels
                .get_mut(panel)
                .and_then(|panel| panel.as_text_mut())
        {
            match event {
                teal_agent::AgentEvent::TextDelta { delta, .. } => {
                    output.append_agent_markdown(&delta)
                }
                teal_agent::AgentEvent::ToolStarted(call) => {
                    output.push_agent_block(crate::presentation::AgentBlock::ToolCall {
                        name: format!("{extension}:{}", call.name),
                        arguments: call.arguments.to_string(),
                    })
                }
                teal_agent::AgentEvent::WaitingForInput { request_id } => {
                    output.push_agent_block(crate::presentation::AgentBlock::Status(format!(
                        "[Extension {extension}] waiting for input {request_id}"
                    )))
                }
                teal_agent::AgentEvent::Failed(message) => {
                    output.push_agent_block(crate::presentation::AgentBlock::Diagnostic {
                        summary: format!("Extension {extension} failed"),
                        detail: message,
                    })
                }
                _ => {}
            }
        }
        if matches!(
            graph_event,
            teal_agent::AgentEvent::ToolStarted(_)
                | teal_agent::AgentEvent::Completed
                | teal_agent::AgentEvent::Failed(_)
                | teal_agent::AgentEvent::WaitingForInput { .. }
        ) && let Ok(store) = teal_agent::RuntimeStore::connect(&self.workspace)
            && let Ok(Some(snapshot)) = store.graph_snapshot(&run_id)
        {
            self.append_agent_graph(run_id, snapshot);
        }
    }
}
