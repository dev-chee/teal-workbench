//! 顶层终端界面状态、命令路由、服务、恢复与集成。
//!
//! `App` 拥有跨面板策略与全局可变状态；各子模块按工作流拆分实现，但共享同一应用边界。
//! 后台任务通过类型化结果回到事件循环，不能持有面板或布局的可变引用。

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    env,
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::workbench::ui::{
    AgentComposerWidget, CallStackWidget, ClipboardAction, CmdlineAction, CmdlineMode,
    CompletionMenu, ComposerMode, ComposerWidget, ConfirmOverlay, ContextMenu, ContextMenuOverlay,
    DebugConsoleWidget, EditorPanel, FrameGeometry, HoverView, MenuItem, OverlayAction,
    OverlayStack, PanelKind, PanelStore, PickerOverlay, Prompt, VariablesWidget, Widget,
    WidgetAction, render_frame,
};
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEventKind, KeyModifiers as CrosstermModifiers,
    MouseButton as CrosstermMouseButton, MouseEvent as CrosstermMouseEvent,
    MouseEventKind as CrosstermMouseEventKind,
};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use teal::{HostCallSignature, HostError, HostValue, RuntimeType};
use teal_agent::{InlineCompletion, MemoryStore, SkillCatalog};
use teal_host::web::TextBrowser;
use teal_lsp_client::{
    JsonRpcId, LineIndex, LspClient, LspEvent, ServerConfig, TextDocumentChange,
    WorkspaceOperation, parse_workspace_edit,
};
use teal_workbench_automation::{
    AgentSettings, McpSettings, ScriptKind, TrustDecision, TrustStore, WorkbenchAutomationRuntime,
};
use teal_workbench_core::syntax::{GrammarSpec, ParserTable};
use teal_workbench_core::{
    AppCommand, CommandInvocation, CommandRegistry, Direction, InputEvent, Key, KeyModifiers,
    KeymapPreset, LayoutError, LayoutSpec, LayoutTree, Message, MessageLog, MouseButton,
    MouseEvent, MouseEventKind, NodeSpec, PanelId, PanelSize, RecoveryJournal, Rect, ResizeAnchor,
    Severity, SplitAxis, SplitDivider, TextEdit, Transaction, Window, WindowCloseError, WindowId,
    WindowKind, WindowSet, WorkspaceIntent, WorkspaceMutationCoordinator,
};
use teal_workbench_services::{
    ClipboardBackend, MarkdownDocument, PanelSession, RuntimeCache, RuntimeLock, RuntimeLockStatus,
    SearchService, SessionPaths, SessionState, SystemClipboard, load_session, run_command,
    save_session,
};
use tokio::sync::{mpsc, oneshot};

use crate::{
    presentation::{AppPanel, BrowserPanel, ExplorerPanel, TextPanel},
    runtime_bridge::{RuntimeEvent, TaskSupervisor},
};

/// 返回一次记忆维护的稳定身份；相同来源重复执行时保持幂等。
pub(crate) fn memory_maintenance_run(sources: &[teal_agent::MemoryMaintenanceSource]) -> String {
    format!(
        "memory-maintenance/{}",
        &memory_maintenance_digest(sources)[..16]
    )
}

/// 返回与顺序无关的来源摘要，用于维护身份与来源变化检测。
pub(crate) fn memory_maintenance_digest(sources: &[teal_agent::MemoryMaintenanceSource]) -> String {
    let mut lines = sources
        .iter()
        .map(|source| format!("{}#{}:{}", source.run, source.event, source.digest))
        .collect::<Vec<_>>();
    lines.sort();
    let mut digest = Sha256::new();
    for line in lines {
        digest.update(line.as_bytes());
        digest.update([0xff]);
    }
    format!("{:x}", digest.finalize())
}

#[path = "app/adapters/agent.rs"]
pub(crate) mod agent;
mod agent_app;
#[path = "app/runs.rs"]
mod runs;
#[cfg(test)]
mod runs_tests;

/// 测试期间把配置用户根目录固定到当前临时会话，并串行化环境变量访问。
#[cfg(test)]
pub(crate) mod test_support {
    use std::{
        env,
        ffi::OsString,
        path::Path,
        sync::{Mutex, MutexGuard, OnceLock},
    };

    pub(crate) fn environment_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    pub(crate) struct TestHomeGuard {
        previous: Option<OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl TestHomeGuard {
        pub(crate) fn set(home: &Path) -> Self {
            let lock = environment_lock()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let previous = env::var_os("TEAL_HOME");
            // SAFETY: the guard serializes every test mutation of TEAL_HOME and restores it on drop.
            unsafe { env::set_var("TEAL_HOME", home) };
            Self {
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for TestHomeGuard {
        fn drop(&mut self) {
            // SAFETY: the guard still owns the process-wide test environment lock.
            match self.previous.take() {
                Some(value) => unsafe { env::set_var("TEAL_HOME", value) },
                None => unsafe { env::remove_var("TEAL_HOME") },
            }
        }
    }
}
pub(crate) mod agent_questions;
#[cfg(test)]
mod analysis_graph_tests;
pub(crate) mod background;
pub(crate) mod bootstrap;
mod commands;
#[cfg(test)]
mod diagnostics_tests;
mod engineering;
mod events;
#[path = "app/adapters/extensions.rs"]
pub(crate) mod extensions;
#[cfg(test)]
mod extensions_tests;
pub(crate) mod files;
pub(crate) mod knowledge;
#[cfg(test)]
mod loopback_tests;
pub(crate) mod lsp;
pub(crate) mod mutation;
pub(crate) mod scripting;
mod session;
pub(crate) mod tests_view;
mod views;
#[cfg(test)]
mod window_tests;
mod windows;
#[path = "app/adapters/workspace.rs"]
pub(crate) mod workspace;

use scripting::register_automation_bridge;
use session::open_editor;

/// 独立于 Clap 的已解析启动选项。
pub struct AppOptions {
    /// 可选文件或目录；省略时启用会话恢复或当前目录。
    pub path: Option<PathBuf>,

    /// 显式的起始文件；设置时主窗口是编辑该文件的 Editor 窗口。
    ///
    /// 命令行传入文件时使用它，从而与「传入目录」区分：目录启动落到 Agent 主窗口，
    /// 需要文件浏览时再打开资源管理器窗口。
    pub start_file: Option<PathBuf>,

    /// 禁用用户/工作区配置和会话恢复。
    pub safe_mode: bool,

    /// 禁用 Teal 配置，同时保留会话和智能体。
    pub no_config: bool,

    /// 显式会话文件覆盖项。
    pub session: Option<PathBuf>,
    /// 显式启用的模型协议抓包根目录。
    pub model_protocol_capture: Option<PathBuf>,
}

/// 确认框背后的实际操作；显示文字不作为恢复操作身份或权限的依据。
enum PendingConfirmation {
    ExtensionLoad(Box<extensions::Loaded>),
    ExtensionApproval {
        extension: String,
        receipt: String,
    },
    LspCommand {
        client: Arc<LspClient>,
        command: serde_json::Value,
        edit: Option<serde_json::Value>,
    },
    Agent(Box<teal_workbench_services::tools::ApprovalPrompt>),
    AgentMutation {
        approval_id: String,
        mutation: Box<MutationRequest>,
    },
    Close(PanelId),
    CloseWindow(WindowId),
    /// 用 Agent 代码块覆盖工作区文件。
    AgentWrite {
        /// 目标工作区路径。
        path: PathBuf,

        /// 代码块正文。
        source: String,
    },
    Quit,
    RestoreSession(Box<SessionState>),
    ExplorerMove {
        panel: PanelId,
        destination: PathBuf,
    },
    ExplorerTrash(PanelId),
    ExplorerDelete(PanelId),
}

/// 当前浮层关闭后需要继续处理的类型化上下文。
enum PendingOverlay {
    DurableQuestion {
        store: Arc<teal_agent::RuntimeStore>,
        request: teal_agent::InteractionRequest,
    },
    Lsp(Box<lsp::Picker>),
    Confirmation(PendingConfirmation),
    BufferPicker(Vec<PanelId>),
    /// 为指定脚本选择运行时配置档。
    ScriptProfile {
        /// 脚本名称。
        name: String,

        /// 选择器展示的配置档名称。
        profiles: Vec<String>,
    },
    Context(ContextTarget),
    AgentPicker,
    /// 缺少共享默认值时选择供应商。
    ModelProviderPicker {
        /// 触发模型准备的原始请求；主动切换时可以为空。
        pending_request: Option<String>,
    },
    /// 选择供应商下的认证资料。
    ModelAuthProfilePicker {
        /// 触发模型准备的原始请求；主动切换时可以为空。
        pending_request: Option<String>,
        /// 已选供应商。
        provider: teal_model::ProviderId,
    },
    /// 选择认证资料下的模型。
    ModelPicker {
        /// 触发模型准备的原始请求；主动切换时可以为空。
        pending_request: Option<String>,
        /// 已选供应商。
        provider: teal_model::ProviderId,
        /// 已选认证资料。
        auth_profile: teal_model::AuthProfileName,
    },
    /// 在完全匹配的模型预设中选择一个。
    ModelProfilePicker {
        /// 触发模型准备的原始请求；主动切换时可以为空。
        pending_request: Option<String>,
        /// 已解析的模型目录项。
        selection: teal_model::AvailableModel,
    },
    ChangeSet(Box<ChangeSetRequest>),
}

#[derive(Clone, Copy)]
enum ContextTarget {
    Panel(PanelId),
    Cmdline,
}

#[derive(Clone, Copy)]
pub(crate) enum PendingPrompt {
    LspRename(PanelId),
    Rename(PanelId),
    CopyTo(PanelId),
    MoveTo(PanelId),
}

/// 等待用户命名新模型预设的输入上下文。
pub(crate) struct PendingModelProfileName {
    /// 触发模型准备的原始请求；主动切换时可以为空。
    pending_request: Option<String>,
    /// 供应商。
    provider: teal_model::ProviderId,
    /// 认证资料。
    auth_profile: teal_model::AuthProfileName,
    /// 模型。
    model: teal_model::ModelId,
}

#[derive(Clone, Copy)]
enum MouseCapture {
    Panel(PanelId),
    Divider(SplitDivider),
    /// 拖动浮动窗口标题栏；保留按下时的指针位置以计算位移。
    WindowMove {
        window: WindowId,
        origin: (u16, u16),
    },
    /// 拖动浮动窗口右下角手柄改变尺寸。
    WindowResize {
        window: WindowId,
        /// 拖动所在的边或角。
        anchor: ResizeAnchor,
        origin: (u16, u16),
    },
}

#[derive(Clone, Copy)]
struct LastClick {
    button: MouseButton,
    column: u16,
    row: u16,
    at: Instant,
    count: u8,
}

#[derive(Default)]
struct MouseNormalizer {
    last_click: Option<LastClick>,
}

impl MouseNormalizer {
    fn normalize(&mut self, event: CrosstermMouseEvent, now: Instant) -> MouseEvent {
        let kind = match event.kind {
            CrosstermMouseEventKind::Down(button) => MouseEventKind::Down(mouse_button(button)),
            CrosstermMouseEventKind::Up(button) => MouseEventKind::Up(mouse_button(button)),
            CrosstermMouseEventKind::Drag(button) => MouseEventKind::Drag(mouse_button(button)),
            CrosstermMouseEventKind::Moved => MouseEventKind::Moved,
            CrosstermMouseEventKind::ScrollUp => MouseEventKind::ScrollUp,
            CrosstermMouseEventKind::ScrollDown => MouseEventKind::ScrollDown,
            CrosstermMouseEventKind::ScrollLeft => MouseEventKind::ScrollLeft,
            CrosstermMouseEventKind::ScrollRight => MouseEventKind::ScrollRight,
        };

        let click_count = if let MouseEventKind::Down(button) = kind {
            let count = self.last_click.map_or(1, |last| {
                if last.button == button
                    && last.column == event.column
                    && last.row == event.row
                    && now.saturating_duration_since(last.at) <= Duration::from_millis(500)
                {
                    last.count.saturating_add(1).min(3)
                } else {
                    1
                }
            });
            self.last_click = Some(LastClick {
                button,
                column: event.column,
                row: event.row,
                at: now,
                count,
            });
            count
        } else {
            0
        };

        MouseEvent {
            kind,
            column: event.column,
            row: event.row,
            modifiers: normalize_modifiers(event.modifiers),
            click_count,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SessionRestorePolicy {
    Always,
    Never,

    #[default]
    Prompt,
}

impl SessionRestorePolicy {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            "prompt" => Some(Self::Prompt),
            _ => None,
        }
    }
}

/// 恢复日志已经消费到的修订及文本基线，与当前视图的选择和焦点无关。
struct JournalTracker {
    journal: RecoveryJournal,
    revision: u64,
    text: String,
}

/// 单次脚本读取快照与待应用效果；执行时由宿主回调填充，结束后由应用消费。
#[derive(Default)]
struct BridgeState {
    target: Option<(teal_workbench_core::BufferId, u64)>,
    panel: Option<PanelId>,
    panel_event: std::collections::BTreeMap<String, String>,
    workspace: PathBuf,
    path: String,
    content: String,
    pending: Vec<BridgeAction>,
    offline: bool,
}

/// 人工编辑中的三方合并，绑定最初输入和承载结果的缓冲区。
struct PendingMergeReview {
    base_path: PathBuf,
    ours_path: PathBuf,
    theirs_path: PathBuf,
    target_relative: PathBuf,
    plan: teal_workbench_services::agent_algorithms::ThreeWayMerge,
    buffer: teal_workbench_core::BufferId,
}

/// 虚拟机执行后才能在应用线程应用的效果。
enum BridgeAction {
    /// 替换目标必须仍是脚本读取时的共享缓冲区修订。
    Replace(teal_workbench_core::BufferId, u64, String),
    Render(PanelId, String),
    Open(PathBuf),
    Message(String),
}

/// 长期存活的应用程序状态与跨层策略边界。
///
/// 面板实例、空间布局和共享文档分别拥有状态，任何后台完成结果都通过事件循环接回。
/// 创建应用不会替调用方接管终端；入口必须先建立终端守卫，并在运行结束后调用清理方法。
pub struct App {
    /// 显式测试会话的配置根；持有到 App 销毁，防止初始化后的配置调用回到用户目录。
    #[cfg(test)]
    pub(crate) test_home_guard: Option<test_support::TestHomeGuard>,

    // 工作区和共享文档是资源身份的来源，面板关闭与缓冲区释放不能混为一谈。
    pub(crate) workspace: PathBuf,
    /// 禁止在安全模式中启动源码 Agent 或访问 RuntimeStore。
    pub(crate) safe_mode: bool,
    pub(crate) panels: PanelStore<AppPanel>,
    buffers: teal_workbench_core::BufferStore,
    mutations: WorkspaceMutationCoordinator,
    io: mutation::State,

    // 交互和空间状态独立于文件存储，浮层优先消费输入；
    // 窗口集合拥有每个工作上下文的布局，非活动窗口保留自己的面板排布。
    windows: WindowSet,
    overlays: OverlayStack,
    cmdline: Option<Prompt>,
    cmdline_history: Vec<String>,
    /// 当前 `:` 命令行的补全菜单；非命令模式或已提交时为空。
    completion_menu: CompletionMenu,
    /// 被动 hover 浮窗；显示 LSP 文档并跟随光标。
    hover: HoverView,
    commands: CommandRegistry,
    messages: MessageLog,
    keymap: KeymapPreset,

    // 每个浏览面板保留当前请求代次，迟到结果不能替换新导航。
    browser_requests: HashMap<PanelId, u64>,
    browser_sequence: u64,
    grammars: HashMap<String, GrammarSpec>,
    grammar_tables: HashMap<String, ParserTable>,
    session_restore_policy: SessionRestorePolicy,
    /// 是否启用用户与工作区 Teal 配置；禁用时不读取分层配置。
    config_enabled: bool,

    // 脚本运行时、信任和桥接各有生命周期，不从面板导航推导脚本授权。
    teal: WorkbenchAutomationRuntime,
    trust: TrustStore,
    script_commands: HashMap<String, scripting::Registration>,
    scripting: scripting::State,
    extensions: extensions::State,
    bridge: Arc<Mutex<BridgeState>>,

    // 工作台会话描述视图；日志和恢复标识以共享缓冲区为单位。
    paths: SessionPaths,
    session_path: PathBuf,
    journals: HashMap<teal_workbench_core::BufferId, JournalTracker>,
    recovery_keys: HashMap<teal_workbench_core::BufferId, String>,
    path_generations: HashMap<PathBuf, u64>,
    deferred_opens: Vec<(PathBuf, bool, Option<serde_json::Value>)>,

    // 命中测试使用实际绘制的几何，拖动保留最初捕获的面板或分隔线。
    last_area: Rect,
    frame_geometry: FrameGeometry,
    mouse_normalizer: MouseNormalizer,
    mouse_capture: Option<MouseCapture>,
    clipboard: Box<dyn ClipboardBackend>,
    should_quit: bool,

    // 智能体展示和类型化会话分开维护，批准响应与工具结果不得从展示文本推断。
    agent_app_tx: Option<mpsc::Sender<agent_app::AgentAppInput>>,
    agent_app_services: Option<teal_agent::RuntimeServices>,
    agent_app_active_context: Option<teal_agent::ActionContext>,
    agent_app_view_panel: Option<PanelId>,
    inline_completion: Option<InlineCompletion>,
    agent_generation: u64,
    workspace_tools: Arc<WorkspaceToolRuntime>,
    agent_permissions: Arc<ChannelApproval>,
    model_access: teal_model::ModelAccess,
    local_model_providers: BTreeSet<String>,
    agent_memory: Arc<MemoryStore>,
    agent_skills: Arc<SkillCatalog>,
    agent_knowledge: Arc<teal_workbench_services::WorkbenchKnowledge>,
    runtime_lock_status: RuntimeLockStatus,
    runtime_lock: Option<RuntimeLock>,
    runtime_packages: Vec<teal_workbench_services::RuntimePackage>,
    runtime_core_catalog_digest: String,
    runtime_extension_catalog_digest: Option<String>,
    runtime_cache: RuntimeCache,
    connectivity: teal_workbench_services::agent_algorithms::ConnectivityMode,
    pub(crate) debug_control:
        Option<std::sync::mpsc::Sender<crate::integration::debug::DebugCommand>>,
    pub(crate) debug_panel: Option<PanelId>,
    /// 当前调试会话绑定的 Debug 窗口。
    pub(crate) debug_window: Option<WindowId>,
    /// Tests 窗口的列表、输出面板与窗口身份。
    pub(crate) tests_panels: Option<tests_view::TestsPanels>,
    /// 最近一次 `:test` 使用的源码路径，供跳转与单测重跑。
    pub(crate) tests_source: Option<PathBuf>,
    /// 是否已有一次测试运行在后台进行；同一时间只允许一个运行。
    pub(crate) tests_run_in_flight: bool,
    agent_settings: BTreeMap<String, (PathBuf, AgentSettings)>,
    agent_app_scripts: BTreeMap<String, PathBuf>,
    mcp_settings: BTreeMap<String, (PathBuf, McpSettings)>,
    active_agent: Option<String>,
    pending_inline_completion: Option<background::PendingInlineCompletion>,
    pending_approvals: std::collections::VecDeque<teal_workbench_services::tools::ApprovalPrompt>,
    pending_mutations: std::collections::VecDeque<MutationRequest>,
    pending_changesets: std::collections::VecDeque<ChangeSetRequest>,
    agent_panel: Option<PanelId>,
    pending_agent_profile_switch: Option<teal_model::AuthProfileName>,
    agent_session_store: Option<Arc<teal_agent::RuntimeStore>>,
    pending_merge: Option<PendingMergeReview>,

    // 应用只在此事件接收边界更新全局状态；后台任务交由监督器统一回收。
    runtime_rx: mpsc::Receiver<RuntimeEvent>,
    pub(crate) tasks: TaskSupervisor,
    observation_health: Option<Arc<teal_workbench_services::observability::Health>>,
    last_observation_warning: Option<Instant>,
    pending_overlay: Option<PendingOverlay>,
    pending_prompt: Option<PendingPrompt>,
    pending_model_profile_name: Option<PendingModelProfileName>,
    window_prefix: bool,
    lsp: teal_lsp_client::LspManager,
    lsp_ui: lsp::State,
    diagnostics: teal_lsp_client::DiagnosticStore,
    pub(crate) runtime_tx: crate::runtime_bridge::RuntimeSender,
    lsp_events_tx: mpsc::Sender<(Arc<LspClient>, LspEvent)>,
    background_tx: mpsc::Sender<Message>,
}

impl App {
    #[cfg(test)]
    pub(crate) fn set_model_access_for_test(
        &mut self,
        auth: teal_model::AuthManager,
        models: teal_model::ModelFactoryRegistry,
    ) {
        // 测试注入的认证与模型工厂自带 provider 目录，不需要读取开发机的用户配置。
        self.model_access = teal_model::ModelAccess::from_parts(
            std::sync::Arc::new(teal_model::ModelConfiguration::Empty),
            auth,
            models,
        );
    }

    #[cfg(test)]
    pub(crate) fn set_configured_model_access_for_test(
        &mut self,
        configuration: teal_model::SharedModelConfiguration,
        auth: teal_model::AuthManager,
        models: teal_model::ModelFactoryRegistry,
    ) {
        self.model_access = teal_model::ModelAccess::from_parts(configuration, auth, models);
    }

    #[cfg(test)]
    pub(crate) fn auth_for_test(&self) -> teal_model::AuthManager {
        self.model_access.auth()
    }

    #[cfg(test)]
    pub(crate) fn models_for_test(&self) -> teal_model::ModelFactoryRegistry {
        self.model_access.models()
    }

    pub(crate) fn set_observation_health(
        &mut self,
        health: Arc<teal_workbench_services::observability::Health>,
    ) {
        self.observation_health = Some(health);
    }

    /// 运行终端输入、计时器、后台服务、智能体事件和批准流程。
    ///
    /// 调用方提供已初始化终端。退出或出错后仍须调用 `shutdown` 处理会话和服务，
    /// 再由入口恢复终端；本方法返回并不意味着后台任务已全部结束。
    pub async fn run(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut events = EventStream::new();

        self.scripting.running = true;
        self.fire_hook("app.started", serde_json::json!({}));
        self.fire_hook(
            "workspace.opened",
            serde_json::json!({"workspace":self.workspace}),
        );

        let mut tick = tokio::time::interval(Duration::from_millis(100));
        while !self.should_quit {
            if self.observation_health.as_ref().is_some_and(|health| {
                health
                    .write_failures
                    .load(std::sync::atomic::Ordering::Relaxed)
                    > 0
                    || health
                        .export_failures
                        .load(std::sync::atomic::Ordering::Relaxed)
                        > 0
            }) && self
                .last_observation_warning
                .is_none_or(|time| time.elapsed() >= Duration::from_secs(30))
            {
                self.last_observation_warning = Some(Instant::now());
                self.message(
                    Severity::Warning,
                    "observability",
                    "logging or telemetry output is unavailable; editing can continue",
                );
            }

            // 绘制下一次输入几何信息前，先应用已就绪的面板与服务结果。
            let panel_ids = self.panels.iter().map(|(id, _)| id).collect::<Vec<_>>();
            for id in panel_ids {
                if let Some(panel) = self.panels.get_mut(id) {
                    let action = panel.poll_action();
                    self.apply_panel_action(id, action);
                }
            }
            self.process_buffer_changes();
            self.pump_mutations();
            self.pump_scripts();
            self.pump_approvals();
            self.pump_lsp_results();

            terminal.draw(|frame| {
                self.frame_geometry = render_frame(
                    frame,
                    &self.windows,
                    &mut self.panels,
                    self.cmdline.as_ref(),
                    &self.completion_menu,
                    &self.messages,
                );
                self.last_area = self.frame_geometry.content;
                // 浮层是"弹出即窗口"的统一形态：每次绘制后同步登记或移除其窗口。
                let content = self.frame_geometry.content;
                self.overlays.sync_windows(&mut self.windows, content);
                self.overlays.render(frame);
                if self.hover.is_visible() {
                    let anchor = self.hover_anchor(frame);
                    self.hover.render(frame, tui_rect(content), anchor);
                }
            })?;

            tokio::select! {
                event = events.next() => {
                    match event {
                        Some(Ok(event)) => self.handle_terminal_event(&event),
                        Some(Err(error)) => self.message(Severity::Error, "terminal", error.to_string()),
                        None => self.should_quit = true,
                    }
                }
                _ = tick.tick() => {self.script_tick();}
                Some(event) = self.runtime_rx.recv() => self.handle_runtime_event(event).await,
            }
        }

        Ok(())
    }

    pub(crate) async fn shutdown(&mut self, save: bool) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(control) = &self.debug_control {
            let _ = control.send(crate::integration::debug::DebugCommand::Stop);
        }
        for prompt in self.pending_approvals.drain(..) {
            let _ = prompt.response.send(teal_agent::ApprovalResponse::Deny);
        }
        for mutation in self.pending_mutations.drain(..) {
            let _ = mutation.response.send(Err(
                "Agent change review was cancelled during shutdown".into(),
            ));
        }
        for change_set in self.pending_changesets.drain(..) {
            let _ = change_set
                .response
                .send(Err("ChangeSet review was cancelled during shutdown".into()));
        }
        if let Some(PendingOverlay::ChangeSet(change_set)) = self.pending_overlay.take() {
            let _ = change_set
                .response
                .send(Err("ChangeSet review was cancelled during shutdown".into()));
        }

        // 停止脚本可能加入文件任务，因此随后再尝试排空修改队列。
        self.stop_scripts().await;
        self.stop_mutations().await;

        // 保留持久化错误，直到其他服务停止后再返回。
        let session_result = if save { self.save_session() } else { Ok(()) };
        for server in self.lsp.iter() {
            let _ = server.client.shutdown().await;
        }

        let panels = self.panels.iter().map(|(id, _)| id).collect::<Vec<_>>();
        let workers = panels
            .into_iter()
            .filter_map(|id| {
                self.panels
                    .get_mut(id)
                    .and_then(AppPanel::as_editor_mut)
                    .and_then(crate::workbench::ui::EditorPanel::stop_syntax)
            })
            .collect::<Vec<_>>();
        if !workers.is_empty() {
            let stopped = crate::runtime_bridge::spawn_blocking(move || {
                let mut succeeded = true;
                for worker in workers {
                    succeeded &= worker.join().is_ok();
                }
                succeeded
            });
            if !matches!(
                tokio::time::timeout(Duration::from_secs(1), stopped).await,
                Ok(Ok(true))
            ) {
                tracing::warn!(event.name = "cancelled");
            }
        }

        self.tasks.shutdown().await;

        session_result?;
        Ok(())
    }

    pub(crate) fn message(&mut self, severity: Severity, source: &str, text: impl Into<String>) {
        if matches!(severity, Severity::Error) {
            tracing::Span::current().record("operation.result", "error");
            tracing::Span::current().record("otel.status_code", "ERROR");
            // 仅记录元数据：界面文本可能包含源码、工具输出或凭据。
            tracing::error!(event.name = "failed");
        }

        self.messages.push(Message::new(severity, source, text));
    }
}

fn normalize_key(code: KeyCode, modifiers: CrosstermModifiers) -> Option<InputEvent> {
    let key = match code {
        KeyCode::Char(character) => Key::Char(character),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::F(number) => Key::Function(number),
        _ => return None,
    };

    Some(InputEvent {
        key,
        modifiers: normalize_modifiers(modifiers),
    })
}

fn normalize_modifiers(modifiers: CrosstermModifiers) -> KeyModifiers {
    KeyModifiers {
        control: modifiers.contains(CrosstermModifiers::CONTROL),
        alt: modifiers.contains(CrosstermModifiers::ALT),
        shift: modifiers.contains(CrosstermModifiers::SHIFT),
    }
}

const fn mouse_button(button: CrosstermMouseButton) -> MouseButton {
    match button {
        CrosstermMouseButton::Left => MouseButton::Left,
        CrosstermMouseButton::Right => MouseButton::Right,
        CrosstermMouseButton::Middle => MouseButton::Middle,
    }
}

const fn tui_rect(rect: Rect) -> ratatui::layout::Rect {
    ratatui::layout::Rect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

pub(crate) use teal_workbench_core::ensure_workspace_path;

impl App {
    /// 返回 hover 浮窗的锚点：聚焦面板光标的绝对单元格。
    ///
    /// 没有可见光标时返回 `None`，由浮窗退回到内容区左上角。
    fn hover_anchor(&self, frame: &ratatui::Frame<'_>) -> Option<(u16, u16)> {
        let _ = frame;
        let focused = self.windows.focused();
        let geometry = self.frame_geometry.panel(focused)?;
        let body = tui_rect(geometry.body);
        let (x, y) = self.panels.get(focused)?.cursor(body)?;
        Some((body.x.saturating_add(x), body.y.saturating_add(y)))
    }
}

fn mutation_text(current: &str, kind: MutationKind) -> Result<String, String> {
    match kind {
        MutationKind::Write(content) | MutationKind::WriteNew(content) => {
            Ok(content.replace("\r\n", "\n").replace('\r', "\n"))
        }
        MutationKind::Replace { old, new } => {
            let old = old.replace("\r\n", "\n").replace('\r', "\n");
            let new = new.replace("\r\n", "\n").replace('\r', "\n");
            let count = current.matches(&old).count();
            if count == 1 {
                Ok(current.replacen(&old, &new, 1))
            } else {
                Err(format!(
                    "replacement target must occur exactly once; found {count}"
                ))
            }
        }
        MutationKind::ApplyPatch {
            expected_sha256,
            patch,
        } => {
            if teal_workbench_services::agent_algorithms::digest(current.as_bytes())
                != expected_sha256.to_lowercase()
            {
                return Err("patch input digest does not match the current document".into());
            }
            apply_unified_patch(current, &patch)
        }
        MutationKind::CreateDirectory | MutationKind::Move { .. } | MutationKind::Delete => {
            Err("resource mutation cannot be converted to text".into())
        }
    }
}

fn apply_unified_patch(current: &str, patch: &str) -> Result<String, String> {
    let normalized = current.replace("\r\n", "\n").replace('\r', "\n");
    let mut final_newline = normalized.ends_with('\n');
    let source = if normalized.is_empty() {
        Vec::new()
    } else {
        normalized
            .strip_suffix('\n')
            .unwrap_or(&normalized)
            .split('\n')
            .collect::<Vec<_>>()
    };
    let patch = patch.replace("\r\n", "\n").replace('\r', "\n");
    let lines = patch.lines().collect::<Vec<_>>();
    let mut output = Vec::new();
    let mut source_index = 0_usize;
    let mut line_index = 0_usize;
    let mut hunks = 0_usize;
    while line_index < lines.len() {
        let line = lines[line_index];
        if line.starts_with("--- ") || line.starts_with("+++ ") || line.is_empty() {
            line_index += 1;
            continue;
        }
        if !line.starts_with("@@ ") {
            return Err(format!("unexpected unified diff line {}", line_index + 1));
        }
        let (old_start, old_count, new_start, new_count) = parse_hunk_header(line)?;
        if (old_count > 0 && old_start == 0) || (new_count > 0 && new_start == 0) {
            return Err("non-empty patch ranges must start at line one or later".into());
        }
        let target = if old_count == 0 {
            old_start
        } else {
            old_start - 1
        };
        if target < source_index || target > source.len() {
            return Err("patch hunk source range is invalid or out of order".into());
        }
        output.extend(source[source_index..target].iter().copied());
        let expected_new_start = if new_count == 0 {
            output.len()
        } else {
            output.len() + 1
        };
        if new_start != expected_new_start {
            return Err("patch hunk destination range is inconsistent".into());
        }
        source_index = target;
        line_index += 1;
        let mut consumed = 0_usize;
        let mut produced = 0_usize;
        let mut last_operation = None;
        let mut new_side_has_no_newline = false;
        while line_index < lines.len() && !lines[line_index].starts_with("@@ ") {
            let operation = lines[line_index];
            if operation.starts_with("--- ") || operation.starts_with("+++ ") {
                return Err("file headers cannot occur inside a patch hunk".into());
            }
            if operation == "\\ No newline at end of file" {
                match last_operation {
                    Some('+') | Some(' ') => new_side_has_no_newline = true,
                    Some('-') => {}
                    _ => return Err("newline marker has no preceding patch line".into()),
                }
                line_index += 1;
                continue;
            }
            let (kind, text) = operation
                .split_at_checked(1)
                .ok_or_else(|| format!("patch line {} has no operation prefix", line_index + 1))?;
            match kind {
                " " => {
                    if source.get(source_index).copied() != Some(text) {
                        return Err(format!(
                            "patch context differs at source line {}",
                            source_index + 1
                        ));
                    }
                    output.push(text);
                    source_index += 1;
                    consumed += 1;
                    produced += 1;
                    last_operation = Some(' ');
                }
                "-" => {
                    if source.get(source_index).copied() != Some(text) {
                        return Err(format!(
                            "patch removal differs at source line {}",
                            source_index + 1
                        ));
                    }
                    source_index += 1;
                    consumed += 1;
                    last_operation = Some('-');
                }
                "+" => {
                    output.push(text);
                    produced += 1;
                    last_operation = Some('+');
                }
                _ => {
                    return Err(format!(
                        "invalid patch operation at line {}",
                        line_index + 1
                    ));
                }
            }
            line_index += 1;
        }
        if consumed != old_count || produced != new_count {
            return Err("patch hunk line counts do not match its header".into());
        }
        if source_index == source.len() {
            final_newline = match last_operation {
                Some('+') | Some(' ') => !new_side_has_no_newline,
                Some('-') => !output.is_empty(),
                None => final_newline,
                _ => unreachable!("patch operation is validated above"),
            };
        }
        hunks += 1;
    }
    if hunks == 0 {
        return Err("patch does not contain a hunk".into());
    }
    output.extend(source[source_index..].iter().copied());
    let mut result = output.join("\n");
    if final_newline {
        result.push('\n');
    }
    Ok(result)
}

fn parse_hunk_header(header: &str) -> Result<(usize, usize, usize, usize), String> {
    let end = header[3..]
        .find(" @@")
        .ok_or_else(|| "patch hunk header is malformed".to_owned())?
        + 3;
    let mut ranges = header[3..end].split_whitespace();
    let old = ranges
        .next()
        .and_then(|value| value.strip_prefix('-'))
        .ok_or_else(|| "patch hunk old range is missing".to_owned())?;
    let new = ranges
        .next()
        .and_then(|value| value.strip_prefix('+'))
        .ok_or_else(|| "patch hunk new range is missing".to_owned())?;
    if ranges.next().is_some() {
        return Err("patch hunk header contains extra ranges".into());
    }
    let parse = |value: &str| -> Result<(usize, usize), String> {
        let mut values = value.split(',');
        let start = values
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| "patch hunk start is invalid".to_owned())?;
        let count = values.next().map_or(Ok(1), |value| {
            value
                .parse()
                .map_err(|_| "patch hunk count is invalid".to_owned())
        })?;
        if values.next().is_some() {
            return Err("patch hunk range is invalid".into());
        }
        Ok((start, count))
    };
    let (old_start, old_count) = parse(old)?;
    let (new_start, new_count) = parse(new)?;
    Ok((old_start, old_count, new_start, new_count))
}

#[cfg(test)]
mod tests;

use teal_workbench_services::tools::{
    ChangeSetRequest, ChannelApproval, MutationKind, MutationRequest, WorkspaceToolRuntime,
};
