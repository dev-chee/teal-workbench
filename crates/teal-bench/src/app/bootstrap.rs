//! 应用初始布局、服务、配置与会话策略的装配。
//!
//! 先建立可用布局和宿主桥接，再加载配置并决定恢复；安全模式、用户配置和工作区配置保持不同边界。

use super::*;
use teal_agent::KnowledgeProvider;

/// 未选择会话 Agent 时使用的内建交互身份。
const DEFAULT_AGENT_NAME: &str = "default";

/// 返回只依赖共享模型默认值的内建 Agent 设置。
fn default_agent_settings() -> teal_workbench_automation::AgentSettings {
    teal_workbench_automation::AgentSettings {
        name: DEFAULT_AGENT_NAME.into(),
        provider: "default".into(),
        profile: None,
        model: "default".into(),
        compaction_profile: None,
        instructions: "Help the user with their request and use available tools when needed."
            .into(),
    }
}

/// 旧 Bench Flow 不能作为源码 VM 的恢复凭据；文件留在原处供人工检查。
fn reject_legacy_agent_state(state: &Path, workspace: &Path) -> Result<(), String> {
    let canonical = workspace
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let key =
        teal_workbench_services::agent_algorithms::digest(canonical.to_string_lossy().as_bytes());
    for directory in ["agent-flows", "agent-goals"] {
        let path = state.join(directory).join(format!("{}.json", &key[..24]));
        if path.exists() {
            return Err(format!(
                "Legacy Bench Agent state at {} cannot be resumed by the Teal Agent application; the file was left unchanged",
                path.display()
            ));
        }
    }
    let conversations = workspace.join(".teal/bench/conversations.json");
    if conversations.exists() {
        return Err(format!(
            "Legacy Bench Agent state at {} cannot be resumed by the Teal Agent application; the file was left unchanged",
            conversations.display()
        ));
    }
    Ok(())
}

/// 展示会话不能把旧 Agent 历史或未知命名入口当作源码 run 的恢复事实。
fn reject_legacy_session_state(
    session: &teal_workbench_services::SessionState,
    applications: &BTreeMap<String, PathBuf>,
    path: &Path,
) -> Result<(), String> {
    if !session.agents.is_empty()
        || session
            .active_agent
            .as_ref()
            .is_some_and(|name| !applications.contains_key(name))
    {
        return Err(format!(
            "Legacy Bench Agent session at {} cannot be resumed by the Teal Agent application; the file was left unchanged",
            path.display()
        ));
    }
    Ok(())
}

impl App {
    /// 解析启动状态、Teal 文法/配置、会话和智能体供应商。
    pub async fn new(options: AppOptions) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_connectivity(
            options,
            None,
            false,
            teal_workbench_services::agent_algorithms::ConnectivityMode::Online,
        )
        .await
    }

    pub(crate) async fn new_with_connectivity(
        options: AppOptions,
        meter: Option<opentelemetry::metrics::Meter>,
        function_spans: bool,
        connectivity: teal_workbench_services::agent_algorithms::ConnectivityMode,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_session_paths(options, meter, function_spans, connectivity, None).await
    }

    /// 在任何持久状态创建前注入会话路径，供同进程的独立宿主与测试隔离状态。
    pub(crate) async fn new_with_session_paths(
        options: AppOptions,
        meter: Option<opentelemetry::metrics::Meter>,
        function_spans: bool,
        connectivity: teal_workbench_services::agent_algorithms::ConnectivityMode,
        session_paths: Option<SessionPaths>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let should_consider_session =
            (options.path.is_none() || options.session.is_some()) && !options.safe_mode;
        let explicit_session = options.session.is_some();
        let config_enabled = !options.safe_mode && !options.no_config;

        let requested = options.path.clone().unwrap_or(env::current_dir()?);
        let requested = requested.canonicalize().unwrap_or(requested);
        let workspace = if requested.is_dir() {
            requested.clone()
        } else {
            requested
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf()
        };
        let unified_config = config_enabled
            .then(|| teal_security::load(Some(&workspace)))
            .transpose()?;

        #[cfg(test)]
        let test_home_guard = session_paths
            .as_ref()
            .filter(|_| env::var_os("TEAL_HOME").is_none())
            .map(|paths| crate::app::test_support::TestHomeGuard::set(&paths.config));

        let paths = session_paths
            .or_else(SessionPaths::discover)
            .unwrap_or_else(|| SessionPaths {
                config: workspace.join(".teal/bench").join("user"),
                state: workspace.join(".teal/bench").join("state"),
                cache: workspace.join(".teal/bench").join("cache"),
            });
        let session_path = options
            .session
            .clone()
            .unwrap_or_else(|| paths.session_for(&workspace));
        let runtime_lock_path = workspace.join(".teal/runtime.lock");
        let runtime_cache =
            teal_workbench_services::RuntimeCache::new(paths.cache.join("runtimes"));
        let runtime_catalog = teal::PackageCatalog::discover()?;
        let catalog_packages = runtime_catalog.packages();
        let runtime_core_catalog_digest = runtime_catalog
            .digest_for_packages(["std"])
            .map_err(|error| format!("Teal core package catalog is invalid: {error}"))?;
        let extension_names = catalog_packages
            .iter()
            .filter(|package| package.name != "std")
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>();
        let runtime_extension_catalog_digest = (!extension_names.is_empty())
            .then(|| runtime_catalog.digest_for_packages(extension_names))
            .transpose()
            .map_err(|error| format!("Teal extension package catalog is invalid: {error}"))?;
        let runtime_packages = catalog_packages
            .into_iter()
            .map(|package| teal_workbench_services::RuntimePackage {
                name: package.name,
                version: package.version,
                digest: package.digest,
            })
            .collect::<Vec<_>>();
        let runtime_lock = if runtime_lock_path.exists() {
            Some(teal_workbench_services::RuntimeLock::load(
                &runtime_lock_path,
            )?)
        } else {
            None
        };
        let mut runtime_lock_status = if let Some(lock) = &runtime_lock {
            lock.status_with_catalog_identity(
                env!("CARGO_PKG_VERSION"),
                &runtime_packages,
                &runtime_core_catalog_digest,
                runtime_extension_catalog_digest.as_deref(),
                teal::PACKAGE_FORMAT_VERSION,
                &runtime_cache,
            )
        } else {
            teal_workbench_services::RuntimeLockStatus::Missing
        };

        // 加载脚本前注册宿主回调；回调与 App 共享此桥接状态。
        let bridge = Arc::new(Mutex::new(BridgeState {
            workspace: workspace.clone(),
            offline: connectivity
                == teal_workbench_services::agent_algorithms::ConnectivityMode::Offline,
            ..BridgeState::default()
        }));
        let mut teal = WorkbenchAutomationRuntime::new(paths.state.join("automation.json"))?;
        teal.set_observation(meter, function_spans);
        register_automation_bridge(teal.registry_mut(), &bridge)?;

        let grammars = if options.safe_mode {
            HashMap::new()
        } else {
            let mut by_extension = HashMap::new();
            let runtime = teal.clone();
            let loaded =
                crate::runtime_bridge::spawn_blocking(move || runtime.load_builtin_grammars())
                    .await??;
            for grammar in loaded {
                for extension in grammar.extensions.clone() {
                    by_extension.insert(extension, grammar.clone());
                }
            }
            by_extension
        };
        let mut trust = TrustStore::load(paths.state.join("script-trust.json"))?;
        trust.clear_directory_approvals()?;
        let grammar_tables = load_parser_tables(&paths.cache, &grammars);

        let mut panels = PanelStore::<AppPanel>::default();
        let mut buffers = teal_workbench_core::BufferStore::default();
        let keymap = KeymapPreset::VimFirst;
        let session_restore_candidate = if should_consider_session {
            match load_session(&session_path) {
                Ok(session) => Some(session),
                Err(teal_workbench_services::SessionError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    None
                }
                Err(error) => return Err(error.into()),
            }
        } else {
            None
        };

        // 主窗口是 Agent 工作流；这里先放入骨架面板，待应用状态就绪后再装配真正的
        // 会话与输入面板（见 `install_primary_window`），从而让标识保持稳定。
        let skeleton = panels.insert(TextPanel::plain(PanelKind::Agent, "Session", "").into());
        let windows = WindowSet::new(Window::new(
            WindowKind::Agent,
            WindowKind::Agent.default_title(),
            LayoutTree::new(skeleton),
        ));

        let (approvals_tx, approvals_rx) = mpsc::channel(16);
        let (mutations_tx, mutations_rx) = mpsc::channel(32);
        let (changesets_tx, changesets_rx) = mpsc::channel(16);
        let diagnostics = teal_lsp_client::DiagnosticStore::default();
        let workspace_tools = Arc::new(WorkspaceToolRuntime::new(
            &workspace,
            paths.state.join("checkpoints"),
            mutations_tx,
            changesets_tx,
            diagnostics.clone(),
        )?);
        workspace_tools.synchronize_buffers(&buffers)?;
        workspace_tools.register_script_hosts(teal.registry_mut())?;
        let agent_permissions = if config_enabled {
            Arc::new(ChannelApproval::for_workspace(
                approvals_tx,
                teal_config::ConfigPaths::discover(Some(&workspace))?,
            ))
        } else {
            Arc::new(ChannelApproval::new(approvals_tx))
        };
        // 模型配置属于统一配置系统：禁用配置时不读取用户与工作区文件。工作区信任由用户层
        // `config.toml` 统一决定，模型后端不再单独接收信任标志。
        let model_configuration: teal_model::SharedModelConfiguration = if config_enabled {
            Arc::new(teal_model::ModelConfiguration::toml(
                teal_model::TomlModelConfiguration::discover(Some(&workspace))?,
            ))
        } else {
            Arc::new(teal_model::ModelConfiguration::Empty)
        };
        let mut model_access = teal_model::ModelAccess::open(model_configuration)?;
        if let Some(directory) = &options.model_protocol_capture {
            model_access = model_access.with_protocol_capture(Arc::new(
                teal_model::FileProtocolCapture::new(directory.clone()),
            ));
        }
        let model_configuration = model_access.configuration()?;
        let local_model_providers = model_configuration
            .document
            .profiles
            .values()
            .filter_map(|profile| {
                model_configuration
                    .document
                    .providers
                    .get(&profile.provider)
                    .filter(|provider| provider.access.local)
                    .map(|_| profile.provider.to_string())
            })
            .collect();
        let agent_memory = Arc::new(teal_agent::MemoryStore::open(
            paths.state.join("agent-memory.sqlite3"),
            workspace.join(".teal/runtime/memory.sqlite3"),
        )?);
        let user_skill_roots = env::var_os("USERPROFILE")
            .or_else(|| env::var_os("HOME"))
            .map(|home| vec![PathBuf::from(home).join(".agents/skills")])
            .unwrap_or_default();
        let agent_skills = Arc::new(teal_agent::SkillCatalog::discover(
            &user_skill_roots,
            &workspace,
        )?);
        let knowledge_config = unified_config
            .as_ref()
            .map(crate::config::knowledge)
            .transpose()?
            .unwrap_or_default();
        let knowledge_options = teal_knowledge::LayerOptions {
            cache: knowledge_config.cache,
            max_files: knowledge_config.max_files,
            max_bytes: knowledge_config.max_bytes,
        };
        let knowledge_trusted = unified_config
            .as_ref()
            .map(crate::config::trusted)
            .transpose()?
            .unwrap_or(false);
        let knowledge_home = teal_config::ConfigPaths::discover(Some(&workspace))
            .ok()
            .map(|paths| paths.teal_home);
        let agent_knowledge = Arc::new(
            teal_workbench_services::WorkbenchKnowledge::official()
                .map_err(|error| format!("Teal knowledge discovery failed: {error}"))?
                .with_layers(
                    knowledge_config
                        .enabled
                        .then(|| {
                            knowledge_home
                                .as_ref()
                                .map(|home| knowledge_config.user_scope_root(home))
                        })
                        .flatten(),
                    (knowledge_config.enabled && knowledge_trusted)
                        .then(|| knowledge_config.workspace_scope_root(&workspace)),
                    knowledge_options,
                )
                .map_err(|error| format!("Teal knowledge layers are unavailable: {error}"))?
                .with_inline(knowledge_config.inject_inline),
        );
        if let Some(lock) = &runtime_lock {
            let current = teal_workbench_services::RuntimeKnowledge {
                format_version: teal_knowledge::KNOWLEDGE_FORMAT_VERSION,
                toolchain_digest: agent_knowledge.toolchain_digest().to_owned(),
                content_digest: agent_knowledge.content_digest().to_owned(),
            };
            if let Some(status) = lock.knowledge_status(&current) {
                runtime_lock_status = status;
            }
        }
        let inline_completion = None;
        // 安全模式跳过所有 Agent 状态读取；旧状态只在普通启动时作为显式迁移错误报告。
        if !options.safe_mode {
            reject_legacy_agent_state(&paths.state, &workspace)?;
        }

        // 将服务队列接入终端循环拥有的事件流。
        let (background_tx, background_rx) = mpsc::channel(256);
        let (lsp_events_tx, lsp_events_rx) = mpsc::channel(256);
        let (runtime_tx, runtime_rx) = mpsc::channel(256);
        let runtime_tx = crate::runtime_bridge::RuntimeSender::new(runtime_tx);
        let mut tasks = TaskSupervisor::default();
        tasks.forward(approvals_rx, runtime_tx.clone(), RuntimeEvent::Approval);
        tasks.forward(mutations_rx, runtime_tx.clone(), RuntimeEvent::Mutation);
        tasks.forward(changesets_rx, runtime_tx.clone(), RuntimeEvent::ChangeSet);
        tasks.forward(background_rx, runtime_tx.clone(), RuntimeEvent::Message);
        tasks.forward(lsp_events_rx, runtime_tx.clone(), |(client, event)| {
            RuntimeEvent::Lsp(client, event)
        });

        let mut agent_settings = BTreeMap::new();
        // 空 owner 是内建声明的稳定标记；真实配置路径始终非空，可以覆盖同名默认项。
        agent_settings.insert(
            DEFAULT_AGENT_NAME.into(),
            (PathBuf::new(), default_agent_settings()),
        );

        let mut app = Self {
            #[cfg(test)]
            test_home_guard,
            observation_health: None,
            last_observation_warning: None,
            mutations: WorkspaceMutationCoordinator::new(
                workspace.clone(),
                paths.state.join("checkpoints"),
            )?,
            io: mutation::State::default(),
            workspace,
            safe_mode: options.safe_mode,
            panels,
            buffers,
            windows,
            overlays: OverlayStack::default(),
            cmdline: None,
            cmdline_history: Vec::new(),
            completion_menu: CompletionMenu::default(),
            hover: HoverView::default(),
            commands: CommandRegistry::standard(),
            messages: MessageLog::default(),
            keymap,
            browser_requests: HashMap::new(),
            browser_sequence: 0,
            grammars,
            grammar_tables,
            session_restore_policy: SessionRestorePolicy::Prompt,
            config_enabled,
            teal,
            trust,
            script_commands: HashMap::new(),
            scripting: scripting::State::default(),
            extensions: extensions::State::default(),
            bridge,
            paths,
            session_path,
            journals: HashMap::new(),
            recovery_keys: HashMap::new(),
            path_generations: HashMap::new(),
            deferred_opens: Vec::new(),
            last_area: Rect {
                x: 0,
                y: 0,
                width: 120,
                height: 40,
            },
            frame_geometry: FrameGeometry::default(),
            mouse_normalizer: MouseNormalizer::default(),
            mouse_capture: None,
            clipboard: Box::<SystemClipboard>::default(),
            should_quit: false,
            inline_completion,
            agent_generation: 0,
            workspace_tools,
            agent_permissions,
            model_access,
            local_model_providers,
            agent_memory,
            agent_skills,
            agent_knowledge,
            runtime_lock_status,
            runtime_lock,
            runtime_packages,
            runtime_core_catalog_digest,
            runtime_extension_catalog_digest,
            runtime_cache,
            connectivity,
            debug_control: None,
            debug_panel: None,
            debug_window: None,
            tests_panels: None,
            tests_source: None,
            tests_run_in_flight: false,
            agent_settings,
            agent_app_scripts: BTreeMap::from([(DEFAULT_AGENT_NAME.into(), PathBuf::new())]),
            agent_app_tx: None,
            agent_app_services: None,
            agent_app_active_context: None,
            agent_app_view_panel: None,
            mcp_settings: BTreeMap::new(),
            active_agent: Some(DEFAULT_AGENT_NAME.into()),
            pending_inline_completion: None,
            pending_approvals: std::collections::VecDeque::new(),
            pending_mutations: std::collections::VecDeque::new(),
            pending_changesets: std::collections::VecDeque::new(),
            agent_panel: None,
            pending_agent_profile_switch: None,
            agent_session_store: None,
            pending_merge: None,
            runtime_rx,
            tasks,
            pending_overlay: None,
            pending_prompt: None,
            pending_model_profile_name: None,
            window_prefix: false,
            lsp: teal_lsp_client::LspManager::default(),
            lsp_ui: lsp::State::default(),
            diagnostics,
            runtime_tx,
            lsp_events_tx,
            background_tx,
        };

        app.install_primary_window(options.start_file.as_deref())?;

        if let Some(config) = unified_config {
            app.install_declarative_config(&config)?;
        }
        // 启动只读：派生缓存只在 `:knowledge refresh` 或 `teal knowledge refresh` 时写回。
        if let Err(error) = app.refresh_knowledge(None, false).await {
            app.message(Severity::Warning, "knowledge", error);
        }
        if let Some(session) = session_restore_candidate {
            reject_legacy_session_state(&session, &app.agent_app_scripts, &app.session_path)?;
            let policy = if explicit_session {
                SessionRestorePolicy::Always
            } else if config_enabled {
                app.session_restore_policy
            } else {
                SessionRestorePolicy::Prompt
            };
            app.begin_session_restore(session, policy);
        } else if !options.safe_mode {
            app.recover_open_documents();
        }

        Ok(app)
    }
}

impl App {
    /// 保证内建默认 Agent 存在，并在当前选择失效时恢复到它。
    fn ensure_default_agent(&mut self) {
        self.agent_settings
            .entry(DEFAULT_AGENT_NAME.into())
            .or_insert_with(|| (PathBuf::new(), default_agent_settings()));
        if self.active_agent.is_none() {
            self.active_agent = Some(DEFAULT_AGENT_NAME.into());
        }
    }

    pub(super) fn install_declarative_config(
        &mut self,
        effective: &teal_config::EffectiveConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let resolved = effective.category::<crate::config::BenchDocument>()?;
        let config = resolved.document();
        for (name, declared) in &config.agent_apps {
            let key = format!("agent_apps.{name}.script");
            let path = resolve_bench_path(&resolved, effective, &key, &declared.script);
            let kind = match resolved.origin(&key).map(|origin| origin.scope) {
                Some(teal_config::ConfigScope::Workspace) => {
                    teal_workbench_automation::ScriptKind::WorkspaceConfig
                }
                _ => teal_workbench_automation::ScriptKind::UserConfig,
            };
            if path.exists() {
                let prepared = self.teal.prepare(&path, kind)?;
                if kind == teal_workbench_automation::ScriptKind::WorkspaceConfig
                    && crate::config::trusted(effective)?
                {
                    self.trust
                        .approve(
                            prepared.path(),
                            prepared.trust_contents(),
                            prepared.capabilities().clone(),
                        )
                        .map_err(|error| {
                            format!("Agent application {name} trust approval failed: {error}")
                        })?;
                }
                if let teal_workbench_automation::TrustDecision::ApprovalRequired { .. } =
                    self.teal.trust_decision(&prepared, &self.trust)
                {
                    return Err(format!(
                        "Agent application {name} requires explicit source approval before it can run"
                    )
                    .into());
                }
            }
            self.agent_app_scripts.insert(name.clone(), path);
        }
        let mut snapshot = teal_workbench_automation::RuntimeSnapshot {
            session_restore: config.session_restore.clone(),
            ..Default::default()
        };
        for (name, server) in &config.mcp_servers {
            snapshot
                .mcp_servers
                .push(teal_workbench_automation::McpSettings {
                    name: name.clone(),
                    transport: server.transport.clone(),
                    target: if server.transport == "stdio" {
                        resolve_bench_path(
                            &resolved,
                            effective,
                            &format!("mcp_servers.{name}.target"),
                            Path::new(&server.target),
                        )
                        .to_string_lossy()
                        .into_owned()
                    } else {
                        server.target.clone()
                    },
                    arguments: server.arguments.clone(),
                });
        }
        snapshot
            .commands
            .extend(config.commands.iter().map(|(name, script)| {
                teal_workbench_automation::AutomationCommand {
                    name: name.clone(),
                    script_path: resolve_bench_path(
                        &resolved,
                        effective,
                        &format!("commands.{name}.script"),
                        &script.script,
                    )
                    .to_string_lossy()
                    .into_owned(),
                }
            }));
        snapshot
            .panels
            .extend(config.panels.iter().map(|(name, script)| {
                teal_workbench_automation::AutomationCommand {
                    name: name.clone(),
                    script_path: resolve_bench_path(
                        &resolved,
                        effective,
                        &format!("panels.{name}.script"),
                        &script.script,
                    )
                    .to_string_lossy()
                    .into_owned(),
                }
            }));
        for (event, scripts) in &config.hooks {
            snapshot.hooks.extend(scripts.iter().map(|script| {
                teal_workbench_automation::AutomationCommand {
                    name: event.clone(),
                    script_path: resolve_bench_path(
                        &resolved,
                        effective,
                        &format!("hooks.{event}.script"),
                        &script.script,
                    )
                    .to_string_lossy()
                    .into_owned(),
                }
            }));
        }
        for (name, extension) in &config.plugins {
            let path = resolve_bench_path(
                &resolved,
                effective,
                &format!("plugins.{name}.path"),
                &extension.path,
            );
            self.load_extension(path, extension.config.clone());
        }
        if snapshot != teal_workbench_automation::RuntimeSnapshot::default() {
            // 所有脚本路径已经按声明层解析为绝对路径。以用户配置为 owner 安装时，
            // 用户目录外的工作区脚本仍会被降级为 WorkspaceConfig 并继续走独立能力批准。
            self.install_script_snapshot(
                &effective
                    .paths()
                    .path(
                        teal_config::ConfigScope::User,
                        teal_config::ConfigFile::Config,
                    )
                    .unwrap_or_else(|| effective.paths().teal_home.clone()),
                ScriptKind::UserConfig,
                snapshot,
            )?;
        }
        self.ensure_default_agent();
        Ok(())
    }
}

pub(super) fn resolve_bench_path(
    resolved: &teal_config::Resolved<crate::config::BenchDocument>,
    effective: &teal_config::EffectiveConfig,
    key: &str,
    path: &Path,
) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    resolved
        .resolve_path(key, path)
        .unwrap_or_else(|| effective.paths().teal_home.join(path))
}

fn load_parser_tables(
    cache_root: &Path,
    grammars: &HashMap<String, GrammarSpec>,
) -> HashMap<String, ParserTable> {
    let directory = cache_root.join("grammars");
    let _ = std::fs::create_dir_all(&directory);

    let mut tables = HashMap::new();
    for (extension, grammar) in grammars {
        let mut key = String::with_capacity(64);
        for byte in grammar.cache_key() {
            write!(key, "{byte:02x}").expect("Writing to string does not fail");
        }

        // 缓存字节无效或过期时，回退到重新构建同一文法。
        let path = directory.join(format!("{}-{key}.table", grammar.language));
        let cached = std::fs::read(&path)
            .ok()
            .and_then(|bytes| ParserTable::from_cache_bytes(&bytes, grammar).ok());
        let needs_write = cached.is_none();
        let table = cached.or_else(|| ParserTable::build(grammar).ok());
        if let Some(table) = table {
            if needs_write {
                let _ = std::fs::write(&path, table.to_cache_bytes());
            }
            tables.insert(extension.clone(), table);
        }
    }

    tables
}

/// 在启动前确认交互式工作区配置是否被信任。
pub(crate) fn ensure_workspace_trust(
    workspace: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{self, IsTerminal as _};
    let effective = teal_security::load(Some(workspace))?;
    if !effective
        .paths()
        .all_files()
        .iter()
        .any(|(scope, _, path)| *scope == teal_config::ConfigScope::Workspace && path.is_file())
        || crate::config::permissions(&effective)?.trust_level != teal_security::TrustLevel::Unknown
    {
        return Ok(());
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Ok(());
    }
    print!(
        "Trust Teal configuration in workspace {}? [y/N] ",
        workspace.display()
    );
    io::Write::flush(&mut io::stdout())?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let trust = if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        teal_security::TrustLevel::Trusted
    } else {
        teal_security::TrustLevel::Untrusted
    };
    teal_security::set_workspace_trust(effective.paths(), trust)?;
    Ok(())
}

#[cfg(test)]
mod legacy_agent_state_tests {
    use super::*;

    #[test]
    fn old_flow_is_rejected_without_rewriting_its_file() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let state = root.path().join("state");
        std::fs::create_dir_all(&workspace).unwrap();
        let key = teal_workbench_services::agent_algorithms::digest(
            workspace
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .as_bytes(),
        );
        let path = state
            .join("agent-flows")
            .join(format!("{}.json", &key[..24]));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let old = b"legacy flow";
        std::fs::write(&path, old).unwrap();
        assert!(
            reject_legacy_agent_state(&state, &workspace)
                .unwrap_err()
                .contains("cannot be resumed")
        );
        assert_eq!(std::fs::read(path).unwrap(), old);
    }

    #[test]
    fn old_agent_session_is_rejected_without_rewriting_its_file() {
        let root = tempfile::tempdir().unwrap();
        let mut panels = PanelStore::<AppPanel>::default();
        let panel = panels.insert(TextPanel::plain(PanelKind::Agent, "Session", "").into());
        let windows = WindowSet::new(Window::new(
            WindowKind::Agent,
            "Agent",
            LayoutTree::new(panel),
        ));
        let mut session =
            teal_workbench_services::SessionState::new(root.path().to_path_buf(), windows);
        session.active_agent = Some("legacy".into());
        let path = root.path().join("session.json");
        let bytes = serde_json::to_vec(&session).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let applications = BTreeMap::from([("default".into(), PathBuf::new())]);
        assert!(
            reject_legacy_session_state(&session, &applications, &path)
                .unwrap_err()
                .contains("cannot be resumed")
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}
