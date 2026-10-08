//! 调试窗口的具名子部件。
//!
//! `DebugConsoleWidget` 是命令控制台，`VariablesWidget` 与 `CallStackWidget` 是只读的
//! 结构化视图。它们复用已有的输入/文本部件，但拥有稳定名称与状态，便于在 Debug 窗口
//! 模板、会话恢复和命中测试中按语义引用。

use ratatui::{Frame, layout::Rect as TuiRect};
use teal_workbench_core::{InputEvent, MouseEvent};

use super::panel::{ClipboardAction, ClipboardOutcome, PanelKind, PanelStatus, WidgetAction};
use super::widget::{Widget, WidgetChrome};
use super::{ComposerMode, ComposerWidget};
use crate::presentation::TextPanel;

/// 调试命令控制台。
pub struct DebugConsoleWidget {
    console: ComposerWidget,
}

impl DebugConsoleWidget {
    /// 创建一个空的调试控制台。
    #[must_use]
    pub fn new() -> Self {
        Self {
            console: ComposerWidget::new(ComposerMode::Command, "Command"),
        }
    }

    /// 返回控制台基于内容的首选行数。
    #[must_use]
    pub fn preferred_rows(&self) -> u16 {
        self.console.preferred_rows()
    }

    /// 返回底层命令输入部件。
    #[must_use]
    pub fn composer(&self) -> &ComposerWidget {
        &self.console
    }

    /// 返回底层命令输入部件的可变引用。
    pub fn composer_mut(&mut self) -> &mut ComposerWidget {
        &mut self.console
    }
}

impl Default for DebugConsoleWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for DebugConsoleWidget {
    fn kind(&self) -> PanelKind {
        self.console.kind()
    }

    fn title(&self) -> String {
        self.console.title()
    }

    fn status(&self) -> Option<PanelStatus> {
        self.console.status()
    }

    fn chrome(&self) -> WidgetChrome {
        self.console.chrome()
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: TuiRect, focused: bool) {
        self.console.render(frame, area, focused);
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        self.console.handle_input(event)
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: TuiRect) -> WidgetAction {
        self.console.handle_mouse(event, area)
    }

    fn handle_clipboard(
        &mut self,
        action: ClipboardAction,
        paste: Option<&str>,
    ) -> ClipboardOutcome {
        self.console.handle_clipboard(action, paste)
    }

    fn handle_menu(&mut self, id: &str) -> WidgetAction {
        self.console.handle_menu(id)
    }

    fn cursor(&self, area: TuiRect) -> Option<(u16, u16)> {
        self.console.cursor(area)
    }

    fn focus_next(&mut self, backwards: bool) -> bool {
        self.console.focus_next(backwards)
    }
}

/// 暂停帧的变量视图。
pub struct VariablesWidget {
    panel: TextPanel,
}

impl VariablesWidget {
    /// 创建一个空的变量视图。
    #[must_use]
    pub fn new() -> Self {
        Self {
            panel: TextPanel::plain(PanelKind::Messages, "Variables", ""),
        }
    }

    /// 替换变量列表正文。
    pub fn set_variables(&mut self, variables: &str) {
        self.panel.replace_plain(variables);
    }
}

impl Default for VariablesWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for VariablesWidget {
    fn kind(&self) -> PanelKind {
        self.panel.kind()
    }

    fn title(&self) -> String {
        self.panel.title()
    }

    fn status(&self) -> Option<PanelStatus> {
        self.panel.status()
    }

    fn chrome(&self) -> WidgetChrome {
        self.panel.chrome()
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: TuiRect, focused: bool) {
        self.panel.render(frame, area, focused);
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        self.panel.handle_input(event)
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: TuiRect) -> WidgetAction {
        self.panel.handle_mouse(event, area)
    }

    fn cursor(&self, area: TuiRect) -> Option<(u16, u16)> {
        self.panel.cursor(area)
    }
}

/// 暂停帧的调用栈视图。
pub struct CallStackWidget {
    panel: TextPanel,
}

impl CallStackWidget {
    /// 创建一个空的调用栈视图。
    #[must_use]
    pub fn new() -> Self {
        Self {
            panel: TextPanel::plain(PanelKind::Messages, "CallStack", ""),
        }
    }

    /// 替换调用栈正文。
    pub fn set_frames(&mut self, frames: &str) {
        self.panel.replace_plain(frames);
    }
}

impl Default for CallStackWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for CallStackWidget {
    fn kind(&self) -> PanelKind {
        self.panel.kind()
    }

    fn title(&self) -> String {
        self.panel.title()
    }

    fn status(&self) -> Option<PanelStatus> {
        self.panel.status()
    }

    fn chrome(&self) -> WidgetChrome {
        self.panel.chrome()
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: TuiRect, focused: bool) {
        self.panel.render(frame, area, focused);
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        self.panel.handle_input(event)
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: TuiRect) -> WidgetAction {
        self.panel.handle_mouse(event, area)
    }

    fn cursor(&self, area: TuiRect) -> Option<(u16, u16)> {
        self.panel.cursor(area)
    }
}

#[cfg(test)]
mod tests {
    use super::{CallStackWidget, DebugConsoleWidget, VariablesWidget};
    use crate::workbench::ui::{PanelKind, Widget};

    #[test]
    fn widgets_keep_stable_kinds_titles_and_text() {
        let console = DebugConsoleWidget::new();
        assert_eq!(console.kind(), PanelKind::Output);
        assert!(console.preferred_rows() >= 3);

        let mut variables = VariablesWidget::new();
        assert_eq!(variables.kind(), PanelKind::Messages);
        assert_eq!(variables.title(), "Variables");
        // 替换正文后可继续绘制，且状态保持稳定。
        variables.set_variables("x = 1\ny = 2");
        assert_eq!(variables.title(), "Variables");

        let mut call_stack = CallStackWidget::new();
        assert_eq!(call_stack.title(), "CallStack");
        call_stack.set_frames("main.tl:12");
        assert_eq!(call_stack.kind(), PanelKind::Messages);
    }
}
