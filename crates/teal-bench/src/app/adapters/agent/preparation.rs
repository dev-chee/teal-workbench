//! 行内补全所需的模型准备与源码 Agent 配置装配。

use super::*;
use crate::app::PendingModelProfileName;
use crate::workbench::ui::{CmdlineMode, Prompt};
use teal_agent::{AgentHookEvent, AgentHookKind, AgentHookRegistry, HookDecision};

impl App {
    /// 启动或控制自由问答与脚本交付 Agent。
    pub(in crate::app) fn run_agent(&mut self, prompt: String) {
        let name = self.active_agent.as_deref().unwrap_or("default");
        if !self.agent_app_scripts.contains_key(name) {
            self.message(
                Severity::Error,
                "agent",
                format!("Agent application {name} is not registered"),
            );
            return;
        }
        let prompt = prompt.trim();
        if prompt.is_empty() {
            let panel = self.ensure_agent_panel();
            let _ = self.windows.focus(panel);
            return;
        }
        let input = if self.extensions.mode == "task" && !prompt.starts_with("task ") {
            format!("task {prompt}")
        } else {
            prompt.to_owned()
        };
        self.submit_source_agent_input(input);
    }

    /// 源码 Agent 自己负责模型准备；工作台只清理旧的行内补全请求。
    pub(in crate::app) fn prepare_active_agent(&mut self, pending_request: Option<String>) {
        self.pending_inline_completion = None;
        let _ = pending_request;
        self.message(
            Severity::Info,
            "agent",
            "Inline completion is unavailable until the source Agent provides a model",
        );
    }

    pub(in crate::app) fn active_model_capability(
        &self,
    ) -> teal_workbench_services::agent_algorithms::ConnectivityCapability {
        let local = self
            .active_agent
            .as_ref()
            .and_then(|name| self.agent_settings.get(name))
            .is_some_and(|(_, settings)| self.local_model_providers.contains(&settings.provider));
        if local {
            teal_workbench_services::agent_algorithms::ConnectivityCapability::LocalModel
        } else {
            teal_workbench_services::agent_algorithms::ConnectivityCapability::RemoteModel
        }
    }

    /// 缺少共享默认值时先按 provider 展示模型目录。
    fn open_model_picker(
        &mut self,
        document: &teal_model::ModelDocument,
        pending_request: Option<String>,
    ) -> bool {
        let providers = document
            .available_models(None, None)
            .into_iter()
            .map(|item| item.provider)
            .collect::<std::collections::BTreeSet<_>>();
        let items = providers
            .into_iter()
            .map(|provider| {
                crate::workbench::ui::MenuItem::enabled(provider.to_string(), provider.to_string())
            })
            .collect::<Vec<_>>();
        if items.is_empty() {
            return false;
        }
        self.overlays
            .push(crate::workbench::ui::PickerOverlay::with_items(
                "Select Provider",
                items,
            ));
        self.pending_overlay = Some(PendingOverlay::ModelProviderPicker { pending_request });
        true
    }

    /// 接受 provider 选择并展示认证资料。
    pub(in crate::app) fn select_model_provider(
        &mut self,
        provider: &str,
        pending_request: Option<String>,
    ) {
        let provider = match teal_model::ProviderId::new(provider.to_owned()) {
            Ok(provider) => provider,
            Err(error) => {
                self.message(Severity::Error, "agent", error.to_string());
                return;
            }
        };
        let snapshot = match self.model_access.configuration() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.message(Severity::Error, "agent", error.to_string());
                return;
            }
        };
        let auth_profiles = snapshot
            .document
            .available_models(Some(&provider), None)
            .into_iter()
            .map(|item| item.auth_profile)
            .collect::<std::collections::BTreeSet<_>>();
        let items = auth_profiles
            .into_iter()
            .map(|profile| {
                crate::workbench::ui::MenuItem::enabled(
                    profile.to_string(),
                    format!("{provider} / {profile}"),
                )
            })
            .collect::<Vec<_>>();
        if items.is_empty() {
            self.message(
                Severity::Error,
                "agent",
                format!("Provider `{provider}` has no available authentication profiles"),
            );
            return;
        }
        self.overlays
            .push(crate::workbench::ui::PickerOverlay::with_items(
                "Select Authentication Profile",
                items,
            ));
        self.pending_overlay = Some(PendingOverlay::ModelAuthProfilePicker {
            pending_request,
            provider,
        });
    }

    /// 接受认证资料选择并展示该账户的模型。
    pub(in crate::app) fn select_model_auth_profile(
        &mut self,
        provider: &teal_model::ProviderId,
        auth_profile: &str,
        pending_request: Option<String>,
    ) {
        let auth_profile = match teal_model::AuthProfileName::new(auth_profile.to_owned()) {
            Ok(profile) => profile,
            Err(error) => {
                self.message(Severity::Error, "agent", error.to_string());
                return;
            }
        };
        let snapshot = match self.model_access.configuration() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.message(Severity::Error, "agent", error.to_string());
                return;
            }
        };
        let models = snapshot
            .document
            .available_models(Some(provider), Some(&auth_profile));
        let auth_status = self
            .model_access
            .auth()
            .profiles()
            .ok()
            .and_then(|profiles| {
                profiles.into_iter().find(|profile| {
                    profile.provider.as_str() == provider.as_str() && profile.name == auth_profile
                })
            })
            .map_or("unknown", |profile| match profile.snapshot {
                teal_model::AuthSnapshot::SignedIn => "signed-in",
                teal_model::AuthSnapshot::SignedOut => "signed-out",
                teal_model::AuthSnapshot::Expired => "expired",
            });
        let items = models
            .iter()
            .map(|item| {
                let profiles = if item.model_profiles.is_empty() {
                    "new named profile".to_owned()
                } else {
                    item.model_profiles
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                crate::workbench::ui::MenuItem::enabled(
                    item.model.to_string(),
                    format!("{}  auth {auth_status}  ({profiles})", item.model),
                )
            })
            .collect::<Vec<_>>();
        if items.is_empty() {
            self.message(
                Severity::Error,
                "agent",
                format!(
                    "Authentication profile `{provider}/{auth_profile}` has no available models"
                ),
            );
            return;
        }
        self.overlays
            .push(crate::workbench::ui::PickerOverlay::with_items(
                "Select Model",
                items,
            ));
        self.pending_overlay = Some(PendingOverlay::ModelPicker {
            pending_request,
            provider: provider.clone(),
            auth_profile,
        });
    }

    /// 接受模型选择并处理已有或新建的命名模型预设。
    pub(in crate::app) fn select_model_model(
        &mut self,
        provider: &teal_model::ProviderId,
        auth_profile: &teal_model::AuthProfileName,
        model: &str,
        pending_request: Option<String>,
    ) {
        let Ok(model) = teal_model::ModelId::new(model.to_owned()) else {
            self.message(Severity::Error, "agent", "Invalid model identifier");
            return;
        };
        let snapshot = match self.model_access.configuration() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.message(Severity::Error, "agent", error.to_string());
                return;
            }
        };
        let Some(selection) = snapshot
            .document
            .available_models(Some(provider), Some(auth_profile))
            .into_iter()
            .find(|item| item.model == model)
        else {
            self.message(Severity::Error, "agent", format!("Model `{provider}/{model}` is unavailable to authentication profile `{auth_profile}`"));
            return;
        };
        if selection.model_profiles.len() == 1 {
            self.apply_model_selection(
                &selection,
                selection.model_profiles[0].clone(),
                pending_request,
            );
        } else {
            let mut items = selection
                .model_profiles
                .iter()
                .map(|profile| {
                    crate::workbench::ui::MenuItem::enabled(
                        profile.to_string(),
                        profile.to_string(),
                    )
                })
                .collect::<Vec<_>>();
            items.push(crate::workbench::ui::MenuItem::enabled(
                "__new_model_profile__",
                "Create a new named model profile",
            ));
            self.overlays
                .push(crate::workbench::ui::PickerOverlay::with_items(
                    "Select Model Profile",
                    items,
                ));
            self.pending_overlay = Some(PendingOverlay::ModelProfilePicker {
                pending_request,
                selection,
            });
        }
    }

    /// 接受已有模型预设，或打开新预设名称输入。
    pub(in crate::app) fn select_model_profile(
        &mut self,
        selection: &teal_model::AvailableModel,
        model_profile: &str,
        pending_request: Option<String>,
    ) {
        if model_profile == "__new_model_profile__" {
            self.pending_model_profile_name = Some(PendingModelProfileName {
                pending_request,
                provider: selection.provider.clone(),
                auth_profile: selection.auth_profile.clone(),
                model: selection.model.clone(),
            });
            self.cmdline = Some(Prompt::with_text(CmdlineMode::Prompt, String::new()));
            self.message(
                Severity::Info,
                "agent",
                "Enter a name for the new model profile",
            );
            return;
        }
        let Ok(model_profile) = teal_model::ProfileName::new(model_profile.to_owned()) else {
            self.message(Severity::Error, "agent", "Invalid model profile name");
            return;
        };
        if !selection.model_profiles.contains(&model_profile) {
            self.message(
                Severity::Error,
                "agent",
                "Model profile does not match the selected model",
            );
            return;
        }
        self.apply_model_selection(selection, model_profile, pending_request);
    }

    /// 完成新模型预设的显式命名。
    pub(crate) fn finish_model_profile_name(
        &mut self,
        text: &str,
        pending: PendingModelProfileName,
    ) {
        let Ok(profile) = teal_model::ProfileName::new(text.trim().to_owned()) else {
            self.message(Severity::Error, "agent", "Invalid model profile name");
            return;
        };
        let selection = teal_model::AvailableModel {
            provider: pending.provider.clone(),
            auth_profile: pending.auth_profile.clone(),
            model: pending.model.clone(),
            model_profiles: Vec::new(),
        };
        self.apply_model_selection(&selection, profile, pending.pending_request);
    }

    /// 将固定的 provider/model/auth 选择写入默认预设并绑定当前 Agent。
    fn apply_model_selection(
        &mut self,
        selection: &teal_model::AvailableModel,
        model_profile: teal_model::ProfileName,
        pending_request: Option<String>,
    ) {
        let snapshot = match self.model_access.configuration() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.message(Severity::Error, "agent", error.to_string());
                return;
            }
        };
        let scope = if snapshot.workspace.as_ref().is_some_and(|document| {
            document.profiles.contains_key(&model_profile)
                || document
                    .providers
                    .get(&selection.provider)
                    .is_some_and(|provider| {
                        provider.auth_profiles.contains_key(&selection.auth_profile)
                            && provider.models.contains_key(&selection.model)
                    })
        }) {
            teal_model::ModelConfigScope::Workspace
        } else {
            teal_model::ModelConfigScope::User
        };
        let save = if selection.model_profiles.contains(&model_profile) {
            self.model_access
                .set_default_profile(scope, model_profile.clone())
        } else {
            self.model_access.set_default_for_profile(
                scope,
                model_profile.clone(),
                teal_model::ModelRef {
                    provider: selection.provider.clone(),
                    model: selection.model.clone(),
                },
                selection.auth_profile.clone(),
            )
        };
        if let Err(error) = save {
            self.message(
                Severity::Error,
                "agent",
                format!("Unable to save the default model: {error}"),
            );
            return;
        }

        let Some(name) = self.active_agent.clone() else {
            self.message(Severity::Error, "agent", "No active Agent is configured");
            return;
        };
        let Some((_, settings)) = self.agent_settings.get_mut(&name) else {
            self.message(
                Severity::Error,
                "agent",
                format!("Active Agent {name} does not exist"),
            );
            return;
        };
        settings.provider = selection.provider.to_string();
        settings.profile = Some(selection.auth_profile.to_string());
        settings.model = selection.model.to_string();
        self.inline_completion = None;
        self.message(
            Severity::Info,
            "agent",
            format!("Selected model {}/{}", selection.provider, selection.model),
        );
        let _ = pending_request;
    }

    pub(in crate::app) fn agent_hooks(&self) -> Result<AgentHookRegistry, String> {
        let mut hooks = AgentHookRegistry::new();
        for (name, kind) in [
            ("agent.session_started", AgentHookKind::SessionStarted),
            ("agent.session_ended", AgentHookKind::SessionEnded),
            ("agent.user_input", AgentHookKind::UserInput),
            ("agent.subagent_started", AgentHookKind::SubagentStarted),
            ("agent.subagent_ended", AgentHookKind::SubagentEnded),
            ("agent.failed", AgentHookKind::Failed),
            ("agent.cancelled", AgentHookKind::Cancelled),
            ("agent.before_model", AgentHookKind::BeforeModel),
            ("agent.after_model", AgentHookKind::AfterModel),
            ("agent.before_tool", AgentHookKind::BeforeTool),
            ("agent.after_tool", AgentHookKind::AfterTool),
            ("agent.before_compaction", AgentHookKind::BeforeCompaction),
            ("agent.after_compaction", AgentHookKind::AfterCompaction),
            ("agent.finished", AgentHookKind::Completed),
        ] {
            for registration in self.scripting.hooks.get(name).into_iter().flatten() {
                // Agent 内联 Hook 不允许工作区脚本在模型循环中等待新的批准；工作区脚本仍可使用
                // 普通工作台生命周期 Hook。用户配置是本机显式代码，并继续受其已有 Host 能力约束。
                if registration.kind != teal_workbench_automation::ScriptKind::UserConfig {
                    continue;
                }
                let runtime = self.teal.clone();
                let tools = self.workspace_tools.clone();
                let registration = registration.clone();
                let hook_name = registration.path.display().to_string();
                hooks
                    .register(kind, hook_name, move |event: AgentHookEvent| {
                        let runtime = runtime.clone();
                        let tools = tools.clone();
                        let registration = registration.clone();
                        Box::pin(async move {
                            let result = tokio::task::spawn_blocking(move || {
                                let (path, kind) = registration.execution_target()?;
                                if kind != teal_workbench_automation::ScriptKind::UserConfig {
                                    return Err("Agent hook script left the trusted user configuration directory".to_owned());
                                }
                                let prepared = runtime
                                    .prepare(
                                        &path,
                                        teal_workbench_automation::ScriptKind::UserConfig,
                                    )
                                    .map_err(|error| error.to_string())?;
                                let context = serde_json::to_string(&event)
                                    .map_err(|error| error.to_string())?;
                                let grant = tools.script_grant(prepared.capabilities());
                                runtime
                                    .execute_legacy_event_with_grant(prepared, &context, grant)
                                    .map_err(|error| error.to_string())
                            })
                            .await
                            .map_err(|error| {
                                teal_agent::AgentError::Policy(format!(
                                    "Teal Hook worker thread failed: {error}"
                                ))
                            })?
                            .map_err(teal_agent::AgentError::Policy)?;
                            Ok(if result.exit_code == 0 {
                                HookDecision::Continue
                            } else {
                                HookDecision::Deny(format!(
                                    "Teal Hook exited with code {}",
                                    result.exit_code
                                ))
                            })
                        })
                    })
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(hooks)
    }

    /// 打开命名 Agent 选择器，或直接选择给定名称。
    pub(in crate::app) fn select_agent(&mut self, requested: &str) {
        if self.agent_app_tx.is_some() {
            self.message(
                Severity::Warning,
                "agent",
                "Stop the current Agent application before switching",
            );
            return;
        }
        if !requested.is_empty() {
            if !self.agent_app_scripts.contains_key(requested) {
                self.message(
                    Severity::Error,
                    "agent",
                    format!("Unknown Agent application {requested}"),
                );
                return;
            }
            self.active_agent = Some(requested.to_owned());
            self.inline_completion = None;
            return;
        }
        let items = self
            .agent_app_scripts
            .keys()
            .map(|name| {
                crate::workbench::ui::MenuItem::enabled(
                    name.clone(),
                    format!("{name}  Teal Agent application"),
                )
            })
            .collect::<Vec<_>>();
        if items.is_empty() {
            self.message(Severity::Warning, "agent", "No Agents are registered");
            return;
        }
        self.overlays
            .push(crate::workbench::ui::PickerOverlay::with_items(
                "Select Agent",
                items,
            ));
        self.pending_overlay = Some(crate::app::PendingOverlay::AgentPicker);
    }

    /// 打开用户或工作区 Teal Agent 配置；缺失文件通过统一原子写入器创建。
    pub(in crate::app) fn open_agent_config(&mut self, scope: &str) {
        let paths = match teal_config::ConfigPaths::discover(Some(&self.workspace)) {
            Ok(paths) => paths,
            Err(error) => {
                self.message(Severity::Error, "agent", error.to_string());
                return;
            }
        };
        let (path, scope) = match scope {
            "" | "user" => (
                paths
                    .path(
                        teal_config::ConfigScope::User,
                        teal_config::ConfigFile::Bench,
                    )
                    .unwrap_or_else(|| paths.teal_home.join("bench.toml")),
                teal_config::ConfigScope::User,
            ),
            "workspace" => (
                self.workspace.join(".teal/bench.toml"),
                teal_config::ConfigScope::Workspace,
            ),
            _ => {
                self.message(
                    Severity::Error,
                    "agent",
                    "agent-config only accepts user or workspace",
                );
                return;
            }
        };
        let source = teal_config::FileConfigSource::new(paths.clone());
        if !path.exists()
            && let Err(error) =
                teal_config::edit::<crate::config::BenchDocument>(&source, scope, None, |_| {})
        {
            self.message(Severity::Error, "agent", error.to_string());
            return;
        }
        self.queue_open(path, true, None);
    }

    /// 根据用户显式选择更新用户总体配置，不接收或写入任何秘密。
    pub(in crate::app) fn setup_agent(&mut self, arguments: &[String]) {
        let [provider, profile, model, model_profile] = arguments else {
            self.message(
                Severity::Error,
                "agent",
                "Usage: :agent-setup <provider> <auth-profile> <model> <model-profile>",
            );
            return;
        };
        let provider = match teal_model::ProviderId::new(provider.clone()) {
            Ok(provider) => provider,
            Err(error) => {
                self.message(Severity::Error, "agent", error.to_string());
                return;
            }
        };
        let profile = match teal_model::AuthProfileName::new(profile.clone()) {
            Ok(profile) => profile,
            Err(error) => {
                self.message(Severity::Error, "agent", error.to_string());
                return;
            }
        };
        let model = match teal_model::ModelId::new(model.clone()) {
            Ok(model) => model,
            Err(error) => {
                self.message(Severity::Error, "agent", error.to_string());
                return;
            }
        };
        let model_profile = match teal_model::ProfileName::new(model_profile.clone()) {
            Ok(model_profile) => model_profile,
            Err(error) => {
                self.message(Severity::Error, "agent", error.to_string());
                return;
            }
        };
        let paths = match teal_config::ConfigPaths::discover(Some(&self.workspace)) {
            Ok(paths) => paths,
            Err(error) => {
                self.message(Severity::Error, "agent", error.to_string());
                return;
            }
        };
        if let Err(error) = self.model_access.set_default_for_profile(
            teal_model::ModelConfigScope::User,
            model_profile.clone(),
            teal_model::ModelRef {
                provider: provider.clone(),
                model: model.clone(),
            },
            profile.clone(),
        ) {
            self.message(Severity::Error, "agent", error.to_string());
            return;
        }
        self.active_agent = Some("default".into());
        self.submit_source_agent_input(format!(":model {model_profile}/{model}"));
        let path = paths
            .path(
                teal_config::ConfigScope::User,
                teal_config::ConfigFile::Model,
            )
            .unwrap_or_else(|| paths.teal_home.join("model.toml"));
        self.queue_open(path, true, None);
        self.message(
            Severity::Info,
            "agent",
            format!(
                "Default model selected for the Teal Agent application; authenticate with `teal auth login {provider} --profile {profile}` before use"
            ),
        );
    }
}
