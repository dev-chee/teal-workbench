//! Agent 输入框与工作台控制台。
//!
//! 输入框拥有自己的文本、光标与提交语义：空输入不提交，行首 `:` 交给工作台命令行，
//! 行首 `!` 执行宿主命令，其余文本按模式提交为 Agent 提示或工作台命令。
//! 高度只提供首选值，实际槽位由 Panel 约束决定。

use ratatui::{
    Frame,
    layout::Rect as TuiRect,
    style::{Color, Style},
    text::Line,
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
};
use teal_workbench_core::{AppCommand, InputEvent, Key};

use super::widget::Widget;
use super::panel::{PanelKind, PanelStatus, WidgetAction};

/// 输入框的提交语义。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerMode {
    /// 普通文本提交为 Agent 提示；`:` 与 `!` 行首前缀保留命令行语义。
    Agent,

    /// 普通文本按工作台命令执行；`!` 行首前缀执行宿主命令。
    Command,
}

/// 多行输入框：文本、光标、chips 与按键提示。
pub struct ComposerWidget {
    mode: ComposerMode,
    placeholder: String,
    chips: Vec<String>,
    hint: String,
    text: String,
    cursor: usize,
}

impl ComposerWidget {
    /// 创建指定模式的输入框。
    #[must_use]
    pub fn new(mode: ComposerMode, placeholder: impl Into<String>) -> Self {
        let hint = match mode {
            ComposerMode::Agent => "Enter send | Shift-Enter newline | : command | ! host command",
            ComposerMode::Command => "Enter run | Shift-Enter newline | ! host command",
        };
        Self {
            mode,
            placeholder: placeholder.into(),
            chips: Vec::new(),
            hint: hint.to_owned(),
            text: String::new(),
            cursor: 0,
        }
    }

    /// 返回提交语义。
    #[must_use]
    pub const fn mode(&self) -> ComposerMode {
        self.mode
    }

    /// 返回当前文本。
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 替换文本并把光标移到末尾。
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.chars().count();
    }

    /// 返回用于边框标题的上下文 chips。
    #[must_use]
    pub fn chips(&self) -> &[String] {
        &self.chips
    }

    /// 替换边框标题上的上下文 chips。
    pub fn set_chips(&mut self, chips: Vec<String>) {
        self.chips = chips;
    }

    /// 返回包含边框的首选终端行数。
    #[must_use]
    pub fn preferred_rows(&self) -> u16 {
        let lines = u16::try_from(self.text.split('\n').count()).unwrap_or(u16::MAX);
        lines.saturating_add(2).clamp(3, 10)
    }

    /// 返回输入框是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// 返回光标所在的行与列（以字符计）。
    fn cursor_position(&self) -> (u16, u16) {
        let mut line = 0_u16;
        let mut column = 0_u16;
        for (index, character) in self.text.chars().enumerate() {
            if index >= self.cursor {
                break;
            }
            if character == '\n' {
                line = line.saturating_add(1);
                column = 0;
            } else {
                column = column.saturating_add(1);
            }
        }
        (line, column)
    }

    /// 按字节偏移量插入字符。
    fn insert(&mut self, character: char) {
        let index = char_index_to_byte(&self.text, self.cursor);
        self.text.insert(index, character);
        self.cursor += 1;
    }

    /// 删除光标前一个字符。
    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = char_index_to_byte(&self.text, self.cursor);
        let start = char_index_to_byte(&self.text, self.cursor - 1);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    /// 删除光标处字符。
    fn delete(&mut self) {
        if self.cursor >= self.text.chars().count() {
            return;
        }
        let start = char_index_to_byte(&self.text, self.cursor);
        let end = char_index_to_byte(&self.text, self.cursor + 1);
        self.text.replace_range(start..end, "");
    }

    /// 返回提交产生的类型化命令；空输入返回 `None`。
    fn submission(&mut self) -> Option<WidgetAction> {
        let text = self.text.trim().to_owned();
        if text.is_empty() {
            return None;
        }
        let command = if let Some(rest) = text.strip_prefix(':') {
            AppCommand::Command(rest.trim().to_owned())
        } else if let Some(rest) = text.strip_prefix('!') {
            AppCommand::Run(rest.trim().to_owned())
        } else {
            match self.mode {
                ComposerMode::Agent => AppCommand::Agent(text.clone()),
                ComposerMode::Command => AppCommand::Command(text.clone()),
            }
        };
        self.text.clear();
        self.cursor = 0;
        Some(WidgetAction::Commands(vec![command]))
    }
}

fn char_index_to_byte(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(offset, _)| offset)
}

impl Widget for ComposerWidget {
    fn id(&self) -> Option<&'static str> {
        Some("composer")
    }

    fn kind(&self) -> PanelKind {
        match self.mode {
            ComposerMode::Agent => PanelKind::Agent,
            ComposerMode::Command => PanelKind::Output,
        }
    }

    fn title(&self) -> String {
        self.chips.join(" \u{b7} ")
    }

    fn status(&self) -> Option<PanelStatus> {
        None
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        match event.key {
            Key::Enter if event.modifiers.shift || event.modifiers.alt => {
                self.insert('\n');
                WidgetAction::Consumed
            }
            Key::Enter => self.submission().unwrap_or(WidgetAction::Consumed),
            Key::Escape => {
                self.text.clear();
                self.cursor = 0;
                WidgetAction::Consumed
            }
            Key::Char(character) if !event.modifiers.control => {
                self.insert(character);
                WidgetAction::Consumed
            }
            Key::Backspace => {
                self.backspace();
                WidgetAction::Consumed
            }
            Key::Delete => {
                self.delete();
                WidgetAction::Consumed
            }
            Key::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                WidgetAction::Consumed
            }
            Key::Right => {
                if self.cursor < self.text.chars().count() {
                    self.cursor += 1;
                }
                WidgetAction::Consumed
            }
            Key::Home => {
                self.cursor = 0;
                WidgetAction::Consumed
            }
            Key::End => {
                self.cursor = self.text.chars().count();
                WidgetAction::Consumed
            }
            _ => WidgetAction::Ignored,
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: TuiRect, focused: bool) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(Line::from(self.title()))
            .title_bottom(Line::from(self.hint.clone()));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let body = if self.text.is_empty() {
            Line::from(self.placeholder.clone()).style(Style::default().fg(Color::DarkGray))
        } else {
            Line::from(self.text.clone())
        };
        frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), inner);
        if focused {
            let (line, column) = self.cursor_position();
            if line < inner.height {
                frame.set_cursor_position((
                    inner
                        .x
                        .saturating_add(column)
                        .min(inner.right().saturating_sub(1)),
                    inner.y.saturating_add(line),
                ));
            }
        }
    }

    fn cursor(&self, area: TuiRect) -> Option<(u16, u16)> {
        let block = Block::default().borders(Borders::ALL);
        let inner = block.inner(area);
        let (line, column) = self.cursor_position();
        if line >= inner.height || inner.width == 0 {
            return None;
        }
        Some((
            inner.x.saturating_sub(area.x).saturating_add(column),
            inner.y.saturating_sub(area.y).saturating_add(line),
        ))
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};
    use teal_workbench_core::{InputEvent, Key, KeyModifiers};

    use super::{ComposerMode, ComposerWidget};
    use crate::workbench::ui::{Widget, WidgetAction};

    fn event(key: Key) -> InputEvent {
        InputEvent {
            key,
            modifiers: KeyModifiers::default(),
        }
    }

    fn typed(composer: &mut ComposerWidget, text: &str) {
        for character in text.chars() {
            composer.handle_input(event(Key::Char(character)));
        }
    }

    #[test]
    fn agent_input_submits_text_and_prefixes_keep_command_semantics() {
        let mut composer = ComposerWidget::new(ComposerMode::Agent, "Describe the goal");
        typed(&mut composer, "add tests");
        let action = composer.handle_input(event(Key::Enter));
        assert!(matches!(
            action,
            WidgetAction::Commands(ref commands)
                if matches!(commands.as_slice(), [teal_workbench_core::AppCommand::Agent(text)] if text == "add tests")
        ));
        assert!(composer.is_empty(), "submitting clears the input");

        typed(&mut composer, ":window next");
        let action = composer.handle_input(event(Key::Enter));
        assert!(matches!(
            action,
            WidgetAction::Commands(ref commands)
                if matches!(commands.as_slice(), [teal_workbench_core::AppCommand::Command(text)] if text == "window next")
        ));

        typed(&mut composer, "!cargo test");
        let action = composer.handle_input(event(Key::Enter));
        assert!(matches!(
            action,
            WidgetAction::Commands(ref commands)
                if matches!(commands.as_slice(), [teal_workbench_core::AppCommand::Run(text)] if text == "cargo test")
        ));
    }

    #[test]
    fn console_input_runs_commands_and_empty_input_never_submits() {
        let mut composer = ComposerWidget::new(ComposerMode::Command, "Command");
        assert_eq!(
            composer.handle_input(event(Key::Enter)),
            WidgetAction::Consumed,
            "empty input must not submit"
        );
        typed(&mut composer, "  ");
        assert_eq!(
            composer.handle_input(event(Key::Enter)),
            WidgetAction::Consumed
        );

        typed(&mut composer, "debug-next");
        let action = composer.handle_input(event(Key::Enter));
        assert!(matches!(
            action,
            WidgetAction::Commands(ref commands)
                if matches!(commands.as_slice(), [teal_workbench_core::AppCommand::Command(text)] if text == "debug-next")
        ));
    }

    #[test]
    fn editing_grows_the_preferred_height_within_bounds() {
        let mut composer = ComposerWidget::new(ComposerMode::Agent, "Describe the goal");
        assert_eq!(composer.preferred_rows(), 3);
        typed(&mut composer, "one");
        assert_eq!(composer.preferred_rows(), 3);
        composer.handle_input(InputEvent {
            key: Key::Enter,
            modifiers: KeyModifiers {
                shift: true,
                ..KeyModifiers::default()
            },
        });
        typed(&mut composer, "two");
        assert_eq!(composer.preferred_rows(), 4);
        composer.set_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
        assert_eq!(composer.preferred_rows(), 10);
        composer.handle_input(event(Key::Escape));
        assert!(composer.is_empty());
        assert_eq!(composer.preferred_rows(), 3);
    }

    #[test]
    fn composer_renders_chips_and_hint() {
        let mut composer = ComposerWidget::new(ComposerMode::Agent, "Describe the goal");
        composer.set_chips(vec!["agent coding".to_owned(), "main".to_owned()]);
        let backend = TestBackend::new(60, 6);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| composer.render(frame, frame.area(), true))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut rendered = String::new();
        for y in 0..6 {
            for x in 0..60 {
                rendered.push_str(buffer[(x, y)].symbol());
            }
        }
        assert!(rendered.contains("agent coding"));
        assert!(rendered.contains("Describe the goal"));
        assert!(rendered.contains("Enter send"));
        assert_eq!(composer.title(), "agent coding \u{b7} main");
    }
}
