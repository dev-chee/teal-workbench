//! 提示原语：命令行、搜索和面板提示共用的局部输入状态。
//!
//! [`Prompt`] 持有文本、光标、选择、视口与历史，文本编辑、选择和剪贴板都在此处理；
//! 接受输入后由调用方解释命令或搜索含义。`:`/`/`/`?`/`>` 四种模式只是同一原语的用途
//! 标签，行为不再按前端各写一份。

use ratatui::text::{Line, Span};
use teal_workbench_core::{InputEvent, Key, MouseButton, MouseEvent, MouseEventKind, Rect};

use super::{
    ClipboardAction, ClipboardOutcome, ContextMenu, MenuItem, TextPoint, TextSelection,
    cell_to_char, char_to_cell, render_lines_with_selection, word_range,
};

/// 全局命令行的用途与可见前缀。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmdlineMode {
    /// Vim 风格 Ex 命令。
    Command,

    /// 向前搜索缓冲区。
    SearchForward,

    /// 向后搜索缓冲区。
    SearchBackward,

    /// 面板专用提示。
    Prompt,
}

impl CmdlineMode {
    const fn prefix(self) -> char {
        match self {
            Self::Command => ':',
            Self::SearchForward => '/',
            Self::SearchBackward => '?',
            Self::Prompt => '>',
        }
    }
}

/// 编辑全局命令行的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CmdlineAction {
    /// 输入已更改但仍处于活动状态。
    Changed,

    /// 用户接受了当前行。
    Submit {
        /// 生成文本的提示模式。
        mode: CmdlineMode,

        /// 不含前缀的已接受文本。
        text: String,
    },

    /// 用户取消提示。
    Cancel,

    /// 按键与此操作无关。
    Ignored,

    /// 指针请求上下文菜单。
    Menu(ContextMenu),

    /// 指针请求系统剪贴板操作。
    Clipboard(ClipboardAction),
}

/// 带有界历史的可编辑命令/搜索提示。
#[derive(Clone, Debug)]
pub struct Prompt {
    mode: CmdlineMode,
    text: String,
    cursor: usize,
    selection_anchor: Option<usize>,
    dragging: bool,
    viewport: u16,
    last_width: u16,
    history: Vec<String>,
    history_index: Option<usize>,
}

impl Prompt {
    /// 启动空提示。
    #[must_use]
    pub fn new(mode: CmdlineMode) -> Self {
        Self {
            mode,
            text: String::new(),
            cursor: 0,
            selection_anchor: None,
            dragging: false,
            viewport: 0,
            last_width: 80,
            history: Vec::new(),
            history_index: None,
        }
    }

    /// 使用初始文本启动提示。
    #[must_use]
    pub fn with_text(mode: CmdlineMode, text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            cursor: text.chars().count(),
            text,
            ..Prompt::new(mode)
        }
    }

    /// 使用保留历史启动提示。
    #[must_use]
    pub fn with_history(mode: CmdlineMode, history: Vec<String>) -> Self {
        Self {
            history,
            ..Prompt::new(mode)
        }
    }

    /// 返回保留的已接受行。
    #[must_use]
    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// 返回包含模式前缀的行。
    #[must_use]
    pub fn display(&self) -> String {
        format!("{}{}", self.mode.prefix(), self.text)
    }

    /// 返回不含模式前缀的正文。
    #[must_use]
    pub fn body(&self) -> &str {
        &self.text
    }

    /// 返回当前提示模式。
    #[must_use]
    pub const fn mode(&self) -> CmdlineMode {
        self.mode
    }

    /// 用整段文本替换正文，并把光标放到给定的字符下标（含前缀偏移）。
    pub fn set_body(&mut self, text: impl Into<String>, cursor: usize) {
        self.text = text.into();
        self.cursor = cursor.min(self.text.chars().count());
        self.selection_anchor = None;
        self.viewport = 0;
    }

    /// 返回包含模式前缀和选择高亮的带样式行。
    #[must_use]
    pub fn render_line(&self) -> Line<'static> {
        let line = Line::from(vec![
            Span::raw(self.mode.prefix().to_string()),
            Span::raw(self.text.clone()),
        ]);
        let selection = self.selection_range().map(|range| TextSelection {
            anchor: TextPoint {
                line: 0,
                column: range.start + 1,
            },
            head: TextPoint {
                line: 0,
                column: range.end + 1,
            },
        });
        render_lines_with_selection(&[line], selection)
            .into_iter()
            .next()
            .unwrap_or_default()
    }

    /// 返回帧渲染使用的水平滚动偏移。
    #[must_use]
    pub const fn viewport(&self) -> u16 {
        self.viewport
    }

    /// 返回命令行分配中的可见光标单元格。
    #[must_use]
    pub fn cursor_cell(&self, width: u16) -> u16 {
        let absolute = 1_usize.saturating_add(char_to_cell(&self.text, self.cursor));
        u16::try_from(absolute)
            .unwrap_or(u16::MAX)
            .saturating_sub(self.viewport)
            .min(width.saturating_sub(1))
    }

    /// 返回包含前缀的字符光标。
    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor + 1
    }

    /// 路由一个按键事件。
    pub fn handle(&mut self, event: InputEvent) -> CmdlineAction {
        let action = match event.key {
            Key::Escape => CmdlineAction::Cancel,
            Key::Enter => {
                let text = self.text.clone();
                if !text.is_empty() && self.history.last() != Some(&text) {
                    self.history.push(text.clone());
                }
                CmdlineAction::Submit {
                    mode: self.mode,
                    text,
                }
            }
            Key::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                self.selection_anchor = None;
                CmdlineAction::Changed
            }
            Key::Right => {
                self.cursor = (self.cursor + 1).min(self.text.chars().count());
                self.selection_anchor = None;
                CmdlineAction::Changed
            }
            Key::Home => {
                self.cursor = 0;
                self.selection_anchor = None;
                CmdlineAction::Changed
            }
            Key::End => {
                self.cursor = self.text.chars().count();
                self.selection_anchor = None;
                CmdlineAction::Changed
            }
            Key::Backspace => {
                if !self.delete_selection() && self.cursor > 0 {
                    let start = char_to_byte(&self.text, self.cursor - 1);
                    let end = char_to_byte(&self.text, self.cursor);
                    self.text.replace_range(start..end, "");
                    self.cursor -= 1;
                }
                CmdlineAction::Changed
            }
            Key::Delete => {
                if !self.delete_selection() && self.cursor < self.text.chars().count() {
                    let start = char_to_byte(&self.text, self.cursor);
                    let end = char_to_byte(&self.text, self.cursor + 1);
                    self.text.replace_range(start..end, "");
                }
                CmdlineAction::Changed
            }
            Key::Up => {
                if self.history.is_empty() {
                    return CmdlineAction::Changed;
                }
                let index = self
                    .history_index
                    .map_or(self.history.len() - 1, |index| index.saturating_sub(1));
                self.history_index = Some(index);
                self.text.clone_from(&self.history[index]);
                self.cursor = self.text.chars().count();
                self.selection_anchor = None;
                CmdlineAction::Changed
            }
            Key::Down => {
                if let Some(index) = self.history_index {
                    if index + 1 < self.history.len() {
                        self.history_index = Some(index + 1);
                        self.text.clone_from(&self.history[index + 1]);
                    } else {
                        self.history_index = None;
                        self.text.clear();
                    }
                    self.cursor = self.text.chars().count();
                    self.selection_anchor = None;
                }
                CmdlineAction::Changed
            }
            Key::Char(character) if !event.modifiers.control && !event.modifiers.alt => {
                let _ = self.delete_selection();
                let byte = char_to_byte(&self.text, self.cursor);
                self.text.insert(byte, character);
                self.cursor += 1;
                CmdlineAction::Changed
            }
            _ => CmdlineAction::Ignored,
        };
        self.ensure_cursor_visible();
        action
    }

    /// 在全局命令行行内路由鼠标事件。
    pub fn handle_mouse(&mut self, event: MouseEvent, area: Rect) -> CmdlineAction {
        if !area.contains(event.column, event.row) {
            return CmdlineAction::Ignored;
        }
        self.last_width = area.width.max(1);
        let target = self.character_at(event.column.saturating_sub(area.x));
        let action = match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if event.click_count >= 3 {
                    self.selection_anchor = Some(0);
                    self.cursor = self.text.chars().count();
                } else if event.click_count == 2 {
                    let range = word_range(&self.text, target);
                    self.selection_anchor = Some(range.start);
                    self.cursor = range.end;
                } else if event.modifiers.shift {
                    self.selection_anchor.get_or_insert(self.cursor);
                    self.cursor = target;
                } else {
                    self.selection_anchor = Some(target);
                    self.cursor = target;
                }
                self.dragging = true;
                CmdlineAction::Changed
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                self.cursor = target;
                CmdlineAction::Changed
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.dragging = false;
                if self.selection_anchor == Some(self.cursor) {
                    self.selection_anchor = None;
                }
                CmdlineAction::Changed
            }
            MouseEventKind::Down(MouseButton::Middle) => {
                self.cursor = target;
                self.selection_anchor = None;
                CmdlineAction::Clipboard(ClipboardAction::Paste)
            }
            MouseEventKind::Down(MouseButton::Right) => {
                let has_selection = self.selection_range().is_some();
                CmdlineAction::Menu(ContextMenu {
                    x: event.column,
                    y: event.row,
                    items: vec![
                        if has_selection {
                            MenuItem::enabled("clipboard.cut", "cut")
                        } else {
                            MenuItem::disabled("clipboard.cut", "cut")
                        },
                        if has_selection {
                            MenuItem::enabled("clipboard.copy", "copy")
                        } else {
                            MenuItem::disabled("clipboard.copy", "copy")
                        },
                        MenuItem::enabled("clipboard.paste", "Paste"),
                        MenuItem::enabled("clipboard.select_all", "Select all"),
                    ],
                })
            }
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollUp if event.modifiers.shift => {
                self.viewport = self.viewport.saturating_sub(4);
                CmdlineAction::Changed
            }
            MouseEventKind::ScrollRight | MouseEventKind::ScrollDown if event.modifiers.shift => {
                self.viewport = self.viewport.saturating_add(4);
                CmdlineAction::Changed
            }
            _ => CmdlineAction::Ignored,
        };
        self.ensure_cursor_visible();
        action
    }

    /// 应用读取粘贴负载后执行剪贴板操作。
    pub fn handle_clipboard(
        &mut self,
        action: ClipboardAction,
        paste: Option<&str>,
    ) -> ClipboardOutcome {
        let mut outcome = ClipboardOutcome {
            consumed: true,
            write: None,
        };
        match action {
            ClipboardAction::Copy => outcome.write = self.selected_text(),
            ClipboardAction::Cut => {
                outcome.write = self.selected_text();
                if outcome.write.is_some() {
                    let _ = self.delete_selection();
                }
            }
            ClipboardAction::Paste => {
                let Some(paste) = paste else {
                    return ClipboardOutcome::default();
                };
                let paste = paste
                    .replace("\r\n", "\n")
                    .replace('\r', "\n")
                    .replace('\n', " ");
                let _ = self.delete_selection();
                let byte = char_to_byte(&self.text, self.cursor);
                self.text.insert_str(byte, &paste);
                self.cursor += paste.chars().count();
            }
            ClipboardAction::SelectAll => {
                self.selection_anchor = Some(0);
                self.cursor = self.text.chars().count();
            }
        }
        self.ensure_cursor_visible();
        outcome
    }

    fn selection_range(&self) -> Option<std::ops::Range<usize>> {
        let anchor = self.selection_anchor?;
        (anchor != self.cursor).then_some(anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    fn selected_text(&self) -> Option<String> {
        let range = self.selection_range()?;
        Some(
            self.text
                .chars()
                .skip(range.start)
                .take(range.end - range.start)
                .collect(),
        )
    }

    fn delete_selection(&mut self) -> bool {
        let Some(range) = self.selection_range() else {
            return false;
        };
        let start = char_to_byte(&self.text, range.start);
        let end = char_to_byte(&self.text, range.end);
        self.text.replace_range(start..end, "");
        self.cursor = range.start;
        self.selection_anchor = None;
        true
    }

    fn character_at(&self, visible_cell: u16) -> usize {
        let absolute = usize::from(self.viewport.saturating_add(visible_cell));
        cell_to_char(&self.text, absolute.saturating_sub(1))
    }

    fn ensure_cursor_visible(&mut self) {
        let cursor = u16::try_from(1_usize.saturating_add(char_to_cell(&self.text, self.cursor)))
            .unwrap_or(u16::MAX);
        if cursor < self.viewport {
            self.viewport = cursor;
        } else if cursor >= self.viewport.saturating_add(self.last_width) {
            self.viewport = cursor
                .saturating_add(1)
                .saturating_sub(self.last_width.max(1));
        }
    }
}

fn char_to_byte(text: &str, character: usize) -> usize {
    text.char_indices()
        .nth(character)
        .map_or(text.len(), |(byte, _)| byte)
}

#[cfg(test)]
mod tests {
    use teal_workbench_core::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind, Rect};

    use super::{CmdlineAction, CmdlineMode, Prompt};
    use super::super::ClipboardAction;

    fn mouse(kind: MouseEventKind, column: u16, clicks: u8) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row: 0,
            modifiers: KeyModifiers::default(),
            click_count: clicks,
        }
    }

    #[test]
    fn mouse_selects_copies_and_pastes_without_keyboard_shortcuts() {
        let mut prompt = Prompt::with_text(CmdlineMode::Command, "open \u{4e16}\u{754c}");
        let area = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 1,
        };
        assert!(matches!(
            prompt.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, 2), area),
            CmdlineAction::Changed
        ));
        assert_eq!(
            prompt
                .handle_clipboard(ClipboardAction::Copy, None)
                .write
                .as_deref(),
            Some("open")
        );
        let _ = prompt.handle_clipboard(ClipboardAction::Paste, Some("edit\r\nfile"));
        assert_eq!(prompt.display(), ":edit file \u{4e16}\u{754c}");
    }
}
