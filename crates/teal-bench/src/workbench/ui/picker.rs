//! 选择器 Widget。
//!
//! 选择器是菜单原语的窗口化形态：它持有条目与过滤输入，负责列表呈现、模糊过滤与
//! 选择导航，并把接受动作表达为类型化应用命令；它不执行打开、切换或写回。

use ratatui::{
    Frame,
    layout::Rect as TuiRect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};
use teal_workbench_core::{AppCommand, InputEvent, MouseEvent};

use super::MenuItem;
use super::menu::{Menu, MenuOutcome};
use super::panel::{ClipboardAction, PanelKind, PanelStatus, WidgetAction};
use super::widget::{Widget, WidgetChrome};

/// 可选条目在列表中的最大可见行数。
const MAX_VISIBLE: usize = 12;

/// 一个选择器：标题、条目、过滤输入与选中位置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerWidget {
    title: String,
    menu: Menu,
    /// 接受条目时使用的命令名；缺省时按 `pick <id>` 交回命令行。
    command: Option<String>,
}

impl PickerWidget {
    /// 用标题与条目创建选择器。
    #[must_use]
    pub fn new(title: impl Into<String>, items: Vec<MenuItem>) -> Self {
        Self {
            title: title.into(),
            menu: Menu::new(items),
            command: None,
        }
    }

    /// 用标签与相同值创建选择器。
    #[must_use]
    pub fn with_labels(title: impl Into<String>, labels: Vec<String>) -> Self {
        let items = labels
            .into_iter()
            .map(|label| MenuItem::enabled(label.clone(), label))
            .collect();
        Self::new(title, items)
    }

    /// 指定接受条目时执行的命令名。
    #[must_use]
    pub fn with_command(mut self, command: impl Into<String>) -> Self {
        self.command = Some(command.into());
        self
    }

    /// 返回全部条目。
    #[must_use]
    pub fn items(&self) -> &[MenuItem] {
        self.menu.items()
    }

    /// 返回当前过滤输入。
    #[must_use]
    pub fn query(&self) -> &str {
        self.menu.query()
    }

    /// 返回过滤结果中的选中下标。
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.menu.selected()
    }

    /// 返回把一次接受转换为的应用命令。
    fn accept_command(&self, value: &str) -> AppCommand {
        match &self.command {
            Some(command) => AppCommand::Command(format!("{command} {value}")),
            None => AppCommand::Command(format!("pick {value}")),
        }
    }

    /// 返回可见列表项。
    fn visible_items(&self) -> Vec<ListItem<'static>> {
        let selected = self.menu.selected();
        self.menu
            .visible()
            .into_iter()
            .enumerate()
            .map(|(position, index)| {
                let item = &self.items()[index];
                let label = if position == selected {
                    format!("> {}", item.label)
                } else {
                    format!("  {}", item.label)
                };
                let style = if !item.enabled {
                    Style::default().fg(Color::DarkGray)
                } else if position == selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                ListItem::new(label).style(style)
            })
            .collect()
    }
}

impl Widget for PickerWidget {
    fn kind(&self) -> PanelKind {
        PanelKind::Messages
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn status(&self) -> Option<PanelStatus> {
        Some(PanelStatus {
            mode: "picker".to_owned(),
            resource: self.title.clone(),
            position: if self.query().is_empty() {
                String::new()
            } else {
                format!("filter {}", self.query())
            },
        })
    }

    fn chrome(&self) -> WidgetChrome {
        WidgetChrome::new(self.kind(), self.title())
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: TuiRect, focused: bool) {
        let block = Block::default()
            .title(self.title.as_str())
            .borders(Borders::ALL)
            .border_style(if focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            });
        frame.render_widget(Clear, area);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        // 最后一行显示过滤输入，其余用于列表。
        let list_area = TuiRect {
            height: inner.height.saturating_sub(1),
            ..inner
        };
        let query_area = TuiRect {
            y: inner.y.saturating_add(inner.height.saturating_sub(1)),
            height: 1,
            ..inner
        };
        let items = self.visible_items();
        frame.render_widget(
            List::new(items).scroll_padding(0).highlight_symbol(""),
            list_area,
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("/{}", self.query()),
                Style::default().fg(Color::DarkGray),
            ))),
            query_area,
        );
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        match self.menu.handle_input(event) {
            MenuOutcome::Accepted { value, .. } => {
                WidgetAction::Commands(vec![self.accept_command(&value)])
            }
            MenuOutcome::Cancelled => {
                WidgetAction::Commands(vec![AppCommand::Command("picker-cancel".to_owned())])
            }
            MenuOutcome::Consumed | MenuOutcome::Ignored => WidgetAction::Consumed,
        }
    }

    fn handle_mouse(&mut self, _event: MouseEvent, _area: TuiRect) -> WidgetAction {
        WidgetAction::Consumed
    }

    fn handle_clipboard(
        &mut self,
        _action: ClipboardAction,
        _paste: Option<&str>,
    ) -> super::ClipboardOutcome {
        super::ClipboardOutcome::default()
    }

    fn handle_menu(&mut self, _id: &str) -> WidgetAction {
        WidgetAction::Ignored
    }
}

#[cfg(test)]
mod tests {
    use teal_workbench_core::{AppCommand, InputEvent, Key, KeyModifiers};

    use super::PickerWidget;
    use crate::workbench::ui::{MenuItem, PanelKind, Widget, WidgetAction};

    fn key(key: Key) -> InputEvent {
        InputEvent {
            key,
            modifiers: KeyModifiers::default(),
        }
    }

    #[test]
    fn filters_then_accepts_the_selected_item_as_a_command() {
        let mut picker = PickerWidget::with_labels(
            "runtime tier",
            vec![
                "embedded".to_owned(),
                "agentic".to_owned(),
                "hosted".to_owned(),
            ],
        )
        .with_command("runtime-tier");
        assert_eq!(picker.kind(), PanelKind::Messages);
        assert_eq!(picker.items().len(), 3);

        // 过滤到 agent，再接受。
        for character in "ag".chars() {
            picker.handle_input(key(Key::Char(character)));
        }
        assert_eq!(picker.query(), "ag");
        assert_eq!(picker.selected(), 0);
        let action = picker.handle_input(key(Key::Enter));
        assert!(matches!(
            action,
            WidgetAction::Commands(commands)
                if commands == vec![AppCommand::Command("runtime-tier agentic".to_owned())]
        ));

        // 取消产生取消命令，而不是静默消费。
        let action = picker.handle_input(key(Key::Escape));
        assert!(matches!(
            action,
            WidgetAction::Commands(commands)
                if commands == vec![AppCommand::Command("picker-cancel".to_owned())]
        ));
    }

    #[test]
    fn disabled_items_cannot_be_accepted_without_moving_off_them() {
        let mut picker = PickerWidget::new(
            "buffer",
            vec![
                MenuItem::disabled("hidden", "Hidden"),
                MenuItem::enabled("visible", "Visible"),
            ],
        );
        // 禁用项是第一个条目：直接回车不产生命令。
        let action = picker.handle_input(key(Key::Enter));
        assert_eq!(action, WidgetAction::Consumed);
        // 下移到启用项后可以接受。
        picker.handle_input(key(Key::Down));
        let action = picker.handle_input(key(Key::Enter));
        assert!(matches!(
            action,
            WidgetAction::Commands(commands)
                if commands == vec![AppCommand::Command("pick visible".to_owned())]
        ));
    }
}
