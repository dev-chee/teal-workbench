//! 交互脚本、钩子、自定义面板和宿主桥接的根应用调度。
//!
//! 串行作业保存入口、来源分类和捕获快照；脚本执行后才在应用线程应用桥接动作。
//! 来源分类与工作区信任不能互相替代；显式 source 的边界见工作台手册与包 README。

use super::{
    App, AppPanel, Arc, BridgeAction, BridgeState, CommandRegistry, Duration, HashMap,
    HostCallSignature, HostError, HostValue, Instant, Mutex, PanelId, Path, PathBuf, RuntimeEvent,
    RuntimeType, ScriptKind, Severity, SplitAxis, TrustDecision, TrustStore,
};
use std::collections::{BTreeMap, VecDeque};
use teal_workbench_automation::RuntimeSnapshot;

const HOOKS: &[&str] = &[
    "app.started",
    "app.stopping",
    "workspace.opened",
    "buffer.opened",
    "buffer.changed",
    "buffer.saved",
    "buffer.closed",
    "panel.focused",
    "diagnostics.changed",
    "lsp.started",
    "agent.completed",
    "agent.before_model",
    "agent.after_model",
    "agent.before_tool",
    "agent.after_tool",
    "agent.before_compaction",
    "agent.after_compaction",
    "agent.finished",
    "session.restored",
];

/// 注册入口及来源分类；重新加载时按脚本路径替换旧注册。
#[derive(Clone)]
pub(crate) struct Registration {
    pub(crate) path: PathBuf,
    /// 注册者用于重载清理，执行路径用于独立信任检查。
    pub(crate) owner: PathBuf,
    pub(crate) kind: ScriptKind,
}

impl Registration {
    /// 执行前重新解析目标，防止注册后文件被替换为指向受信任目录外的链接。
    pub(super) fn execution_target(&self) -> Result<(PathBuf, ScriptKind), String> {
        let path = self
            .path
            .canonicalize()
            .map_err(|error| format!("Unable to resolve script: {error}"))?;
        let kind = if self.kind == ScriptKind::UserConfig
            && !path.starts_with(self.owner.parent().unwrap_or(Path::new(".")))
        {
            ScriptKind::WorkspaceConfig
        } else {
            self.kind
        };
        Ok((path, kind))
    }
}

/// 在触发时捕获上下文的串行脚本作业，不从执行时的活动视图重新取值。
#[derive(Clone)]
struct Job {
    observation: crate::runtime_bridge::Context,
    registration: Registration,
    context: serde_json::Value,
    panel: Option<PanelId>,
    hook: Option<String>,
    activation: bool,
    approve: bool,
    /// 缓冲区标识、修订、显示路径和完整文本；替换结果须匹配这一修订。
    target: Option<(teal_workbench_core::BufferId, u64, String, String)>,
}

#[derive(Default)]
pub(crate) struct State {
    pub(crate) hooks: HashMap<String, Vec<Registration>>,
    pub(crate) panels: HashMap<String, Registration>,
    queue: VecDeque<Job>,
    active: Option<Job>,
    pub(crate) running: bool,
    pub(crate) changed: HashMap<teal_workbench_core::BufferId, (Instant, PathBuf)>,
}

pub(crate) struct Completed {
    pub(crate) snapshot: RuntimeSnapshot,
    pub(crate) trust: TrustStore,
}

impl App {
    pub(super) fn legacy_scripts_pending(&self) -> bool {
        self.scripting.active.is_some() || !self.scripting.queue.is_empty()
    }
    #[cfg(test)]
    pub(super) async fn settle_scripts(&mut self) {
        loop {
            self.pump_scripts();
            if self.scripting.active.is_none() {
                break;
            }
            let event = tokio::time::timeout(Duration::from_secs(10), self.runtime_rx.recv())
                .await
                .unwrap()
                .unwrap();
            self.handle_runtime_event(event).await;
        }
    }

    pub(super) fn note_buffer_change(&mut self, id: teal_workbench_core::BufferId, path: PathBuf) {
        if self
            .scripting
            .active
            .as_ref()
            .and_then(|job| job.hook.as_deref())
            != Some("buffer.changed")
        {
            self.scripting.changed.insert(id, (Instant::now(), path));
        }
    }

    pub(super) fn script_tick(&mut self) {
        let ids = self
            .panels
            .iter()
            .filter(|(_, panel)| panel.as_custom().is_some())
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        for id in ids {
            self.queue_panel_event(
                id,
                BTreeMap::from([
                    ("kind".into(), "tick".into()),
                    ("elapsed_ms".into(), "100".into()),
                ]),
            );
        }
    }

    pub(super) async fn stop_scripts(&mut self) {
        self.scripting.queue.clear();
        self.scripting.changed.clear();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while self.scripting.active.is_some() {
            match tokio::time::timeout_at(deadline, self.runtime_rx.recv()).await {
                Ok(Some(RuntimeEvent::ScriptFinished(result))) => self.finish_script(result),
                Ok(Some(event)) => self.handle_runtime_event(event).await,
                _ => return,
            }
        }
        self.fire_hook("app.stopping", serde_json::json!({}));
        loop {
            self.pump_scripts();
            if self.scripting.active.is_none() {
                break;
            }
            match tokio::time::timeout_at(deadline, self.runtime_rx.recv()).await {
                Ok(Some(RuntimeEvent::ScriptFinished(result))) => self.finish_script(result),
                Ok(Some(event)) => self.handle_runtime_event(event).await,
                _ => break,
            }
        }
    }

    pub(super) fn install_script_snapshot(
        &mut self,
        path: &Path,
        kind: ScriptKind,
        snapshot: RuntimeSnapshot,
    ) -> Result<(), String> {
        for hook in &snapshot.hooks {
            if !HOOKS.contains(&hook.name.as_str()) {
                return Err(format!("Unknown hook {}", hook.name));
            }
        }
        if let Some(policy) = snapshot.session_restore.as_deref() {
            self.configure_session_restore_policy(policy);
        }
        self.agent_settings.retain(|_, (owner, _)| owner != path);
        for settings in &snapshot.agents {
            self.agent_settings
                .insert(settings.name.clone(), (path.to_owned(), settings.clone()));
        }
        self.mcp_settings.retain(|_, (owner, _)| owner != path);
        for settings in &snapshot.mcp_servers {
            self.mcp_settings
                .insert(settings.name.clone(), (path.to_owned(), settings.clone()));
        }
        if let Some(active) = snapshot.active_agent.as_deref() {
            if !self.agent_settings.contains_key(active) {
                return Err(format!("Active Agent {active} has not registered yet"));
            }
            if self.agent_app_tx.is_some() {
                self.submit_source_agent_input(":quit".into());
            }
            self.active_agent = Some(active.to_owned());
            self.inline_completion = None;
        } else if self
            .active_agent
            .as_ref()
            .is_some_and(|name| !self.agent_settings.contains_key(name))
        {
            self.active_agent = None;
            self.inline_completion = None;
        }
        let owner = path.canonicalize().map_err(|error| error.to_string())?;
        let registration = |script_path: String| -> Result<Registration, String> {
            let requested = PathBuf::from(script_path);
            let target = if requested.is_absolute() {
                requested
            } else {
                owner.parent().unwrap_or(Path::new(".")).join(requested)
            }
            .canonicalize()
            .map_err(|error| format!("Unable to resolve registered script: {error}"))?;
            let directory = owner
                .parent()
                .unwrap_or(Path::new("."))
                .canonicalize()
                .map_err(|error| error.to_string())?;
            let target_kind = if kind == ScriptKind::UserConfig && target.starts_with(&directory) {
                ScriptKind::UserConfig
            } else {
                ScriptKind::WorkspaceConfig
            };
            Ok(Registration {
                path: target,
                owner: owner.clone(),
                kind: target_kind,
            })
        };
        // 先解析全部目标，避免路径错误留下半套命令或钩子注册。
        let commands = snapshot
            .commands
            .into_iter()
            .map(|command| Ok((command.name, registration(command.script_path)?)))
            .collect::<Result<Vec<_>, String>>()?;
        let hooks = snapshot
            .hooks
            .into_iter()
            .map(|hook| Ok((hook.name, registration(hook.script_path)?)))
            .collect::<Result<Vec<_>, String>>()?;
        let panels = snapshot
            .panels
            .into_iter()
            .map(|panel| Ok((panel.name, registration(panel.script_path)?)))
            .collect::<Result<Vec<_>, String>>()?;
        for registrations in self.scripting.hooks.values_mut() {
            registrations.retain(|r| r.owner != owner);
        }
        self.scripting.panels.retain(|_, r| r.owner != owner);
        let obsolete = self
            .script_commands
            .iter()
            .filter(|(_, registration)| registration.owner == owner)
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let builtins = CommandRegistry::standard();
        for name in obsolete {
            self.script_commands.remove(&name);
            self.commands.unregister(&name);
            if let Some(command) = builtins.resolve(&name) {
                self.commands.register(command.clone());
            }
        }
        for (name, registration) in commands {
            self.commands.register(teal_workbench_core::CommandSpec {
                name: name.clone(),
                aliases: Vec::new(),
                description: format!("Teal script {}", registration.path.display()),
                scope: teal_workbench_core::CommandScope::Window,
            });
            self.script_commands.insert(name, registration);
        }
        for (name, registration) in hooks {
            self.scripting
                .hooks
                .entry(name)
                .or_default()
                .push(registration);
        }
        for (name, registration) in panels {
            self.scripting.panels.insert(name, registration);
        }
        Ok(())
    }

    fn queue_script(&mut self, mut job: Job) {
        if self.scripting.queue.len() >= 128 {
            self.message(Severity::Warning, "teal", "Script event queue is full");
            return;
        }
        if let Some(hook) = &job.hook {
            if self.scripting.active.as_ref().and_then(|j| j.hook.as_ref()) == Some(hook) {
                return;
            }
            if self.scripting.queue.iter().any(|j| {
                j.hook.as_ref() == Some(hook) && j.registration.path == job.registration.path
            }) {
                return;
            }
        }
        if job.context.get("kind").and_then(serde_json::Value::as_str) == Some("tick")
            && self.scripting.queue.iter().any(|j| j.panel == job.panel)
        {
            return;
        }
        let buffer = job
            .context
            .get("path")
            .and_then(serde_json::Value::as_str)
            .and_then(|path| self.buffers.find(Path::new(path)))
            .or_else(|| {
                self.panels
                    .get(self.windows.focused())
                    .and_then(AppPanel::as_editor)
                    .map(|e| e.document().clone())
            });
        // 入队即冻结读取快照，避免排队期间切换面板或编辑文本改变脚本的操作对象。
        job.target = buffer.map(|buffer| {
            (
                buffer.id(),
                buffer.revision(),
                buffer
                    .path()
                    .map_or_else(String::new, |p| p.to_string_lossy().into_owned()),
                buffer.to_string(),
            )
        });
        let bytes = self
            .scripting
            .queue
            .iter()
            .filter_map(|job| job.target.as_ref().map(|target| target.3.len()))
            .sum::<usize>()
            + job.target.as_ref().map_or(0, |target| target.3.len());
        if bytes > 8 * 1024 * 1024 {
            self.message(
                Severity::Warning,
                "teal",
                "Script snapshot queue exceeds 8 MiB",
            );
            return;
        }
        self.scripting.queue.push_back(job);
    }

    pub(super) fn queue_activation(&mut self, path: &Path, kind: ScriptKind, approve: bool) {
        self.queue_script(Job {
            observation: crate::runtime_bridge::Context::with_span(tracing::info_span!(
                "teal.bench.script",
                operation.id = teal_workbench_services::observability::operation_id(),
                operation.result = tracing::field::Empty,
                otel.status_code = tracing::field::Empty
            )),
            registration: Registration {
                path: path.to_owned(),
                owner: path.canonicalize().unwrap_or_else(|_| path.to_owned()),
                kind,
            },
            context: serde_json::json!({"workspace":self.workspace}),
            panel: None,
            hook: None,
            activation: true,
            approve,
            target: None,
        });
    }

    pub(super) fn queue_script_path(&mut self, path: &Path, kind: ScriptKind) {
        self.queue_script(Job {
            observation: crate::runtime_bridge::Context::with_span(tracing::info_span!(
                "teal.bench.script",
                operation.id = teal_workbench_services::observability::operation_id(),
                operation.result = tracing::field::Empty,
                otel.status_code = tracing::field::Empty
            )),
            registration: Registration {
                path: path.to_owned(),
                owner: path.canonicalize().unwrap_or_else(|_| path.to_owned()),
                kind,
            },
            context: serde_json::json!({}),
            panel: None,
            hook: None,
            activation: false,
            approve: false,
            target: None,
        });
    }

    pub(super) fn fire_hook(&mut self, name: &str, context: serde_json::Value) {
        self.fire_extension_hook(name, &context);
        let context = std::sync::Arc::new(context);
        for registration in self.scripting.hooks.get(name).cloned().unwrap_or_default() {
            self.queue_script(Job {
                observation: crate::runtime_bridge::Context::with_span(tracing::info_span!(
                    "teal.bench.script",
                    operation.id = teal_workbench_services::observability::operation_id(),
                    operation.result = tracing::field::Empty,
                    otel.status_code = tracing::field::Empty
                )),
                registration,
                context: (*context).clone(),
                panel: None,
                hook: Some(name.to_owned()),
                activation: false,
                approve: false,
                target: None,
            });
        }
    }

    pub(super) fn queue_panel_event(&mut self, panel: PanelId, event: BTreeMap<String, String>) {
        if self.agent_app_view_panel == Some(panel) {
            if let Some(sender) = &self.agent_app_tx
                && let Err(error) = sender.try_send(super::agent_app::AgentAppInput::Ui(event))
            {
                self.message(Severity::Warning, "agent", error.to_string());
            }
            return;
        }
        if self.route_extension_panel(panel, event.clone()) {
            return;
        }
        let Some(name) = self
            .panels
            .get(panel)
            .and_then(AppPanel::as_custom)
            .map(|p| p.name.clone())
        else {
            return;
        };
        let Some(registration) = self.scripting.panels.get(&name).cloned() else {
            return;
        };
        self.queue_script(Job {
            observation: crate::runtime_bridge::Context::with_span(tracing::info_span!(
                "teal.bench.script",
                operation.id = teal_workbench_services::observability::operation_id(),
                operation.result = tracing::field::Empty,
                otel.status_code = tracing::field::Empty
            )),
            registration,
            context: serde_json::to_value(event).unwrap_or_default(),
            panel: Some(panel),
            hook: None,
            activation: false,
            approve: false,
            target: None,
        });
    }

    pub(super) fn open_custom_panel(&mut self, name: &str) {
        if self.open_extension_panel(name) {
            return;
        }
        if !self.scripting.panels.contains_key(name) {
            self.message(Severity::Error, "teal", format!("Unknown panel {name}"));
            return;
        }
        let panel = self.add_panel(
            crate::presentation::CustomPanel::new(name.to_owned()),
            SplitAxis::Columns,
        );
        self.queue_panel_event(panel, BTreeMap::from([("kind".into(), "mount".into())]));
    }

    pub(super) fn pump_scripts(&mut self) {
        let ready = self
            .scripting
            .changed
            .iter()
            .filter(|(_, (when, _))| when.elapsed() >= Duration::from_millis(100))
            .map(|(id, (_, path))| (*id, path.clone()))
            .collect::<Vec<_>>();
        for (id, path) in ready {
            self.scripting.changed.remove(&id);
            self.fire_hook("buffer.changed", serde_json::json!({"path":path}));
        }
        if self.scripting.active.is_some() || self.extensions.active {
            return;
        }
        let Some(job) = self.scripting.queue.pop_front() else {
            self.pump_extensions();
            return;
        };
        if let Ok(mut bridge) = self.bridge.lock() {
            bridge.pending.clear();
            bridge.target = job
                .target
                .as_ref()
                .map(|(id, revision, _, _)| (*id, *revision));
            bridge.path = job
                .target
                .as_ref()
                .map_or_else(String::new, |target| target.2.clone());
            bridge.content = job
                .target
                .as_ref()
                .map_or_else(String::new, |target| target.3.clone());
            bridge.panel = job.panel;
            bridge.panel_event = job
                .context
                .as_object()
                .map(|o| {
                    o.iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                v.as_str().map_or_else(|| v.to_string(), str::to_owned),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        self.scripting.active = Some(job.clone());
        let runtime = self.teal.clone();
        let tools = self.workspace_tools.clone();
        let mut trust = self.trust.clone();
        let output = self.runtime_tx.clone();
        let observation = job.observation.clone();
        self.tasks.spawn(observation.instrument(async move {
            let result = crate::runtime_bridge::spawn_blocking(move || {
                let (path, kind) = job.registration.execution_target()?;
                let prepared = runtime.prepare(&path, kind).map_err(|e| e.to_string())?;
                refresh_directory_trust(&mut trust, &path)?;
                if job.approve
                    && matches!(
                        runtime.trust_decision(&prepared, &trust),
                        TrustDecision::ApprovalRequired { .. }
                    )
                {
                    approve_script_snapshot(&mut trust, &prepared, &path)
                        .map_err(|e| e.to_string())?;
                }
                if matches!(
                    runtime.trust_decision(&prepared, &trust),
                    TrustDecision::ApprovalRequired { .. }
                ) {
                    return Err(
                        "Scripts and imported closures require approval; please use :source! path"
                            .into(),
                    );
                }
                let grant = tools.script_grant(prepared.capabilities());
                let snapshot = if job.activation {
                    runtime.execute_with_grant(prepared, &job.context.to_string(), grant)
                } else {
                    runtime.execute_legacy_event_with_grant(
                        prepared,
                        &job.context.to_string(),
                        grant,
                    )
                }
                .map_err(|e| e.to_string())?;
                Ok(Completed { snapshot, trust })
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|result| result);
            teal_workbench_services::observability::outcome(&result);
            let _ = output.send(RuntimeEvent::ScriptFinished(result)).await;
        }));
    }

    pub(super) fn finish_script(&mut self, result: Result<Completed, String>) {
        let Some(job) = self.scripting.active.clone() else {
            return;
        };
        match result {
            Ok(completed) => {
                self.trust = completed.trust;
                if job.activation
                    && let Err(error) = self.install_script_snapshot(
                        &job.registration.path,
                        job.registration.kind,
                        completed.snapshot,
                    )
                {
                    self.message(Severity::Error, "teal", error);
                    if let Ok(mut bridge) = self.bridge.lock() {
                        bridge.pending.clear();
                    }
                }
                self.apply_bridge_actions();
                self.process_buffer_changes();
            }
            Err(error) => {
                if let Ok(mut bridge) = self.bridge.lock() {
                    bridge.pending.clear();
                }
                self.message(Severity::Error, "teal", error);
            }
        }
        self.scripting.active = None;
        self.pump_scripts();
    }
}

impl App {
    pub(super) fn load_script(&mut self, path: &Path, kind: ScriptKind, approve: bool) {
        if self.scripting.running {
            self.queue_activation(path, kind, approve);
            return;
        }

        self.load_startup_script(path, kind, approve);
    }

    #[tracing::instrument(name = "teal.bench.script", skip_all, fields(operation.id = teal_workbench_services::observability::operation_id(), operation.result = "ok", otel.status_code = tracing::field::Empty))]
    fn load_startup_script(&mut self, path: &Path, kind: ScriptKind, approve: bool) {
        self.sync_bridge_state();
        match self.teal.prepare(path, kind) {
            Ok(prepared) => {
                if let Err(error) = refresh_directory_trust(&mut self.trust, path) {
                    self.message(Severity::Error, "config", error);
                    return;
                }
                let decision = self.teal.trust_decision(&prepared, &self.trust);
                if matches!(&decision, TrustDecision::ApprovalRequired { .. }) && !approve {
                    self.message(
                        Severity::Warning,
                        "config",
                        format!(
                            "{} requires approval; please run :source! {}",
                            path.display(),
                            path.display()
                        ),
                    );
                    return;
                }
                if approve
                    && matches!(&decision, TrustDecision::ApprovalRequired { .. })
                    && let Err(error) = approve_script_snapshot(&mut self.trust, &prepared, path)
                {
                    self.message(Severity::Error, "config", error.to_string());
                    return;
                }
                if matches!(
                    self.teal.trust_decision(&prepared, &self.trust),
                    TrustDecision::ApprovalRequired { .. }
                ) {
                    self.message(
                        Severity::Error,
                        "config",
                        "Script approval did not cover its imported closure",
                    );
                    return;
                }
                let context =
                    serde_json::json!({"workspace":self.workspace.to_string_lossy()}).to_string();
                let grant = self.workspace_tools.script_grant(prepared.capabilities());
                match self.teal.execute_with_grant(prepared, &context, grant) {
                    Ok(snapshot) => {
                        if let Err(error) = self.install_script_snapshot(path, kind, snapshot) {
                            self.message(Severity::Error, "teal", error);
                            return;
                        }
                        self.apply_bridge_actions();
                        self.message(
                            Severity::Info,
                            "config",
                            format!("Loaded {}", path.display()),
                        );
                    }
                    Err(error) => self.message(Severity::Error, "config", error.to_string()),
                }
            }
            Err(error) => self.message(Severity::Error, "config", error.to_string()),
        }
    }

    pub(super) fn execute_script_path(&mut self, path: &Path, kind: ScriptKind) {
        self.queue_script_path(path, kind);
    }

    pub(super) fn sync_bridge_state(&mut self) {
        let focused = self.windows.focused();
        let snapshot = self
            .panels
            .get(focused)
            .and_then(|panel| panel.as_editor())
            .map(|editor| {
                (
                    editor
                        .document()
                        .path()
                        .map_or_else(String::new, |path| path.to_string_lossy().into_owned()),
                    editor.document().to_string(),
                )
            })
            .unwrap_or_default();
        if let Ok(mut bridge) = self.bridge.lock() {
            bridge.target = self
                .panels
                .get(focused)
                .and_then(AppPanel::as_editor)
                .map(|e| (e.document().id(), e.document().revision()));
            bridge.path = snapshot.0;
            bridge.content = snapshot.1;
        }
    }

    pub(super) fn apply_bridge_actions(&mut self) {
        // 动作回调应用服务前，先释放桥接锁。
        let actions = self
            .bridge
            .lock()
            .map(|mut bridge| std::mem::take(&mut bridge.pending))
            .unwrap_or_default();

        for action in actions {
            match action {
                BridgeAction::Replace(id, revision, content) => {
                    if let Some(buffer) = self.buffers.get(id) {
                        if buffer.revision() != revision {
                            self.message(
                                Severity::Warning,
                                "teal",
                                "Expired buffer replacement discarded",
                            );
                            continue;
                        }

                        // 按脚本读取时的同一修订应用替换。
                        let change = teal_workbench_core::DocumentChange::between(
                            &buffer.to_string(),
                            &content,
                            revision,
                            revision + 1,
                        );
                        let _ = buffer.apply(change.transaction);
                    }
                }
                BridgeAction::Render(panel, json) => {
                    if let Some(custom) =
                        self.panels.get_mut(panel).and_then(AppPanel::as_custom_mut)
                        && let Err(error) = custom.ui.replace(&json)
                    {
                        self.message(Severity::Error, "teal.ui", error);
                    }
                }
                BridgeAction::Open(path) => self.open_path(path),
                BridgeAction::Message(message) => self.message(Severity::Info, "teal", message),
            }
        }
    }
}

fn approve_script_snapshot(
    trust: &mut TrustStore,
    prepared: &teal_workbench_automation::PreparedScript,
    path: &Path,
) -> Result<(), String> {
    let workspace = path.parent().unwrap_or_else(|| Path::new("."));
    let paths = teal_config::ConfigPaths::discover(Some(workspace)).map_err(|e| e.to_string())?;
    let _ = paths;
    trust
        .approve(
            prepared.path(),
            prepared.trust_contents(),
            prepared.capabilities().clone(),
        )
        .map_err(|e| e.to_string())
}

pub(super) fn refresh_directory_trust(
    trust: &mut TrustStore,
    workspace_path: &Path,
) -> Result<(), String> {
    let paths =
        teal_config::ConfigPaths::discover(Some(workspace_path)).map_err(|e| e.to_string())?;
    let _ = teal_security::load_declarations_paths(paths).map_err(|e| e.to_string())?;
    trust
        .clear_directory_approvals()
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn register_host<F>(
    registry: &mut teal::HostRegistry,
    module: &'static str,
    function: &'static str,
    signature: teal::HostCallSignature,
    capability: &'static str,
    callback: F,
) -> Result<(), teal::HostRegistryError>
where
    F: Fn(&[HostValue]) -> Result<HostValue, teal::HostError> + Send + Sync + 'static,
{
    registry.register(teal::HostFunctionDefinition::new(
        module,
        function,
        signature,
        capability,
        teal::ExternalCallRecovery::Unsupported,
        callback,
    ))
}

pub(super) fn register_automation_bridge(
    registry: &mut teal::HostRegistry,
    state: &Arc<Mutex<BridgeState>>,
) -> Result<(), teal::HostRegistryError> {
    let panel_event = state.clone();
    let network = Arc::clone(state);
    register_host(
        registry,
        "std.bench.panel",
        "event",
        signature([], RuntimeType::Map),
        "ui.panel",
        move |_| {
            let bridge = panel_event
                .lock()
                .map_err(|_| host_error("Panel bridge lock is poisoned"))?;
            Ok(HostValue::Map(
                bridge
                    .panel_event
                    .iter()
                    .map(|(key, value)| {
                        (
                            HostValue::String(key.clone()),
                            HostValue::String(value.clone()),
                        )
                    })
                    .collect(),
            ))
        },
    )?;
    let render = state.clone();
    register_host(
        registry,
        "std.bench.panel",
        "render",
        signature([RuntimeType::String], RuntimeType::None),
        "ui.panel",
        move |args| {
            let json = host_string(args, 0)?;
            if json.len() > 2 * 1024 * 1024 {
                return Err(host_error("Interface data exceeds 2 MiB"));
            }
            let mut bridge = render
                .lock()
                .map_err(|_| host_error("Panel bridge lock is poisoned"))?;
            let panel = bridge
                .panel
                .ok_or_else(|| host_error("Rendering requires custom panel events"))?;
            bridge.pending.push(BridgeAction::Render(panel, json));
            Ok(HostValue::None)
        },
    )?;
    let current_path = Arc::clone(state);
    register_host(
        registry,
        "std.bench.editor",
        "current_path",
        signature([], RuntimeType::String),
        "editor.read",
        move |_| {
            Ok(HostValue::String(
                current_path
                    .lock()
                    .map_err(|_| host_error("Editor bridge lock is poisoned"))?
                    .path
                    .clone(),
            ))
        },
    )?;
    let read_buffer = Arc::clone(state);
    register_host(
        registry,
        "std.bench.editor",
        "read_buffer",
        signature([], RuntimeType::String),
        "editor.read",
        move |_| {
            Ok(HostValue::String(
                read_buffer
                    .lock()
                    .map_err(|_| host_error("Editor bridge lock is poisoned"))?
                    .content
                    .clone(),
            ))
        },
    )?;
    let replace_buffer = Arc::clone(state);
    register_host(
        registry,
        "std.bench.editor",
        "replace_buffer",
        signature([RuntimeType::String], RuntimeType::None),
        "editor.write",
        move |arguments| {
            let content = host_string(arguments, 0)?;
            let mut bridge = replace_buffer
                .lock()
                .map_err(|_| host_error("Editor bridge lock is poisoned"))?;
            let (target, revision) = bridge
                .target
                .ok_or_else(|| host_error("no editor is focused"))?;
            bridge
                .pending
                .push(BridgeAction::Replace(target, revision, content));
            Ok(HostValue::None)
        },
    )?;
    let open = Arc::clone(state);
    register_host(
        registry,
        "std.bench.editor",
        "open",
        signature([RuntimeType::String], RuntimeType::None),
        "editor.write",
        move |arguments| {
            let path = host_string(arguments, 0)?;
            let mut bridge = open
                .lock()
                .map_err(|_| host_error("Editor bridge lock is poisoned"))?;
            let path = PathBuf::from(path);
            let path = if path.is_absolute() {
                path
            } else {
                bridge.workspace.join(path)
            };
            bridge.pending.push(BridgeAction::Open(path));
            Ok(HostValue::None)
        },
    )?;
    let message = Arc::clone(state);
    register_host(
        registry,
        "std.bench.editor",
        "message",
        signature([RuntimeType::String], RuntimeType::None),
        "ui.panel",
        move |arguments| {
            message
                .lock()
                .map_err(|_| host_error("Editor bridge lock is poisoned"))?
                .pending
                .push(BridgeAction::Message(host_string(arguments, 0)?));
            Ok(HostValue::None)
        },
    )?;
    register_host(
        registry,
        "std.bench.network",
        "get",
        signature([RuntimeType::String], RuntimeType::String),
        "network.http",
        move |arguments| {
            if network
                .lock()
                .map_err(|_| host_error("Editor bridge lock is poisoned"))?
                .offline
            {
                return Err(host_error(
                    "OfflineBlocked: Network is unavailable in offline mode",
                ));
            }
            let url = host_string(arguments, 0)?;
            let response = reqwest::blocking::Client::new()
                .get(url)
                .send()
                .map_err(|error| host_error(error.to_string()))?;
            if response
                .content_length()
                .is_some_and(|length| length > 5 * 1024 * 1024)
            {
                return Err(host_error("Network response exceeds 5 MiB"));
            }
            response
                .text()
                .map(HostValue::String)
                .map_err(|error| host_error(error.to_string()))
        },
    )?;
    let process = Arc::clone(state);
    register_host(
        registry,
        "std.bench.process",
        "run",
        signature([RuntimeType::String], RuntimeType::String),
        "process.spawn",
        move |arguments| {
            if process
                .lock()
                .map_err(|_| host_error("Editor bridge lock is poisoned"))?
                .offline
            {
                return Err(host_error(
                    "OfflineBlocked: Extension processes are unavailable in offline mode",
                ));
            }
            let command = host_string(arguments, 0)?;
            #[cfg(windows)]
            let output = std::process::Command::new("cmd.exe")
                .args(["/D", "/S", "/C", &command])
                .output();
            #[cfg(not(windows))]
            let output = std::process::Command::new("sh")
                .args(["-lc", &command])
                .output();
            let output = output.map_err(|error| host_error(error.to_string()))?;
            Ok(HostValue::String(format!(
                "exit={}\n{}{}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )))
        },
    )?;
    Ok(())
}

fn signature<const N: usize>(
    arguments: [RuntimeType; N],
    return_type: RuntimeType,
) -> HostCallSignature {
    HostCallSignature::new(arguments, return_type).expect("Static Teal Bench signature is valid")
}

fn host_string(arguments: &[HostValue], index: usize) -> Result<String, HostError> {
    match arguments.get(index) {
        Some(HostValue::String(value)) => Ok(value.clone()),
        _ => Err(host_error(format!("Parameter {index} must be string"))),
    }
}

fn host_error(message: impl Into<String>) -> HostError {
    HostError::new(std::io::ErrorKind::Other, message)
}
