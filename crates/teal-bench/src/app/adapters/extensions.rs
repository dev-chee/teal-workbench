//! 通用扩展装配与事件路由；业务流程全部由扩展源码定义。
use super::{App, AppPanel, PendingConfirmation, PendingOverlay, RuntimeEvent};
use crate::workbench::ui::ConfirmOverlay;
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
};
use teal_workbench_automation::extension::{
    ArtifactReceipt, ExtensionCall, ExtensionCallOutput, ExtensionEvent, ExtensionInstance,
    WorkbenchPluginPackage, sha256,
};
use teal_workbench_automation::{TrustDecision, WorkbenchAutomationRuntime};
use teal_workbench_core::{CommandSpec, PanelId, Severity, SplitAxis};
#[path = "extensions/hosts.rs"]
mod hosts;
#[path = "extensions/jobs.rs"]
mod jobs;

pub(crate) struct Loaded {
    pub(super) package: Arc<WorkbenchPluginPackage>,
    pub(super) instance: ExtensionInstance,
    pub(super) path: PathBuf,
    pub(super) config: BTreeMap<String, String>,
}
#[derive(Default)]
pub(crate) struct State {
    pub(super) loaded: BTreeMap<String, Loaded>,
    pub(super) queue: VecDeque<Job>,
    pub(super) active: bool,
    pub(super) authenticating: BTreeMap<String, (String, teal_model::Cancellation)>,
    pub(super) running: BTreeMap<String, (String, teal_model::Cancellation)>,
    pub(super) mode: String,
    pub(super) counter: u64,
    pub(super) pending_loads: VecDeque<Loaded>,
    activity: BTreeMap<String, String>,
}
pub(super) struct Job {
    extension: String,
    event: ExtensionEvent,
    panel: Option<PanelId>,
}
pub(crate) struct EventFinished {
    extension: String,
    event: ExtensionEvent,
    panel: Option<PanelId>,
    revision: u64,
    result: Result<ExtensionCallOutput, String>,
}

impl App {
    pub(super) fn load_extension(&mut self, path: PathBuf, overrides: BTreeMap<String, String>) {
        let path = if path.is_absolute() {
            path
        } else {
            self.workspace.join(path)
        };
        let workspace = self.workspace.clone();
        let state_root = self.paths.state.clone();
        let tools = self.workspace_tools.clone();
        let output = self.runtime_tx.clone();
        self.tasks.spawn(async move {
            let result =
                crate::runtime_bridge::spawn_blocking(move || -> Result<Loaded, String> {
                    let mut registry = teal::HostRegistry::standard();
                    tools.register_script_hosts(&mut registry).map_err(err)?;
                    hosts::register_preflight(&mut registry).map_err(err)?;
                    let package = WorkbenchPluginPackage::load(
                        &path,
                        &WorkbenchAutomationRuntime::for_extension(registry),
                    )
                    .map_err(err)?;
                    let id = format!(
                        "{}-{}",
                        package.manifest.id,
                        &sha256(workspace.to_string_lossy().as_bytes())[..16]
                    );
                    let state_path = state_root.join("extensions").join(format!("{id}.json"));
                    let instance = if state_path.exists() {
                        ExtensionInstance::load(&state_path).map_err(err)?
                    } else {
                        ExtensionInstance::new(&package.manifest.id, &id, &package.fingerprint)
                    };
                    let mut config = package.manifest.config.clone();
                    config.extend(overrides);
                    Ok(Loaded {
                        package: Arc::new(package),
                        instance,
                        path: state_path,
                        config,
                    })
                })
                .await
                .map_err(err)
                .and_then(|v| v);
            let _ = output.send(RuntimeEvent::ExtensionLoaded(result)).await;
        });
    }

    pub(super) fn extension_loaded(&mut self, result: Result<Loaded, String>) {
        match result {
            Err(error) => self.message(Severity::Error, "extension", error),
            Ok(loaded) => {
                if let Err(error) =
                    super::scripting::refresh_directory_trust(&mut self.trust, &self.workspace)
                {
                    self.message(Severity::Error, "extension", error);
                    return;
                }
                let p = &loaded.package;
                if self.trust.check_with_inputs(
                    &p.root.join("extension.toml"),
                    p.fingerprint.as_bytes(),
                    &p.operations,
                    p.source_inputs().keys().map(PathBuf::as_path),
                ) == TrustDecision::Trusted
                {
                    self.install_extension(loaded, false);
                } else {
                    self.extensions.pending_loads.push_back(loaded);
                    self.pump_extension_approvals();
                }
            }
        }
    }

    pub(super) fn install_extension(&mut self, mut loaded: Loaded, approve: bool) {
        let result = (|| -> Result<(), String> {
            let id = &loaded.package.manifest.id;
            if loaded.path.exists() {
                loaded.instance = ExtensionInstance::load(&loaded.path).map_err(err)?;
            }
            if self
                .extensions
                .running
                .values()
                .any(|(owner, _)| owner == id)
                || self.extensions.active
            {
                return Err("Wait for running extension calls before reloading".into());
            }
            if loaded.instance.fingerprint != loaded.package.fingerprint {
                // 源码更新后保留扩展业务值，但旧任务和审批绑定旧代码，不得恢复或复用。
                loaded.instance.fingerprint = loaded.package.fingerprint.clone();
                loaded.instance.tasks.clear();
                loaded.instance.approvals.clear();
                loaded.instance.revision = loaded.instance.revision.saturating_add(1);
            }
            for name in loaded.package.manifest.commands.keys() {
                let owned = self
                    .extensions
                    .loaded
                    .get(id)
                    .is_some_and(|old| old.package.manifest.commands.contains_key(name));
                if self.commands.resolve(name).is_some() && !owned {
                    return Err(format!("Command {name} is already registered"));
                }
            }
            for (other_id, other) in &self.extensions.loaded {
                if other_id != id
                    && (loaded
                        .package
                        .manifest
                        .modes
                        .keys()
                        .any(|n| other.package.manifest.modes.contains_key(n))
                        || loaded
                            .package
                            .manifest
                            .panels
                            .keys()
                            .any(|n| other.package.manifest.panels.contains_key(n)))
                {
                    return Err("Extension mode or panel is already registered".into());
                }
            }
            if loaded
                .package
                .manifest
                .panels
                .keys()
                .any(|n| self.scripting.panels.contains_key(n))
            {
                return Err("Panel belongs to a legacy script".into());
            }
            if approve {
                approve_extension_trust(&mut self.trust, &loaded.package, &self.workspace)?;
            }
            loaded.instance.save(&loaded.path).map_err(err)
        })();
        if let Err(error) = result {
            self.message(Severity::Error, "extension", error);
            return;
        }
        let id = loaded.package.manifest.id.clone();
        if let Some(old) = self.extensions.loaded.remove(&id) {
            for name in old.package.manifest.commands.keys() {
                self.commands.unregister(name);
            }
        }
        for name in loaded.package.manifest.commands.keys() {
            self.commands.register(CommandSpec {
                name: name.clone(),
                aliases: vec![],
                description: format!("Teal extension {id}"),
                scope: teal_workbench_core::CommandScope::Window,
            });
        }
        if let Ok(mode) = std::fs::read_to_string(self.extension_mode_path())
            && loaded.package.manifest.modes.contains_key(mode.trim())
        {
            self.extensions.mode = mode.trim().into();
        }
        self.extensions.loaded.insert(id.clone(), loaded);
        self.message(
            Severity::Info,
            "extension",
            format!("Loaded {id}; restored tasks require explicit resume"),
        );
    }

    pub(super) fn extension_command(&mut self, args: &[String]) {
        let action = args.first().map(String::as_str).unwrap_or("status");
        let id = args.get(1).map(String::as_str).unwrap_or("");
        match action {
            "load" if !id.is_empty() => self.load_extension(PathBuf::from(id), BTreeMap::new()),
            "reload" => {
                if let Some(l) = self.extensions.loaded.get(id) {
                    self.load_extension(l.package.root.clone(), l.config.clone());
                }
            }
            "unload" | "abandon" => {
                if self.extensions.active
                    || self
                        .extensions
                        .running
                        .values()
                        .any(|(owner, _)| owner == id)
                {
                    self.message(Severity::Warning, "extension", "Stop running tasks first");
                    return;
                }
                if let Some(l) = self.extensions.loaded.remove(id) {
                    self.extensions.queue.retain(|job| job.extension != id);
                    for name in l.package.manifest.commands.keys() {
                        self.commands.unregister(name);
                    }
                    if l.package.manifest.modes.contains_key(&self.extensions.mode) {
                        self.extensions.mode = "chat".into();
                    }
                    if action == "abandon"
                        && let Err(error) = std::fs::rename(
                            &l.path,
                            l.path.with_extension(format!(
                                "abandoned-{}.json",
                                teal_workbench_services::agent_algorithms::now_ms()
                            )),
                        )
                    {
                        self.message(Severity::Error, "extension", error.to_string());
                    }
                    self.message(
                        Severity::Info,
                        "extension",
                        format!("Extension {id}: {action}"),
                    );
                }
            }
            "resume" => self.resume_extension(id),
            "stop" => self.stop_extension(id),
            "status" => {
                let lines = self
                    .extensions
                    .loaded
                    .values()
                    .map(|l| {
                        format!(
                            "{} revision={} pending_tasks={} mode={} state={:?}",
                            l.package.manifest.id,
                            l.instance.revision,
                            l.instance.tasks.values().filter(|t| !t.delivered).count(),
                            self.extensions.mode,
                            l.instance.values
                        )
                    })
                    .collect::<Vec<_>>();
                self.message(
                    Severity::Info,
                    "extension",
                    if lines.is_empty() {
                        "No extensions loaded".into()
                    } else {
                        lines.join("\n")
                    },
                );
            }
            _ => self.message(
                Severity::Error,
                "extension",
                "Usage: :extension load <path> | reload|unload|status|resume|stop|abandon <id>",
            ),
        }
    }

    pub(super) fn select_work_mode(&mut self, name: &str) {
        if name.is_empty() {
            let mut names = vec!["chat".to_owned(), "task".to_owned()];
            names.extend(
                self.extensions
                    .loaded
                    .values()
                    .flat_map(|l| l.package.manifest.modes.keys().cloned()),
            );
            self.message(Severity::Info, "mode", names.join(", "));
            return;
        }
        let owner = self
            .extensions
            .loaded
            .iter()
            .find(|(_, l)| l.package.manifest.modes.contains_key(name))
            .map(|(id, _)| id.clone());
        if !matches!(name, "chat" | "task") && owner.is_none() {
            self.message(Severity::Error, "mode", "Unknown working mode");
            return;
        }
        let old = self.extensions.mode.clone();
        if let Some(id) = self
            .extensions
            .loaded
            .iter()
            .find(|(_, l)| l.package.manifest.modes.contains_key(&old))
            .map(|(id, _)| id.clone())
        {
            self.queue_extension(
                &id,
                "mode",
                &old,
                BTreeMap::from([("action".into(), "leave".into())]),
                None,
            );
        }
        self.extensions.mode = name.into();
        let _ = std::fs::write(self.extension_mode_path(), name);
        if let Some(id) = owner {
            self.open_extension_panel(name);
            self.queue_extension(
                &id,
                "mode",
                name,
                BTreeMap::from([("action".into(), "enter".into())]),
                None,
            );
        }
        self.message(Severity::Info, "mode", format!("Working mode: {name}"));
    }
    fn extension_mode_path(&self) -> PathBuf {
        self.paths.state.join(format!(
            "mode-{}.txt",
            &sha256(self.workspace.to_string_lossy().as_bytes())[..16]
        ))
    }
    pub(super) fn route_mode_input(&mut self, input: &str) -> bool {
        if self.extensions.mode == "task" {
            let input = input.trim();
            if !input.is_empty() {
                self.submit_source_agent_input(format!("task {input}"));
            }
            return true;
        }
        let mode = self.extensions.mode.clone();
        if let Some(id) = self
            .extensions
            .loaded
            .iter()
            .find(|(_, l)| l.package.manifest.modes.contains_key(&mode))
            .map(|(id, _)| id.clone())
        {
            self.queue_extension(
                &id,
                "mode",
                &mode,
                BTreeMap::from([
                    ("action".into(), "input".into()),
                    ("input".into(), input.into()),
                ]),
                None,
            );
            true
        } else {
            false
        }
    }
    pub(super) fn route_extension_command(&mut self, name: &str, args: &[String]) -> bool {
        if let Some(id) = self
            .extensions
            .loaded
            .iter()
            .find(|(_, l)| l.package.manifest.commands.contains_key(name))
            .map(|(id, _)| id.clone())
        {
            self.queue_extension(
                &id,
                "command",
                name,
                BTreeMap::from([("input".into(), args.join(" "))]),
                None,
            );
            true
        } else {
            false
        }
    }
    pub(super) fn fire_extension_hook(&mut self, name: &str, context: &serde_json::Value) {
        if self.extensions.active {
            return;
        }
        let owners = self
            .extensions
            .loaded
            .iter()
            .filter(|(_, l)| l.package.manifest.hooks.contains_key(name))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in owners {
            self.queue_extension(
                &id,
                "hook",
                name,
                BTreeMap::from([("context".into(), context.to_string())]),
                None,
            );
        }
    }
    pub(super) fn open_extension_panel(&mut self, name: &str) -> bool {
        if !self
            .extensions
            .loaded
            .values()
            .any(|l| l.package.manifest.panels.contains_key(name))
        {
            return false;
        }
        let existing = self
            .panels
            .iter()
            .find(|(_, p)| p.as_custom().is_some_and(|p| p.name == name))
            .map(|(id, _)| id);
        let panel = existing.unwrap_or_else(|| {
            self.add_panel(
                crate::presentation::CustomPanel::new(name.into()),
                SplitAxis::Columns,
            )
        });
        self.route_extension_panel(panel, BTreeMap::from([("kind".into(), "mount".into())]));
        true
    }
    pub(super) fn route_extension_panel(
        &mut self,
        panel: PanelId,
        event: BTreeMap<String, String>,
    ) -> bool {
        let Some(name) = self
            .panels
            .get(panel)
            .and_then(AppPanel::as_custom)
            .map(|p| p.name.clone())
        else {
            return false;
        };
        if let Some(id) = self
            .extensions
            .loaded
            .iter()
            .find(|(_, l)| l.package.manifest.panels.contains_key(&name))
            .map(|(id, _)| id.clone())
        {
            self.queue_extension(&id, "panel", &name, event, Some(panel));
            true
        } else {
            false
        }
    }
    fn queue_extension(
        &mut self,
        extension: &str,
        kind: &str,
        name: &str,
        mut fields: BTreeMap<String, String>,
        panel: Option<PanelId>,
    ) {
        if self.extensions.queue.len() >= 128 {
            self.message(
                Severity::Warning,
                "extension",
                "Extension event queue is full",
            );
            return;
        }
        if fields.get("kind").is_some_and(|k| k == "tick")
            && self
                .extensions
                .queue
                .iter()
                .any(|j| j.extension == extension && j.panel == panel)
        {
            return;
        }
        self.extensions.counter += 1;
        if kind == "panel" {
            fields.insert(
                "activity".into(),
                self.extensions
                    .activity
                    .get(extension)
                    .cloned()
                    .unwrap_or_default(),
            );
        }
        self.extensions.queue.push_back(Job {
            extension: extension.into(),
            event: ExtensionEvent {
                id: format!(
                    "{}-{}",
                    teal_workbench_services::agent_algorithms::now_ms(),
                    self.extensions.counter
                ),
                kind: kind.into(),
                name: name.into(),
                fields,
            },
            panel,
        });
        self.pump_extensions();
    }
    pub(super) fn pump_extensions(&mut self) {
        if self.extensions.active || self.legacy_scripts_pending() {
            return;
        }
        let Some(job) = self.extensions.queue.pop_front() else {
            return;
        };
        let Some(l) = self.extensions.loaded.get(&job.extension) else {
            self.pump_extensions();
            return;
        };
        match l.instance.contains_event(&job.event) {
            Ok(true) => {
                self.pump_extensions();
                return;
            }
            Err(error) => {
                self.message(Severity::Error, "extension", error.to_string());
                return;
            }
            Ok(false) => {}
        }
        let store = teal_agent::RuntimeStore::connect(&self.workspace).ok();
        let states = l
            .instance
            .tasks
            .iter()
            .map(|(key, task)| {
                let status = task
                    .run_id
                    .as_deref()
                    .and_then(|run| store.as_ref().and_then(|s| s.run(run).ok().flatten()))
                    .and_then(|run| serde_json::to_value(run.state).ok())
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "pending".into());
                (key.clone(), status)
            })
            .collect();
        let call = ExtensionCall::new(
            l.instance.clone(),
            job.event.clone(),
            l.config.clone(),
            l.package.manifest.tasks.keys().cloned().collect(),
        )
        .with_task_states(states);
        let package = l.package.clone();
        let workspace = self.workspace.clone();
        let revision = l.instance.revision;
        let tools = self.workspace_tools.clone();
        let output = self.runtime_tx.clone();
        self.extensions.active = true;
        self.tasks.spawn(async move {
            let event = job.event.clone();
            let result = crate::runtime_bridge::spawn_blocking(
                move || -> Result<ExtensionCallOutput, String> {
                    let mut registry = teal::HostRegistry::standard();
                    call.register(&mut registry).map_err(err)?;
                    tools.register_script_hosts(&mut registry).map_err(err)?;
                    hosts::register_preflight(&mut registry).map_err(err)?;
                    let runtime = WorkbenchAutomationRuntime::for_extension(registry);
                    if !package.is_current().map_err(err)? {
                        return Err("Extension source changed; reload requires approval".into());
                    }
                    let entry = package
                        .entry(&event.kind, &event.name)
                        .ok_or("Unknown extension entry")?;
                    if runtime
                        .execute_event(entry, grant(&package, &event.name, false, &workspace))
                        .map_err(err)?
                        != 0
                    {
                        return Err("Extension event failed".into());
                    }
                    call.output().map_err(err)
                },
            )
            .await
            .map_err(err)
            .and_then(|v| v);
            let _ = output
                .send(RuntimeEvent::ExtensionEventFinished(EventFinished {
                    extension: job.extension,
                    event: job.event,
                    panel: job.panel,
                    revision,
                    result,
                }))
                .await;
        });
    }
    pub(super) fn finish_extension_event(&mut self, f: EventFinished) {
        self.extensions.active = false;
        let result = (|| -> Result<ExtensionCallOutput, String> {
            let output = f.result?;
            let l = self
                .extensions
                .loaded
                .get_mut(&f.extension)
                .ok_or("Extension was unloaded")?;
            if l.instance.revision != f.revision {
                return Err("Extension event is stale".into());
            }
            // 纯重绘不产生可恢复业务动作，避免 tick 把去重历史无限写大。
            if output.values == l.instance.values
                && output.tasks.is_empty()
                && output.approvals.is_empty()
                && output.controls.is_empty()
                && !f.event.id.starts_with("task-result:")
            {
                return Ok(output);
            }
            let mut approvals = output.approvals.clone();
            for r in &mut approvals {
                for a in &mut r.artifacts {
                    *a = artifact_receipt(&self.workspace, &a.path)?;
                }
            }
            let mut next = l.instance.clone();
            for (key, control) in &output.controls {
                next.tasks
                    .get_mut(key)
                    .ok_or("Task control target is missing")?
                    .control = Some(*control);
            }
            if f.event.id.starts_with("task-result:")
                && let Some(task) = f
                    .event
                    .fields
                    .get("task_key")
                    .and_then(|key| next.tasks.get_mut(key))
            {
                task.delivered = true;
            }
            next.commit(
                &l.path,
                &f.event,
                output.values.clone(),
                output.tasks.clone(),
                approvals,
            )
            .map_err(err)?;
            l.instance = next;
            Ok(output)
        })();
        match result {
            Err(error) => self.message(Severity::Error, "extension", error),
            Ok(output) => {
                if let (Some(panel), Some(json)) = (f.panel, output.rendered)
                    && let Some(custom) =
                        self.panels.get_mut(panel).and_then(AppPanel::as_custom_mut)
                    && let Err(error) = custom.ui.replace(&json)
                {
                    self.message(Severity::Error, "extension.ui", error);
                }
                for message in output.messages {
                    self.message(Severity::Info, "extension", message);
                }
                for task in output.tasks {
                    self.start_extension_task(&f.extension, &task.key);
                }
                if !output.controls.is_empty() {
                    self.apply_extension_controls(
                        &f.extension,
                        Some(&output.controls.keys().cloned().collect()),
                    );
                }
            }
        }
        self.pump_scripts();
        self.pump_extension_approvals();
    }
    pub(super) fn pump_extension_approvals(&mut self) {
        if self.pending_overlay.is_some() || self.extensions.active {
            return;
        }
        if let Some(l) = self.extensions.pending_loads.pop_front() {
            self.overlays.push(ConfirmOverlay::new(
                "Load Teal extension",
                format!(
                    "{}\nSource: {}\nCapabilities: {:?}",
                    l.package.manifest.id, l.package.fingerprint, l.package.operations
                ),
            ));
            self.pending_overlay = Some(PendingOverlay::Confirmation(
                PendingConfirmation::ExtensionLoad(Box::new(l)),
            ));
            return;
        }
        if let Some((extension, r)) = self.extensions.loaded.iter().find_map(|(id, l)| {
            l.instance
                .approvals
                .values()
                .find(|r| {
                    r.approved.is_none()
                        && r.artifacts.iter().all(|a| {
                            artifact_receipt(&self.workspace, &a.path)
                                .is_ok_and(|current| current == *a)
                        })
                })
                .map(|r| (id.clone(), r.clone()))
        }) {
            self.overlays.push(ConfirmOverlay::new(
                "Extension approval",
                format!(
                    "{}\nOperation: {}\nEnvironment: {}\nArtifacts: {:?}",
                    r.prompt, r.operation, r.environment, r.artifacts
                ),
            ));
            self.pending_overlay = Some(PendingOverlay::Confirmation(
                PendingConfirmation::ExtensionApproval {
                    extension,
                    receipt: r.id,
                },
            ));
        }
    }
    pub(super) fn answer_extension_approval(
        &mut self,
        extension: &str,
        receipt_id: &str,
        accepted: bool,
    ) {
        let result = (|| -> Result<(), String> {
            let l = self
                .extensions
                .loaded
                .get_mut(extension)
                .ok_or("Extension was unloaded")?;
            let r = l
                .instance
                .approvals
                .get(receipt_id)
                .ok_or("Unknown approval")?;
            if r.approved.is_some() {
                return Err("Approval was already answered".into());
            }
            if accepted {
                for a in &r.artifacts {
                    if artifact_receipt(&self.workspace, &a.path)? != *a {
                        return Err("Approval artifacts changed; request a new approval".into());
                    }
                }
            }
            let mut next = l.instance.clone();
            next.approvals
                .get_mut(receipt_id)
                .expect("known receipt")
                .approved = Some(accepted);
            next.revision += 1;
            next.save(&l.path).map_err(err)?;
            l.instance = next;
            Ok(())
        })();
        match result {
            Err(error) => self.message(Severity::Error, "extension", error),
            Ok(()) => {
                if let Some(callback) = self.extensions.loaded.get(extension).and_then(|l| {
                    l.instance
                        .approvals
                        .get(receipt_id)
                        .and_then(|r| r.callback.clone())
                        .or_else(|| {
                            l.package.manifest.modes.keys().next().map(|name| {
                                teal_workbench_automation::extension::ExtensionCallback {
                                    kind: "mode".into(),
                                    name: name.clone(),
                                }
                            })
                        })
                }) {
                    let panel = if callback.kind == "panel" {
                        self.panels
                            .iter()
                            .find(|(_, p)| p.as_custom().is_some_and(|p| p.name == callback.name))
                            .map(|(id, _)| id)
                    } else {
                        None
                    };
                    self.queue_extension(
                        extension,
                        &callback.kind,
                        &callback.name,
                        BTreeMap::from([
                            ("action".into(), "approval".into()),
                            ("receipt".into(), receipt_id.into()),
                        ]),
                        panel,
                    );
                }
            }
        }
    }
}

fn approve_extension_trust(
    trust: &mut teal_workbench_automation::TrustStore,
    package: &WorkbenchPluginPackage,
    workspace: &Path,
) -> Result<(), String> {
    let paths = teal_config::ConfigPaths::discover(Some(workspace)).map_err(err)?;
    let _ = paths;
    trust
        .approve(
            &package.root.join("extension.toml"),
            package.fingerprint.as_bytes(),
            package.operations.clone(),
        )
        .map_err(err)
}
fn grant(
    package: &WorkbenchPluginPackage,
    task: &str,
    model: bool,
    workspace: &Path,
) -> Arc<teal::HostExecutionGrant> {
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let mut g =
        teal::HostExecutionGrant::deny_all(format!("bench-extension:{}", package.fingerprint))
            .allow_permissions(package.operations.iter().cloned());
    if model {
        g = g.allow_tools(
            package
                .manifest
                .task_tools
                .get(task)
                .into_iter()
                .flatten()
                .cloned(),
        );
    }
    if package.operations.contains("editor.read") {
        g = g.allow_resource(teal::ResourceGrant {
            access: teal::ResourceAccess::Read,
            resource: teal::HostResource::Path(workspace.clone()),
            scope: teal::ResourceScope::Descendants,
        });
    }
    if package.operations.contains("editor.write") {
        g = g.allow_resource(teal::ResourceGrant {
            access: teal::ResourceAccess::Write,
            resource: teal::HostResource::Path(workspace),
            scope: teal::ResourceScope::Descendants,
        });
    }
    if package.operations.contains("network.http")
        || package.operations.contains("browser.navigate")
    {
        g = g.allow_resource(teal::ResourceGrant {
            access: teal::ResourceAccess::Network,
            resource: teal::HostResource::Network(String::new()),
            scope: teal::ResourceScope::Descendants,
        });
    }
    if package.operations.contains("browser.inspect") {
        g = allow_application_resource(g, teal::ResourceAccess::Read, "browser");
    }
    if package.operations.contains("browser.navigate")
        || package.operations.contains("browser.interact")
    {
        g = allow_application_resource(g, teal::ResourceAccess::Write, "browser");
    }
    if package.operations.contains("computer.inspect") {
        g = allow_application_resource(g, teal::ResourceAccess::Read, "computer");
    }
    if package.operations.contains("computer.interact")
        || package.operations.contains("computer.raw_input")
    {
        g = allow_application_resource(g, teal::ResourceAccess::Write, "computer");
    }
    Arc::new(g)
}

fn allow_application_resource(
    grant: teal::HostExecutionGrant,
    access: teal::ResourceAccess,
    namespace: &str,
) -> teal::HostExecutionGrant {
    grant.allow_resource(teal::ResourceGrant {
        access,
        resource: teal::HostResource::Application {
            namespace: namespace.into(),
            identity: String::new(),
        },
        scope: teal::ResourceScope::Descendants,
    })
}
fn artifact_receipt(workspace: &Path, path: &str) -> Result<ArtifactReceipt, String> {
    let root = workspace.canonicalize().map_err(err)?;
    let candidate = root.join(path);
    let (target, absent) = match candidate.canonicalize() {
        Ok(target) => (target, false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut parent = candidate.as_path();
            let mut suffix = Vec::new();
            while !parent.exists() {
                suffix.push(
                    parent
                        .file_name()
                        .ok_or("Invalid artifact path")?
                        .to_owned(),
                );
                parent = parent.parent().ok_or("Invalid artifact parent")?;
            }
            let mut target = parent.canonicalize().map_err(err)?;
            for part in suffix.into_iter().rev() {
                target.push(part);
            }
            (target, true)
        }
        Err(error) => return Err(error.to_string()),
    };
    if !target.starts_with(&root) || (!absent && !target.is_file()) {
        return Err("Artifact must be a regular workspace file or a removed file".into());
    }
    let relative = target
        .strip_prefix(&root)
        .map_err(err)?
        .to_string_lossy()
        .replace('\\', "/");
    Ok(ArtifactReceipt {
        path: relative,
        sha256: if absent {
            "absent".into()
        } else {
            sha256(&std::fs::read(target).map_err(err)?)
        },
    })
}
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
