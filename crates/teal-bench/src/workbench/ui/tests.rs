//! Tests 窗口的测试列表组件。
//!
//! 组件只保存一次运行的显示投影：分组、选中行与筛选说明。跳转与单测重跑通过类型化应用
//! 命令交给宿主执行，组件自身不编译、不执行测试，也不读写工作区。

use std::path::PathBuf;

use ratatui::{
    Frame,
    layout::Rect as TuiRect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use teal_workbench_core::{AppCommand, InputEvent, Key};

use super::panel::{PanelKind, PanelStatus, WidgetAction};
use super::widget::Widget;

/// 单个测试的稳定显示状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestRowStatus {
    /// 测试返回 `0`。
    Passed,
    /// 测试抛出未捕获错误或执行失败。
    Failed,
    /// 测试超出指令预算或墙钟上限。
    TimedOut,
    /// 测试返回非零退出码。
    ExitCode(i32),
}

impl TestRowStatus {
    /// 返回列表使用的短标记。
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Passed => "ok".to_owned(),
            Self::Failed => "FAIL".to_owned(),
            Self::TimedOut => "TIMEOUT".to_owned(),
            Self::ExitCode(code) => format!("EXIT {code}"),
        }
    }

    /// 返回该状态是否表示测试没有通过。
    #[must_use]
    pub const fn is_failure(self) -> bool {
        !matches!(self, Self::Passed)
    }
}

/// 一次运行中的单个测试结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestRow {
    /// 测试名称。
    pub name: String,
    /// 声明所在源码文件的显示名称；分组按它进行。
    pub source: String,
    /// 声明所在文件的规范路径；缺失时无法跳转。
    pub path: Option<PathBuf>,
    /// 声明行号，从 1 开始。
    pub line: u32,
    /// 执行状态。
    pub status: TestRowStatus,
    /// 失败说明；通过时为空。
    pub message: String,
    /// 测试实例的控制台输出。
    pub output: String,
}

/// 分组显示的测试列表。
pub struct TestsWidget {
    rows: Vec<TestRow>,
    selected: usize,
    scroll: usize,
    viewport_rows: u16,
    filter: String,
    summary: String,
}

impl Default for TestsWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl TestsWidget {
    /// 创建空列表；`@test` 运行结果随后通过 [`Self::replace`] 提交。
    #[must_use]
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            viewport_rows: 1,
            filter: String::new(),
            summary: "No tests run yet; use `:test` to compile and run `@test` functions"
                .to_owned(),
        }
    }

    /// 用一次运行的结果替换列表，并选中第一个失败测试。
    pub fn replace(&mut self, rows: Vec<TestRow>, filter: String, summary: String) {
        self.selected = rows
            .iter()
            .position(|row| row.status.is_failure())
            .unwrap_or(0);
        self.scroll = 0;
        self.rows = rows;
        self.filter = filter;
        self.summary = summary;
    }

    /// 从会话恢复一次运行的显示状态。
    pub fn restore(
        &mut self,
        rows: Vec<TestRow>,
        selected: usize,
        scroll: usize,
        filter: String,
        summary: String,
    ) {
        self.selected = selected.min(rows.len().saturating_sub(1));
        if rows.is_empty() {
            self.scroll = 0;
        } else {
            self.scroll = scroll;
        }
        self.rows = rows;
        self.filter = filter;
        self.summary = summary;
    }

    /// 返回当前结果行。
    #[must_use]
    pub fn rows(&self) -> &[TestRow] {
        &self.rows
    }

    /// 返回选中行序号。
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// 返回垂直滚动偏移，单位为显示行。
    #[must_use]
    pub const fn scroll(&self) -> usize {
        self.scroll
    }

    /// 返回选中行。
    #[must_use]
    pub fn selected_row(&self) -> Option<&TestRow> {
        self.rows.get(self.selected)
    }

    /// 选中指定行；越界时保持不变。
    pub fn select(&mut self, index: usize) {
        if index < self.rows.len() {
            self.selected = index;
            self.reveal_selection();
        }
    }

    /// 返回本次运行使用的名称过滤器。
    #[must_use]
    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// 返回列表末尾显示的运行摘要。
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        self.reveal_selection();
    }

    /// 返回选中行在显示行序列中的下标；分组标题各占一行。
    fn display_index_of(&self, row_index: usize) -> usize {
        let mut line = 0;
        let mut group = None::<&str>;
        for (index, row) in self.rows.iter().enumerate() {
            if group != Some(row.source.as_str()) {
                group = Some(row.source.as_str());
                line += 1;
            }
            if index == row_index {
                return line;
            }
            line += 1;
        }
        line
    }

    /// 调整滚动偏移，使选中行落在最近的可见窗口内。
    fn reveal_selection(&mut self) {
        if self.rows.is_empty() {
            self.scroll = 0;
            return;
        }
        let viewport = usize::from(self.viewport_rows).max(1);
        let line = self.display_index_of(self.selected);
        if line < self.scroll {
            self.scroll = line;
        } else if line >= self.scroll.saturating_add(viewport) {
            self.scroll = line + 1 - viewport;
        }
    }

    /// 构造按源码文件分组的显示行。
    fn display_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let mut group = None::<&str>;
        for (index, row) in self.rows.iter().enumerate() {
            if group != Some(row.source.as_str()) {
                group = Some(row.source.as_str());
                let label = if row.source.is_empty() {
                    "<unknown source>".to_owned()
                } else {
                    row.source.clone()
                };
                lines.push(Line::from(Span::styled(
                    format!("== {label} =="),
                    Style::default().add_modifier(Modifier::BOLD),
                )));
            }
            let mut text = format!("  {:<8} {}", row.status.label(), row.name);
            if !row.message.is_empty() {
                text.push_str("  ");
                text.push_str(&row.message);
            }
            let style = if index == self.selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(text, style)));
        }
        if lines.is_empty() {
            lines.push(Line::from("  (no registered test matched)"));
        }
        lines.push(Line::from(String::new()));
        lines.push(Line::from(self.summary.clone()));
        lines
    }
}

impl Widget for TestsWidget {
    fn kind(&self) -> PanelKind {
        PanelKind::Messages
    }

    fn title(&self) -> String {
        if self.filter.is_empty() {
            "Tests".to_owned()
        } else {
            format!("Tests ({})", self.filter)
        }
    }

    fn status(&self) -> Option<PanelStatus> {
        Some(PanelStatus {
            mode: "tests".to_owned(),
            resource: self.summary.clone(),
            position: if self.rows.is_empty() {
                "0/0".to_owned()
            } else {
                format!("{}/{}", self.selected + 1, self.rows.len())
            },
        })
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: TuiRect, _focused: bool) {
        self.viewport_rows = area.height;
        self.reveal_selection();
        let lines = self.display_lines();
        let offset = u16::try_from(self.scroll).unwrap_or(u16::MAX);
        frame.render_widget(Paragraph::new(lines).scroll((offset, 0)), area);
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        match event.key {
            Key::Up | Key::Char('k') => {
                self.move_selection(-1);
                self.select_command()
            }
            Key::Down | Key::Char('j') => {
                self.move_selection(1);
                self.select_command()
            }
            Key::Home | Key::Char('g') => {
                self.selected = 0;
                self.scroll = 0;
                self.select_command()
            }
            Key::End | Key::Char('G') => {
                self.selected = self.rows.len().saturating_sub(1);
                self.reveal_selection();
                self.select_command()
            }
            Key::PageUp => {
                let step = isize::try_from(self.viewport_rows.max(1)).unwrap_or(isize::MAX);
                self.move_selection(-step);
                self.select_command()
            }
            Key::PageDown => {
                let step = isize::try_from(self.viewport_rows.max(1)).unwrap_or(isize::MAX);
                self.move_selection(step);
                self.select_command()
            }
            Key::Enter | Key::Char('o') => WidgetAction::Commands(vec![AppCommand::Command(
                format!("test-jump {}", self.selected),
            )]),
            Key::Char('r') => WidgetAction::Commands(vec![AppCommand::Command(format!(
                "test-rerun {}",
                self.selected
            ))]),
            _ => WidgetAction::Ignored,
        }
    }
}

impl TestsWidget {
    /// 选中行变化后刷新输出面板；选中行本身已在此处更新。
    fn select_command(&self) -> WidgetAction {
        WidgetAction::Commands(vec![AppCommand::Command(format!(
            "test-select {}",
            self.selected
        ))])
    }
}

#[cfg(test)]
mod unit_tests {
    use ratatui::{Terminal, backend::TestBackend};
    use teal_workbench_core::{AppCommand, InputEvent, Key, KeyModifiers};

    use super::{TestRow, TestRowStatus, TestsWidget, Widget, WidgetAction};

    fn row(name: &str, source: &str, status: TestRowStatus) -> TestRow {
        TestRow {
            name: name.to_owned(),
            source: source.to_owned(),
            path: None,
            line: 1,
            status,
            message: String::new(),
            output: String::new(),
        }
    }

    fn key(key: Key) -> InputEvent {
        InputEvent {
            key,
            modifiers: KeyModifiers::default(),
        }
    }

    fn rendered(widget: &mut TestsWidget, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| widget.render(frame, frame.area(), true))
            .expect("panel render");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn replaces_results_and_selects_the_first_failure() {
        let mut widget = TestsWidget::new();
        assert!(!widget.status().expect("status").position.is_empty());
        widget.replace(
            vec![
                row("passes", "main.tl", TestRowStatus::Passed),
                row("fails", "other.tl", TestRowStatus::Failed),
            ],
            String::new(),
            "1 passed; 1 failed; 2 selected".to_owned(),
        );
        assert_eq!(widget.selected(), 1);
        assert_eq!(widget.rows().len(), 2);
        assert_eq!(widget.title(), "Tests");
        assert_eq!(
            widget.selected_row().map(|row| row.name.as_str()),
            Some("fails")
        );

        // 分组标题按源码文件出现，状态标记与摘要同时渲染。
        let text = rendered(&mut widget, 60, 8);
        assert!(text.contains("== main.tl =="), "{text}");
        assert!(text.contains("== other.tl =="), "{text}");
        assert!(text.contains("ok"), "{text}");
        assert!(text.contains("FAIL"), "{text}");
        assert!(text.contains("1 passed; 1 failed; 2 selected"), "{text}");
    }

    #[test]
    fn moves_selection_and_emits_jump_and_rerun_commands() {
        let mut widget = TestsWidget::new();
        widget.replace(
            vec![
                row("first", "main.tl", TestRowStatus::Passed),
                row("second", "main.tl", TestRowStatus::ExitCode(4)),
            ],
            "second".to_owned(),
            "0 passed; 1 failed; 1 selected".to_owned(),
        );
        assert_eq!(widget.selected(), 1);
        assert_eq!(widget.filter(), "second");
        assert_eq!(widget.title(), "Tests (second)");

        match widget.handle_input(key(Key::Up)) {
            WidgetAction::Commands(commands) => assert_eq!(
                commands,
                vec![AppCommand::Command("test-select 0".to_owned())]
            ),
            other => panic!("expected a select command, received {other:?}"),
        }
        assert_eq!(widget.selected(), 0);
        match widget.handle_input(key(Key::End)) {
            WidgetAction::Commands(commands) => assert_eq!(
                commands,
                vec![AppCommand::Command("test-select 1".to_owned())]
            ),
            other => panic!("expected a select command, received {other:?}"),
        }
        assert_eq!(widget.selected(), 1);

        match widget.handle_input(key(Key::Enter)) {
            WidgetAction::Commands(commands) => assert_eq!(
                commands,
                vec![AppCommand::Command("test-jump 1".to_owned())]
            ),
            other => panic!("expected a jump command, received {other:?}"),
        }
        match widget.handle_input(key(Key::Char('r'))) {
            WidgetAction::Commands(commands) => assert_eq!(
                commands,
                vec![AppCommand::Command("test-rerun 1".to_owned())]
            ),
            other => panic!("expected a rerun command, received {other:?}"),
        }
        assert_eq!(
            widget.handle_input(key(Key::Char('x'))),
            WidgetAction::Ignored
        );
    }

    /// 构造 `count` 行测试，全部通过，源码分组固定为 `main.tl`。
    fn many_rows(count: usize) -> Vec<TestRow> {
        (0..count)
            .map(|index| row(&format!("case{index}"), "main.tl", TestRowStatus::Passed))
            .collect()
    }

    #[test]
    fn selection_stays_visible_and_scrolling_follows_the_viewport() {
        let mut widget = TestsWidget::new();
        widget.replace(many_rows(40), String::new(), "40 passed".to_owned());
        let _ = rendered(&mut widget, 40, 10);
        assert_eq!(widget.scroll(), 0);

        // 走到列表末尾：滚动偏移必须推进到让末行可见。
        assert!(matches!(
            widget.handle_input(key(Key::End)),
            WidgetAction::Commands(_)
        ));
        assert_eq!(widget.selected(), 39);
        let _ = rendered(&mut widget, 40, 10);
        assert!(
            widget.scroll() >= 31,
            "the last row must stay inside the 10-row viewport: {}",
            widget.scroll()
        );

        // 回到首行：滚动回到顶部。
        widget.handle_input(key(Key::Home));
        assert_eq!(widget.scroll(), 0);
        assert_eq!(widget.selected(), 0);
    }

    #[test]
    fn paging_moves_by_the_visible_window_and_emits_select_commands() {
        let mut widget = TestsWidget::new();
        widget.replace(many_rows(40), String::new(), "40 passed".to_owned());
        let _ = rendered(&mut widget, 40, 5);

        match widget.handle_input(key(Key::PageDown)) {
            WidgetAction::Commands(commands) => assert_eq!(
                commands,
                vec![AppCommand::Command("test-select 5".to_owned())]
            ),
            other => panic!("expected a select command, received {other:?}"),
        }
        assert_eq!(widget.selected(), 5);
        widget.handle_input(key(Key::PageDown));
        assert_eq!(widget.selected(), 10);
        widget.handle_input(key(Key::PageUp));
        assert_eq!(widget.selected(), 5);
    }

    #[test]
    fn restore_keeps_the_selected_row_and_clamps_out_of_range_state() {
        let mut widget = TestsWidget::new();
        widget.restore(many_rows(6), 4, 3, "case".to_owned(), "6 passed".to_owned());
        assert_eq!(widget.selected(), 4);
        assert_eq!(widget.filter(), "case");
        assert_eq!(widget.summary(), "6 passed");
        assert_eq!(widget.scroll(), 3, "restored offsets stay as recorded");

        widget.restore(Vec::new(), 5, 9, String::new(), "none".to_owned());
        assert_eq!(widget.selected(), 0);
        assert_eq!(widget.scroll(), 0, "an empty list cannot stay scrolled");
    }
}
