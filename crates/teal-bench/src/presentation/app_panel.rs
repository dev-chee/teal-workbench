//! 根应用面板的穷举包装与能力转发。
//!
//! 使用枚举变体区分具体面板，避免通过动态向下转换推断编辑、浏览或会话能力。

use super::{BrowserPanel, CustomPanel, ExplorerPanel, TextPanel};
use ratatui::{Frame, layout::Rect};
use teal_workbench_core::{InputEvent, MouseEvent};
use crate::workbench::ui::{
    AgentComposerWidget, CallStackWidget, ClipboardAction, ClipboardOutcome, ComposerWidget,
    DebugConsoleWidget, EditorPanel, PanelKind, PanelStatus, PickerWidget, TestRow, TestRowStatus,
    TestsWidget, VariablesWidget, Widget, WidgetAction,
};

/// 把测试行映射为可持久化的会话负载。
pub(crate) fn session_test_row(row: &TestRow) -> teal_workbench_services::TestSessionRow {
    teal_workbench_services::TestSessionRow {
        name: row.name.clone(),
        source: row.source.clone(),
        path: row.path.clone(),
        line: row.line,
        status: match row.status {
            TestRowStatus::Passed => teal_workbench_services::TestSessionStatus::Passed,
            TestRowStatus::Failed => teal_workbench_services::TestSessionStatus::Failed,
            TestRowStatus::TimedOut => teal_workbench_services::TestSessionStatus::TimedOut,
            TestRowStatus::ExitCode(code) => {
                teal_workbench_services::TestSessionStatus::ExitCode { code }
            }
        },
        message: row.message.clone(),
        output: row.output.clone(),
    }
}

/// 把会话负载还原为测试行。
pub(crate) fn test_row_from_session(row: &teal_workbench_services::TestSessionRow) -> TestRow {
    TestRow {
        name: row.name.clone(),
        source: row.source.clone(),
        path: row.path.clone(),
        line: row.line,
        status: match row.status {
            teal_workbench_services::TestSessionStatus::Passed => TestRowStatus::Passed,
            teal_workbench_services::TestSessionStatus::Failed => TestRowStatus::Failed,
            teal_workbench_services::TestSessionStatus::TimedOut => TestRowStatus::TimedOut,
            teal_workbench_services::TestSessionStatus::ExitCode { code } => {
                TestRowStatus::ExitCode(code)
            }
        },
        message: row.message.clone(),
        output: row.output.clone(),
    }
}

/// 具体应用面板；穷举变体使全局能力可静态检查。
pub enum AppPanel {
    AgentComposer(Box<AgentComposerWidget>),
    Composer(Box<ComposerWidget>),
    DebugConsole(Box<DebugConsoleWidget>),
    Variables(Box<VariablesWidget>),
    CallStack(Box<CallStackWidget>),
    Custom(Box<CustomPanel>),
    Editor(Box<EditorPanel>),
    Explorer(Box<ExplorerPanel>),
    Picker(Box<PickerWidget>),
    Text(Box<TextPanel>),
    Browser(Box<BrowserPanel>),
    Tests(Box<TestsWidget>),
}

impl AppPanel {
    pub(crate) fn allows_global_prompt(&self) -> bool {
        match self {
            Self::Editor(editor) => editor.mode() == teal_workbench_core::EditorMode::Normal,
            Self::Explorer(explorer) => !explorer.address_active(),
            Self::Custom(custom) => !custom.ui.text_input_active(),
            // 输入框与选择器自己解释 `:`/`!` 或过滤输入，不再让全局命令行抢走按键。
            Self::AgentComposer(_)
            | Self::Composer(_)
            | Self::DebugConsole(_)
            | Self::Picker(_) => false,
            Self::Variables(_) | Self::CallStack(_) => true,
            Self::Text(_) | Self::Browser(_) => true,
            Self::Tests(_) => true,
        }
    }

    /// 返回面板是否允许全局窗口前缀键（`Ctrl-w` 与 `Alt` 组合）。
    pub(crate) fn allows_window_keys(&self) -> bool {
        match self {
            Self::AgentComposer(_)
            | Self::Composer(_)
            | Self::DebugConsole(_)
            | Self::Picker(_) => true,
            other => other.allows_global_prompt(),
        }
    }

    pub(crate) fn session_state(&self) -> teal_workbench_services::PanelSessionState {
        use teal_workbench_services::PanelSessionState;
        match self {
            Self::Composer(panel) => PanelSessionState::Text {
                category: "composer".to_owned(),
                title: panel.title(),
                content: panel.text().to_owned(),
                blocks: None,
                resource: String::new(),
                scroll: 0,
                column: 0,
            },
            Self::AgentComposer(panel) => PanelSessionState::Text {
                category: "composer".to_owned(),
                title: panel.title(),
                content: panel.composer().text().to_owned(),
                blocks: None,
                resource: String::new(),
                scroll: 0,
                column: 0,
            },
            // 调试子部件与选择器是运行期视图，不作为持久的编辑器/文本会话保存。
            Self::DebugConsole(panel) => PanelSessionState::Text {
                category: "debug-console".to_owned(),
                title: panel.title(),
                content: String::new(),
                blocks: None,
                resource: String::new(),
                scroll: 0,
                column: 0,
            },
            Self::Variables(panel) => PanelSessionState::Text {
                category: "debug-variables".to_owned(),
                title: panel.title(),
                content: String::new(),
                blocks: None,
                resource: String::new(),
                scroll: 0,
                column: 0,
            },
            Self::CallStack(panel) => PanelSessionState::Text {
                category: "debug-call-stack".to_owned(),
                title: panel.title(),
                content: String::new(),
                blocks: None,
                resource: String::new(),
                scroll: 0,
                column: 0,
            },
            Self::Picker(panel) => PanelSessionState::Text {
                category: "picker".to_owned(),
                title: panel.title(),
                content: panel.query().to_owned(),
                blocks: None,
                resource: String::new(),
                scroll: 0,
                column: 0,
            },
            Self::Editor(editor) => {
                let (scroll, column) = editor.view_state();
                PanelSessionState::Editor {
                    recovery_key: None,
                    path: editor.document().path(),
                    selections: editor.document().selections(),
                    scroll,
                    column,
                }
            }
            Self::Explorer(panel) => PanelSessionState::Explorer {
                tree: panel.session_state(),
            },
            Self::Text(panel) => panel.session_state(),
            Self::Browser(panel) => panel.session_state(),
            Self::Custom(panel) => PanelSessionState::TealCustom {
                name: panel.name.clone(),
                specification: panel.ui.specification(),
                widget_state: panel.ui.state(),
            },
            Self::Tests(panel) => PanelSessionState::Tests {
                rows: panel.rows().iter().map(session_test_row).collect(),
                selected: panel.selected(),
                scroll: panel.scroll(),
                filter: panel.filter().to_owned(),
                summary: panel.summary().to_owned(),
                source: None,
            },
        }
    }

    pub(crate) fn as_custom(&self) -> Option<&CustomPanel> {
        if let Self::Custom(p) = self {
            Some(p)
        } else {
            None
        }
    }

    pub(crate) fn as_composer(&self) -> Option<&ComposerWidget> {
        match self {
            Self::Composer(p) => Some(p),
            Self::AgentComposer(p) => Some(p.composer()),
            _ => None,
        }
    }

    pub(crate) fn as_composer_mut(&mut self) -> Option<&mut ComposerWidget> {
        match self {
            Self::Composer(p) => Some(p),
            Self::AgentComposer(p) => Some(p.composer_mut()),
            Self::DebugConsole(p) => Some(p.composer_mut()),
            _ => None,
        }
    }

    pub(crate) fn as_debug_console(&self) -> Option<&DebugConsoleWidget> {
        if let Self::DebugConsole(p) = self {
            Some(p)
        } else {
            None
        }
    }

    pub(crate) fn as_custom_mut(&mut self) -> Option<&mut CustomPanel> {
        if let Self::Custom(p) = self {
            Some(p)
        } else {
            None
        }
    }

    pub fn as_editor(&self) -> Option<&EditorPanel> {
        if let Self::Editor(p) = self {
            Some(p)
        } else {
            None
        }
    }

    pub fn as_editor_mut(&mut self) -> Option<&mut EditorPanel> {
        if let Self::Editor(p) = self {
            Some(p)
        } else {
            None
        }
    }

    pub fn as_explorer(&self) -> Option<&ExplorerPanel> {
        if let Self::Explorer(p) = self {
            Some(p)
        } else {
            None
        }
    }

    pub fn as_explorer_mut(&mut self) -> Option<&mut ExplorerPanel> {
        if let Self::Explorer(p) = self {
            Some(p)
        } else {
            None
        }
    }

    pub fn as_text(&self) -> Option<&TextPanel> {
        if let Self::Text(p) = self {
            Some(p)
        } else {
            None
        }
    }

    pub fn as_text_mut(&mut self) -> Option<&mut TextPanel> {
        if let Self::Text(p) = self {
            Some(p)
        } else {
            None
        }
    }

    pub fn as_browser(&self) -> Option<&BrowserPanel> {
        if let Self::Browser(p) = self {
            Some(p)
        } else {
            None
        }
    }

    pub fn as_browser_mut(&mut self) -> Option<&mut BrowserPanel> {
        if let Self::Browser(p) = self {
            Some(p)
        } else {
            None
        }
    }

    pub(crate) fn as_tests(&self) -> Option<&TestsWidget> {
        if let Self::Tests(p) = self {
            Some(p)
        } else {
            None
        }
    }

    pub(crate) fn as_tests_mut(&mut self) -> Option<&mut TestsWidget> {
        if let Self::Tests(p) = self {
            Some(p)
        } else {
            None
        }
    }

    fn panel(&self) -> &dyn Widget {
        match self {
            Self::AgentComposer(p) => p.as_ref(),
            Self::Composer(p) => p.as_ref(),
            Self::DebugConsole(p) => p.as_ref(),
            Self::Variables(p) => p.as_ref(),
            Self::CallStack(p) => p.as_ref(),
            Self::Custom(p) => p.as_ref(),
            Self::Editor(p) => p.as_ref(),
            Self::Explorer(p) => p.as_ref(),
            Self::Picker(p) => p.as_ref(),
            Self::Text(p) => p.as_ref(),
            Self::Browser(p) => p.as_ref(),
            Self::Tests(p) => p.as_ref(),
        }
    }

    fn panel_mut(&mut self) -> &mut dyn Widget {
        match self {
            Self::AgentComposer(p) => p.as_mut(),
            Self::Composer(p) => p.as_mut(),
            Self::DebugConsole(p) => p.as_mut(),
            Self::Variables(p) => p.as_mut(),
            Self::CallStack(p) => p.as_mut(),
            Self::Custom(p) => p.as_mut(),
            Self::Editor(p) => p.as_mut(),
            Self::Explorer(p) => p.as_mut(),
            Self::Picker(p) => p.as_mut(),
            Self::Text(p) => p.as_mut(),
            Self::Browser(p) => p.as_mut(),
            Self::Tests(p) => p.as_mut(),
        }
    }
}

impl From<DebugConsoleWidget> for AppPanel {
    fn from(panel: DebugConsoleWidget) -> Self {
        Self::DebugConsole(Box::new(panel))
    }
}

impl From<AgentComposerWidget> for AppPanel {
    fn from(panel: AgentComposerWidget) -> Self {
        Self::AgentComposer(Box::new(panel))
    }
}

impl From<VariablesWidget> for AppPanel {
    fn from(panel: VariablesWidget) -> Self {
        Self::Variables(Box::new(panel))
    }
}

impl From<CallStackWidget> for AppPanel {
    fn from(panel: CallStackWidget) -> Self {
        Self::CallStack(Box::new(panel))
    }
}

impl From<PickerWidget> for AppPanel {
    fn from(panel: PickerWidget) -> Self {
        Self::Picker(Box::new(panel))
    }
}

impl From<TestsWidget> for AppPanel {
    fn from(panel: TestsWidget) -> Self {
        Self::Tests(Box::new(panel))
    }
}

impl From<CustomPanel> for AppPanel {
    fn from(panel: CustomPanel) -> Self {
        Self::Custom(Box::new(panel))
    }
}

impl From<ComposerWidget> for AppPanel {
    fn from(panel: ComposerWidget) -> Self {
        Self::Composer(Box::new(panel))
    }
}

impl From<EditorPanel> for AppPanel {
    fn from(p: EditorPanel) -> Self {
        Self::Editor(Box::new(p))
    }
}

impl From<ExplorerPanel> for AppPanel {
    fn from(p: ExplorerPanel) -> Self {
        Self::Explorer(Box::new(p))
    }
}

impl From<TextPanel> for AppPanel {
    fn from(p: TextPanel) -> Self {
        Self::Text(Box::new(p))
    }
}

impl From<BrowserPanel> for AppPanel {
    fn from(p: BrowserPanel) -> Self {
        Self::Browser(Box::new(p))
    }
}

impl Widget for AppPanel {
    fn poll_action(&mut self) -> WidgetAction {
        self.panel_mut().poll_action()
    }

    fn kind(&self) -> PanelKind {
        self.panel().kind()
    }

    fn title(&self) -> String {
        self.panel().title()
    }

    fn status(&self) -> Option<PanelStatus> {
        self.panel().status()
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        self.panel_mut().handle_input(event)
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: Rect) -> WidgetAction {
        self.panel_mut().handle_mouse(event, area)
    }

    fn handle_menu(&mut self, id: &str) -> WidgetAction {
        self.panel_mut().handle_menu(id)
    }

    fn handle_clipboard(
        &mut self,
        action: ClipboardAction,
        paste: Option<&str>,
    ) -> ClipboardOutcome {
        self.panel_mut().handle_clipboard(action, paste)
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        self.panel_mut().render(frame, area, focused);
    }

    fn cursor(&self, area: Rect) -> Option<(u16, u16)> {
        self.panel().cursor(area)
    }
}
