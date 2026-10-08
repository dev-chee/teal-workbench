//! Agent 输入框的具名部件。
//!
//! 关卡的「会话 + 输入框」模板里的输入框就是 `AgentComposerWidget`：它复用通用
//! [`ComposerWidget`]，但固定 Agent 语义并给出稳定名称，便于模板、会话恢复与测试按
//! 语义引用，而不是按泛型部件推断。

use ratatui::{Frame, layout::Rect as TuiRect};
use teal_workbench_core::{InputEvent, MouseEvent};

use super::panel::{ClipboardAction, ClipboardOutcome, PanelKind, PanelStatus, WidgetAction};
use super::widget::{Widget, WidgetChrome};
use super::{ComposerMode, ComposerWidget};

/// Agent 自然语言输入框。
pub struct AgentComposerWidget {
    composer: ComposerWidget,
}

impl AgentComposerWidget {
    /// 创建带占位符的 Agent 输入框。
    #[must_use]
    pub fn new(placeholder: impl Into<String>) -> Self {
        Self {
            composer: ComposerWidget::new(ComposerMode::Agent, placeholder),
        }
    }

    /// 返回底层输入部件。
    #[must_use]
    pub fn composer(&self) -> &ComposerWidget {
        &self.composer
    }

    /// 返回底层输入部件的可变引用。
    pub fn composer_mut(&mut self) -> &mut ComposerWidget {
        &mut self.composer
    }

    /// 设置边框上的上下文 chips。
    pub fn set_chips(&mut self, chips: Vec<String>) {
        self.composer.set_chips(chips);
    }

    /// 返回基于内容的首选行数。
    #[must_use]
    pub fn preferred_rows(&self) -> u16 {
        self.composer.preferred_rows()
    }
}

impl Default for AgentComposerWidget {
    fn default() -> Self {
        Self::new("Describe the goal")
    }
}

impl Widget for AgentComposerWidget {
    fn kind(&self) -> PanelKind {
        self.composer.kind()
    }

    fn title(&self) -> String {
        self.composer.title()
    }

    fn status(&self) -> Option<PanelStatus> {
        self.composer.status()
    }

    fn chrome(&self) -> WidgetChrome {
        self.composer.chrome()
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: TuiRect, focused: bool) {
        self.composer.render(frame, area, focused);
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        self.composer.handle_input(event)
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: TuiRect) -> WidgetAction {
        self.composer.handle_mouse(event, area)
    }

    fn handle_clipboard(
        &mut self,
        action: ClipboardAction,
        paste: Option<&str>,
    ) -> ClipboardOutcome {
        self.composer.handle_clipboard(action, paste)
    }

    fn handle_menu(&mut self, id: &str) -> WidgetAction {
        self.composer.handle_menu(id)
    }

    fn cursor(&self, area: TuiRect) -> Option<(u16, u16)> {
        self.composer.cursor(area)
    }

    fn focus_next(&mut self, backwards: bool) -> bool {
        self.composer.focus_next(backwards)
    }
}

#[cfg(test)]
mod tests {
    use super::AgentComposerWidget;
    use crate::workbench::ui::{ComposerMode, PanelKind, Widget};
    use teal_workbench_core::{AppCommand, InputEvent, Key, KeyModifiers};

    fn key(key: Key) -> InputEvent {
        InputEvent {
            key,
            modifiers: KeyModifiers::default(),
        }
    }

    #[test]
    fn agent_composer_submits_agent_commands_with_chips() {
        let mut composer = AgentComposerWidget::new("Describe the goal");
        composer.set_chips(vec!["agent default".to_owned()]);
        assert_eq!(composer.kind(), PanelKind::Agent);
        assert_eq!(composer.composer().mode(), ComposerMode::Agent);
        assert!(composer.title().contains("agent default"));

        // Enter 提交为 Agent 命令，而不是普通命令行。
        for character in "hi".chars() {
            composer.handle_input(key(Key::Char(character)));
        }
        let action = composer.handle_input(key(Key::Enter));
        assert!(matches!(
            action,
            crate::workbench::ui::WidgetAction::Commands(commands)
                if commands == vec![AppCommand::Agent("hi".to_owned())]
        ));
    }
}
