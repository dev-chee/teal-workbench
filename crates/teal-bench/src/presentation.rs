//! 组合可复用界面与服务模型的应用面板。

use crate::workbench::ui::{
    ClipboardAction, ClipboardOutcome, ContextMenu, MenuItem, PanelKind, PanelStatus, TextPoint,
    TextSelection, Widget, WidgetAction, cell_to_char, render_lines_with_selection, selected_text,
    word_range,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction as LayoutDirection, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use teal_host::web::WebDocument;
use teal_workbench_core::WorkspaceIntent;
use teal_workbench_core::{AppCommand, InputEvent, Key};
use teal_workbench_core::{MouseButton, MouseEvent, MouseEventKind};
use teal_workbench_services::{
    ExplorerSession, FileExplorer, MarkdownDocument, MarkdownStyle, Preview, SearchMatch,
};
mod app_panel;
mod browser;
mod custom;
mod explorer;
mod text;
pub use app_panel::AppPanel;
pub(crate) use app_panel::{session_test_row, test_row_from_session};
pub use browser::BrowserPanel;
pub(crate) use custom::CustomPanel;
pub use explorer::ExplorerPanel;
pub use text::{AgentBlock, TextPanel};

#[derive(Default)]
struct TextViewState {
    vertical: usize,
    horizontal: usize,
    selection: Option<TextSelection>,
    drag_anchor: Option<TextPoint>,
}

impl TextViewState {
    fn point_at(
        &mut self,
        lines: &[String],
        event: MouseEvent,
        area: Rect,
        auto_scroll: bool,
    ) -> TextPoint {
        let inner = Block::default().borders(Borders::ALL).inner(area);
        if auto_scroll {
            if event.row <= inner.y {
                self.vertical = self.vertical.saturating_sub(1);
            } else if event.row >= inner.bottom().saturating_sub(1) {
                self.vertical = (self.vertical + 1).min(lines.len().saturating_sub(1));
            }
            if event.column <= inner.x {
                self.horizontal = self.horizontal.saturating_sub(1);
            } else if event.column >= inner.right().saturating_sub(1) {
                self.horizontal = self.horizontal.saturating_add(1);
            }
        }
        let line = self
            .vertical
            .saturating_add(usize::from(event.row.saturating_sub(inner.y)))
            .min(lines.len().saturating_sub(1));
        let cell = self
            .horizontal
            .saturating_add(usize::from(event.column.saturating_sub(inner.x)));
        let column = lines.get(line).map_or(0, |text| cell_to_char(text, cell));
        TextPoint { line, column }
    }

    fn handle_mouse(&mut self, lines: &[String], event: MouseEvent, area: Rect) -> WidgetAction {
        match event.kind {
            MouseEventKind::ScrollUp if event.modifiers.shift => {
                self.horizontal = self.horizontal.saturating_sub(4);
                WidgetAction::Consumed
            }
            MouseEventKind::ScrollDown if event.modifiers.shift => {
                self.horizontal = self.horizontal.saturating_add(4);
                WidgetAction::Consumed
            }
            MouseEventKind::ScrollUp => {
                self.vertical = self.vertical.saturating_sub(3);
                WidgetAction::Consumed
            }
            MouseEventKind::ScrollDown => {
                self.vertical = (self.vertical + 3).min(lines.len().saturating_sub(1));
                WidgetAction::Consumed
            }
            MouseEventKind::ScrollLeft => {
                self.horizontal = self.horizontal.saturating_sub(4);
                WidgetAction::Consumed
            }
            MouseEventKind::ScrollRight => {
                self.horizontal = self.horizontal.saturating_add(4);
                WidgetAction::Consumed
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let point = self.point_at(lines, event, area, false);
                let selection = if event.click_count >= 3 {
                    let end = lines.get(point.line).map_or(0, |line| line.chars().count());
                    TextSelection {
                        anchor: TextPoint {
                            line: point.line,
                            column: 0,
                        },
                        head: TextPoint {
                            line: point.line,
                            column: end,
                        },
                    }
                } else if event.click_count == 2 {
                    let range = lines
                        .get(point.line)
                        .map_or(0..0, |line| word_range(line, point.column));
                    TextSelection {
                        anchor: TextPoint {
                            line: point.line,
                            column: range.start,
                        },
                        head: TextPoint {
                            line: point.line,
                            column: range.end,
                        },
                    }
                } else if event.modifiers.shift {
                    TextSelection {
                        anchor: self.selection.map_or(point, |selection| selection.anchor),
                        head: point,
                    }
                } else {
                    TextSelection {
                        anchor: point,
                        head: point,
                    }
                };
                self.drag_anchor = Some(selection.anchor);
                self.selection = Some(selection);
                WidgetAction::Consumed
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let point = self.point_at(lines, event, area, true);
                if let Some(anchor) = self.drag_anchor {
                    self.selection = Some(TextSelection {
                        anchor,
                        head: point,
                    });
                }
                WidgetAction::Consumed
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag_anchor = None;
                WidgetAction::Consumed
            }
            MouseEventKind::Down(MouseButton::Right) => {
                let point = self.point_at(lines, event, area, false);
                if self.selection.is_none_or(|selection| {
                    let (start, end) = selection.ordered();
                    point < start || point > end
                }) {
                    self.selection = Some(TextSelection {
                        anchor: point,
                        head: point,
                    });
                }
                let enabled = self
                    .selection
                    .and_then(|selection| selected_text(lines, selection))
                    .is_some();
                WidgetAction::Menu(ContextMenu {
                    x: event.column,
                    y: event.row,
                    items: vec![
                        if enabled {
                            MenuItem::enabled("clipboard.copy", "copy")
                        } else {
                            MenuItem::disabled("clipboard.copy", "copy")
                        },
                        MenuItem::enabled("clipboard.select_all", "Select all"),
                    ],
                })
            }
            _ => WidgetAction::Ignored,
        }
    }

    fn handle_clipboard(&mut self, lines: &[String], action: ClipboardAction) -> ClipboardOutcome {
        match action {
            ClipboardAction::Copy => ClipboardOutcome {
                consumed: true,
                write: self
                    .selection
                    .and_then(|selection| selected_text(lines, selection)),
            },
            ClipboardAction::SelectAll => {
                let last = lines.len().saturating_sub(1);
                self.selection = Some(TextSelection {
                    anchor: TextPoint { line: 0, column: 0 },
                    head: TextPoint {
                        line: last,
                        column: lines.get(last).map_or(0, |line| line.chars().count()),
                    },
                });
                ClipboardOutcome {
                    consumed: true,
                    write: None,
                }
            }
            ClipboardAction::Cut | ClipboardAction::Paste => ClipboardOutcome::default(),
        }
    }
}

fn plain_lines(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn rect_contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x && x < area.right() && y >= area.y && y < area.bottom()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::workbench::ui::{ClipboardAction, Widget, WidgetAction};
    use ratatui::{Frame, Terminal, backend::TestBackend, layout::Rect};
    use teal_workbench_core::{
        AppCommand, InputEvent, Key, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use teal_workbench_services::FileSort;
    use tempfile::tempdir;
    use url::Url;

    use super::{BrowserPanel, ExplorerPanel, TextPanel};

    fn mouse(kind: MouseEventKind, column: u16, row: u16, clicks: u8) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::default(),
            click_count: clicks,
        }
    }

    fn key(key: Key) -> InputEvent {
        InputEvent {
            key,
            modifiers: KeyModifiers::default(),
        }
    }

    fn render_panel(panel: &mut impl Widget, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame: &mut Frame<'_>| panel.render(frame, frame.area(), true))
            .expect("panel render");
        let buffer = terminal.backend().buffer();
        let mut rendered = String::new();
        for y in 0..height {
            let mut x = 0;
            while x < width {
                let symbol = buffer[(x, y)].symbol();
                rendered.push_str(symbol);
                x += u16::try_from(ratatui::text::Span::raw(symbol).width().max(1)).unwrap();
            }
            rendered.push('\n');
        }
        rendered
    }

    #[test]
    fn explorer_mouse_selects_ranges_and_double_clicks_files() {
        let directory = tempdir().expect("temporary directory");
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(directory.path().join(name), name).expect("test file");
        }
        let mut panel = ExplorerPanel::new(directory.path(), false).expect("explorer");
        render_panel(&mut panel, 90, 20);
        let area = panel.tree_area;
        panel.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                area.x,
                area.y + 1,
                1,
            ),
            Rect::new(0, 0, 90, 20),
        );
        let mut shift = mouse(
            MouseEventKind::Down(MouseButton::Left),
            area.x,
            area.y + 3,
            1,
        );
        shift.modifiers.shift = true;
        panel.handle_mouse(shift, Rect::new(0, 0, 90, 20));
        assert_eq!(panel.selected_paths().len(), 3);
        assert!(matches!(
            panel.handle_mouse(
                mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    area.x,
                    area.y + 1,
                    2,
                ),
                Rect::new(0, 0, 90, 20)
            ),
            WidgetAction::Commands(commands) if matches!(commands.as_slice(), [AppCommand::Open(_)])
        ));
    }

    #[test]
    fn explorer_renders_tree_and_disclosure_click_toggles_directory() {
        let directory = tempdir().expect("temporary directory");
        let folder = directory.path().join("folder");
        fs::create_dir(&folder).expect("directory test data");
        fs::write(folder.join("nested.txt"), "nested").expect("nested test data");
        let mut panel = ExplorerPanel::new(directory.path(), false).expect("explorer");
        let rendered = render_panel(&mut panel, 100, 20);
        assert!(panel.tree_area.width > 50);
        assert!(rendered.contains('▾'));
        assert!(rendered.contains("▸ folder/"));
        assert!(rendered.contains("Preview"));

        let folder_row = panel.tree_area.y + 1;
        let disclosure = panel.tree_area.x + 4;
        assert!(matches!(
            panel.handle_mouse(
                mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    disclosure,
                    folder_row,
                    1,
                ),
                Rect::new(0, 0, 100, 20),
            ),
            WidgetAction::Consumed
        ));
        panel.settle();
        assert!(
            panel
                .explorer
                .expanded_paths()
                .iter()
                .any(|path| path.ends_with("folder"))
        );
        assert!(
            panel
                .explorer
                .visible_entries()
                .iter()
                .any(|row| { row.depth == 2 && row.entry.path.ends_with("nested.txt") })
        );
    }

    #[test]
    fn explorer_keyboard_uses_ide_tree_navigation_and_restores_view_state() {
        let directory = tempdir().expect("temporary directory");
        let folder = directory.path().join("folder");
        fs::create_dir(&folder).expect("directory test data");
        for index in 0..12 {
            fs::write(folder.join(format!("file-{index:02}.txt")), "content")
                .expect("file test data");
        }
        let mut panel = ExplorerPanel::new(directory.path(), false).expect("explorer");
        panel.handle_input(key(Key::Char('j')));
        panel.handle_input(key(Key::Char('l')));
        panel.settle();
        assert!(panel.explorer.hovered().expect("folder").expanded);
        panel.handle_input(key(Key::Char('l')));
        assert_eq!(panel.explorer.hovered().expect("first child node").depth, 2);
        panel.explorer.set_sort(FileSort::Modified);
        panel.scroll = 5;

        let state = panel.session_state();
        let mut restored = ExplorerPanel::restore(&state, false).expect("restore file explorer");
        render_panel(&mut restored, 60, 8);
        assert_eq!(restored.session_state(), state);
    }

    #[test]
    fn text_panel_drag_selection_copies_rendered_plain_text() {
        let mut panel = TextPanel::plain(
            crate::workbench::ui::PanelKind::Output,
            "Output",
            "abc\ndef",
        );
        let area = Rect::new(0, 0, 30, 8);
        panel.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 2, 1, 1),
            area,
        );
        panel.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 3, 2, 0),
            area,
        );
        assert_eq!(
            panel
                .handle_clipboard(ClipboardAction::Copy, None)
                .write
                .as_deref(),
            Some("bc\nde")
        );
    }

    #[test]
    fn browser_link_rows_click_and_offer_link_actions() {
        let document = teal_host::web::WebDocument::from_html(
            Url::parse("https://example.com").expect("URL"),
            200,
            "<p>Hello <a href='/next'>Next</a></p>",
        );
        let mut panel = BrowserPanel::new(document);
        let area = Rect::new(0, 0, 80, 12);
        let click = mouse(MouseEventKind::Down(MouseButton::Left), 2, 4, 1);
        assert!(matches!(
            panel.handle_mouse(click, area),
            WidgetAction::Commands(commands) if matches!(commands.as_slice(), [AppCommand::Browse(_)])
        ));
        let menu = panel.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Right), 2, 4, 1),
            area,
        );
        assert!(matches!(
            menu,
            WidgetAction::Menu(menu) if menu.items.iter().any(|item| item.id == "browser.copy_link.0")
        ));
    }
}
