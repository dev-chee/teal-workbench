//! 共享缓冲区上的模态编辑视图。
//!
//! 选择区和滚动属于视图，文本与撤销历史属于缓冲区；输入最终转换为字符坐标事务。
//! 语法工作在线程中完成，只有当前修订的结果可以用于绘制。

use std::path::Path;

use ratatui::{
    Frame,
    layout::{Constraint, Direction as LayoutDirection, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use teal_workbench_core::{
    AppCommand, BufferHandle, Document, EditorMode, InputEvent, Key, KeymapPreset, MouseButton,
    MouseEvent, MouseEventKind, Selection, SelectionSet, TextEdit, Transaction,
};
use teal_workbench_core::syntax::{Capture, GrammarSpec, HighlightQuery, Parser, ParserTable};
mod syntax;
use syntax::SyntaxState;

use super::{
    ClipboardAction, ClipboardOutcome, ContextMenu, DecorationKind, Decorations, GutterType,
    MenuItem, PanelKind, PanelStatus, TextPoint, TextSelection, Widget, WidgetAction, cell_to_char,
    char_to_cell, expand_line_tabs, style_lines_with_selection, word_range,
};

/// 由 [`Document`] 事务与选择区支持的模态编辑器面板。
pub struct EditorPanel {
    document: BufferHandle,
    mode: EditorMode,
    keymap: KeymapPreset,
    viewport_line: usize,
    viewport_column: usize,
    mouse_drag_anchor: Option<usize>,
    pending_operator: Option<char>,
    last_area: Rect,
    syntax: Option<SyntaxState>,
    semantic_captures: Vec<Capture>,
    diagnostic_captures: Vec<Capture>,
    /// 最近一次工作区写入改动的行区间；保存或重新加载后清空。
    changed_lines: Vec<std::ops::Range<usize>>,
    /// 当前搜索命中的字节区间；下一次搜索或清空时替换。
    search_captures: Vec<Capture>,
    /// 由上述来源统一投影出的 gutter/装饰视图；渲染前刷新。
    decorations: Decorations,
}

impl EditorPanel {
    /// 为一个文档创建编辑器。
    #[must_use]
    pub fn new(document: Document, keymap: KeymapPreset) -> Self {
        Self::from_buffer(&BufferHandle::new(document), keymap)
    }

    /// 为工作区拥有的缓冲区创建视图。
    pub fn from_buffer(document: &BufferHandle, keymap: KeymapPreset) -> Self {
        Self {
            document: document.fork_view(),
            mode: EditorMode::Normal,
            keymap,
            viewport_line: 0,
            viewport_column: 0,
            mouse_drag_anchor: None,
            pending_operator: None,
            last_area: Rect::default(),
            syntax: None,
            semantic_captures: Vec::new(),
            diagnostic_captures: Vec::new(),
            changed_lines: Vec::new(),
            search_captures: Vec::new(),
            decorations: Decorations::default(),
        }
    }

    /// 在新编辑器面板中打开路径。
    pub fn open(
        path: impl AsRef<Path>,
        keymap: KeymapPreset,
    ) -> Result<Self, teal_workbench_core::DocumentError> {
        Document::open(path).map(|document| Self::new(document, keymap))
    }

    /// 返回后备文档。
    #[must_use]
    pub const fn document(&self) -> &BufferHandle {
        &self.document
    }

    /// 返回可变后备文档。
    pub const fn document_mut(&mut self) -> &BufferHandle {
        &self.document
    }

    /// 将此视图绑定到规范工作区缓冲区。
    pub fn bind_buffer(&mut self, buffer: &BufferHandle) {
        let selections = self.document.selections();
        self.document = buffer.fork_view();
        self.document.set_selections(selections);
        self.refresh_syntax();
    }

    /// 返回供会话使用的垂直和水平视图偏移。
    pub const fn view_state(&self) -> (usize, usize) {
        (self.viewport_line, self.viewport_column)
    }

    /// 恢复受限视图偏移，不改变选择区。
    pub fn restore_view(&mut self, line: usize, column: usize) {
        self.viewport_line = line.min(self.document.text().len_lines().saturating_sub(1));
        self.viewport_column = column;
    }

    /// 返回当前编辑器模式。
    #[must_use]
    pub const fn mode(&self) -> EditorMode {
        self.mode
    }

    /// 更改键位风格而不改变选择区或历史。
    pub const fn set_keymap(&mut self, keymap: KeymapPreset) {
        self.keymap = keymap;
    }

    /// 附加文法并解析当前文档。
    pub fn set_grammar(
        &mut self,
        grammar: &GrammarSpec,
    ) -> Result<(), teal_workbench_core::syntax::ParseError> {
        let parser = Parser::new(grammar)?;
        let query = HighlightQuery::from_grammar(grammar);
        self.syntax = Some(SyntaxState::new(
            parser,
            query,
            grammar.injections.clone(),
            Vec::new(),
        ));
        self.refresh_syntax();
        Ok(())
    }

    /// 使用兼容的缓存解析表附加文法。
    pub fn set_grammar_with_table(
        &mut self,
        grammar: &GrammarSpec,
        table: ParserTable,
    ) -> Result<(), teal_workbench_core::syntax::ParseError> {
        let parser = Parser::with_table(grammar, table)?;
        let query = HighlightQuery::from_grammar(grammar);
        self.syntax = Some(SyntaxState::new(
            parser,
            query,
            grammar.injections.clone(),
            Vec::new(),
        ));
        self.refresh_syntax();
        Ok(())
    }

    /// 附加已缓存宿主文法与嵌入语言；解析及有界注入遍历在工作线程上运行。
    pub fn set_grammar_environment(
        &mut self,
        grammar: &GrammarSpec,
        table: ParserTable,
        languages: Vec<(GrammarSpec, ParserTable)>,
    ) -> Result<(), teal_workbench_core::syntax::ParseError> {
        let parser = Parser::with_table(grammar, table)?;
        self.syntax = Some(SyntaxState::new(
            parser,
            HighlightQuery::from_grammar(grammar),
            grammar.injections.clone(),
            languages,
        ));
        self.refresh_syntax();
        Ok(())
    }

    /// 停止接收语法作业，断开待处理结果通道，并返回由宿主负责关闭的线程。不在 UI 线程阻塞或 join；正在进行的有界解析可能在退出前完成，后续调用返回 None。
    pub fn stop_syntax(&mut self) -> Option<std::thread::JoinHandle<()>> {
        self.syntax.take().map(SyntaxState::stop)
    }

    /// 替换较高优先级的语义/LSP 捕获。
    pub fn set_semantic_captures(&mut self, captures: Vec<Capture>) {
        self.semantic_captures = captures;
    }

    /// 替换诊断而不丢弃语义 token 高亮。
    pub fn set_diagnostic_captures(&mut self, captures: Vec<Capture>) {
        self.diagnostic_captures = captures;
    }

    /// 标记最近一次写入改动的行区间，供渲染时加一层弱背景。
    pub fn mark_changed_lines(&mut self, lines: Vec<std::ops::Range<usize>>) {
        self.changed_lines = lines;
    }

    /// 清除改动标记（保存、重新加载或用户确认后调用）。
    pub fn clear_changed_lines(&mut self) {
        self.changed_lines.clear();
    }

    /// 替换当前搜索命中高亮。
    pub fn set_search_captures(&mut self, captures: Vec<Capture>) {
        self.search_captures = captures;
    }

    /// 返回当前搜索命中的字节区间。
    #[must_use]
    pub fn search_capture_ranges(&self) -> Vec<std::ops::Range<usize>> {
        self.search_captures
            .iter()
            .map(|capture| capture.range.clone())
            .collect()
    }

    /// 返回当前搜索命中数量。
    #[must_use]
    pub fn search_hits(&self) -> usize {
        self.search_captures.len()
    }
    /// 返回当前标记的行区间数量。
    #[must_use]
    pub fn changed_line_count(&self) -> usize {
        self.changed_lines.iter().map(std::ops::Range::len).sum()
    }

    /// 返回当前 gutter 与装饰视图。
    ///
    /// 编辑器把诊断、搜索命中、Agent 改动与选区统一投影为 [`Decorations`]，编辑器自身
    /// 用它绘制 gutter 与行内标记；会话面板等其他视图可以请求同一类型以复用语义。
    #[must_use]
    pub fn decorations(&self) -> Decorations {
        self.project_decorations()
    }

    /// 把逐行来源投影为装饰视图。
    fn project_decorations(&self) -> Decorations {
        let mut decorations = Decorations::default();
        for range in &self.changed_lines {
            for line in range.clone() {
                decorations.push_gutter(line, GutterType::Tool);
            }
        }
        for capture in &self.diagnostic_captures {
            let Some((line, start, end)) = self.capture_line_span(capture) else {
                continue;
            };
            decorations.push_gutter(line, GutterType::Warning);
            decorations.push_range(line, start, end, DecorationKind::Diagnostic);
        }
        for capture in &self.search_captures {
            let Some((line, start, end)) = self.capture_line_span(capture) else {
                continue;
            };
            decorations.push_range(line, start, end, DecorationKind::Search);
        }
        decorations
    }

    /// 把字节区间捕获换算为零基行号与行内字符列区间。
    fn capture_line_span(&self, capture: &Capture) -> Option<(usize, usize, usize)> {
        let line = self.document.text().byte_to_line(capture.range.start);
        let line_byte = self.document.text().line_to_byte(line);
        let text = self.document.text().line(line).to_string();
        let start = capture
            .range
            .start
            .saturating_sub(line_byte)
            .min(text.len());
        let end = capture.range.end.saturating_sub(line_byte).min(text.len());
        let start_chars = text.get(..start).map_or(0, |prefix| prefix.chars().count());
        let end_chars = text
            .get(..end)
            .map_or(start_chars, |prefix| prefix.chars().count());
        Some((line, start_chars, end_chars.max(start_chars + 1)))
    }

    /// 返回指定行的 gutter 标记字符；没有标记时返回空格。
    fn gutter_marker(&self, line: usize) -> char {
        let markers = self.decorations.gutter(line);
        if markers.contains(&GutterType::Error) {
            'E'
        } else if markers.contains(&GutterType::Warning) {
            'W'
        } else if markers.contains(&GutterType::Breakpoint) {
            'B'
        } else if markers.contains(&GutterType::Tool) {
            '*'
        } else if markers.contains(&GutterType::Vcs) {
            '+'
        } else {
            ' '
        }
    }

    /// 历史存在父节点时撤销一个编辑器事务。
    pub fn undo(&mut self) {
        let _ = self.document.undo();
        self.refresh_syntax();
        self.ensure_cursor_visible();
    }

    /// 可用时重做第一个保留分支。
    pub fn redo(&mut self) {
        let _ = self.document.redo(0);
        self.refresh_syntax();
        self.ensure_cursor_visible();
    }

    fn refresh_syntax(&mut self) {
        if let Some(syntax) = &mut self.syntax {
            syntax.update(&self.document);
        }
    }

    fn insert_text(&mut self, text: &str) {
        let selections = self.document.selections().selections().to_vec();
        let edits = selections
            .iter()
            .map(|selection| TextEdit {
                range: selection.range(),
                insert: text.to_owned(),
            })
            .collect::<Vec<_>>();
        let Ok(transaction) = Transaction::new(edits) else {
            return;
        };
        let mut delta = 0_isize;
        let inserted = text.chars().count();
        let cursors = selections
            .iter()
            .map(|selection| {
                let range = selection.range();
                let at = range.start.saturating_add_signed(delta) + inserted;
                delta += inserted.cast_signed() - (range.end - range.start).cast_signed();
                Selection::cursor(at)
            })
            .collect();
        if self.document.apply(transaction).is_ok() {
            self.document.set_selections(SelectionSet::new(cursors, 0));
        }
    }

    fn delete_ranges(&mut self, backwards: bool) {
        let len = self.document.text().len_chars();
        let ranges = self
            .document
            .selections()
            .selections()
            .iter()
            .map(|selection| {
                let range = selection.range();
                if !range.is_empty() {
                    range
                } else if backwards {
                    range.start.saturating_sub(1)..range.start
                } else {
                    range.start..(range.start + 1).min(len)
                }
            })
            .collect::<Vec<_>>();
        let edits = ranges
            .iter()
            .filter(|range| !range.is_empty())
            .cloned()
            .map(Transaction::delete)
            .flat_map(|transaction| transaction.edits().to_vec())
            .collect::<Vec<_>>();
        let Ok(transaction) = Transaction::new(edits) else {
            return;
        };
        let mut delta = 0_isize;
        let cursors = ranges
            .iter()
            .map(|range| {
                let at = range.start.saturating_add_signed(delta);
                delta -= (range.end - range.start).cast_signed();
                Selection::cursor(at)
            })
            .collect();
        if self.document.apply(transaction).is_ok() {
            self.document.set_selections(SelectionSet::new(cursors, 0));
        }
    }

    fn delete_lines(&mut self) {
        let rope = self.document.text();
        let mut ranges = Vec::new();
        for selection in self.document.selections().selections() {
            let start_line = rope.char_to_line(selection.range().start);
            let end_line = rope.char_to_line(selection.range().end.min(rope.len_chars()));
            let start = rope.line_to_char(start_line);
            let end = if end_line + 1 < rope.len_lines() {
                rope.line_to_char(end_line + 1)
            } else {
                rope.len_chars()
            };
            ranges.push(start..end);
        }
        let selections = SelectionSet::new(
            ranges
                .iter()
                .map(|range| Selection {
                    anchor: range.start,
                    head: range.end,
                })
                .collect(),
            0,
        );
        self.document.set_selections(selections);
        self.delete_ranges(false);
    }

    fn move_selections(&mut self, horizontal: isize, vertical: isize, extend: bool) {
        let rope = self.document.text();
        let len = rope.len_chars();
        let selections = self
            .document
            .selections()
            .selections()
            .iter()
            .map(|selection| {
                let mut head = selection.head;
                if horizontal != 0 {
                    head = head.saturating_add_signed(horizontal).min(len);
                }
                if vertical != 0 {
                    let line = rope.char_to_line(head.min(len));
                    let line_start = rope.line_to_char(line);
                    let column = head - line_start;
                    let target_line = line
                        .saturating_add_signed(vertical)
                        .min(rope.len_lines().saturating_sub(1));
                    let target_start = rope.line_to_char(target_line);
                    let target_len = rope
                        .line(target_line)
                        .len_chars()
                        .saturating_sub(usize::from(target_line + 1 < rope.len_lines()));
                    head = target_start + column.min(target_len);
                }
                Selection {
                    anchor: if extend { selection.anchor } else { head },
                    head,
                }
            })
            .collect();
        self.document
            .set_selections(SelectionSet::new(selections, 0));
        self.ensure_cursor_visible();
    }

    fn move_word(&mut self, extend: bool) {
        let text = self.document.to_string();
        let chars = text.chars().collect::<Vec<_>>();
        let selections = self
            .document
            .selections()
            .selections()
            .iter()
            .map(|selection| {
                let mut head = selection.head.min(chars.len());
                while head < chars.len() && chars[head].is_alphanumeric() {
                    head += 1;
                }
                while head < chars.len() && !chars[head].is_alphanumeric() {
                    head += 1;
                }
                Selection {
                    anchor: if extend { selection.anchor } else { head },
                    head,
                }
            })
            .collect();
        self.document
            .set_selections(SelectionSet::new(selections, 0));
    }

    fn ensure_cursor_visible(&mut self) {
        let line = self.document.text().char_to_line(
            self.document
                .selections()
                .primary()
                .head
                .min(self.document.text().len_chars()),
        );
        let height = self.last_area.height.saturating_sub(2) as usize;
        if line < self.viewport_line {
            self.viewport_line = line;
        } else if height > 0 && line >= self.viewport_line + height {
            self.viewport_line = line + 1 - height;
        }
        let head = self
            .document
            .selections()
            .primary()
            .head
            .min(self.document.text().len_chars());
        let line_start = self.document.text().line_to_char(line);
        let line_text = self.line_text(line);
        let cell = char_to_cell(&line_text, head.saturating_sub(line_start));
        let content_width = self.content_width();
        if cell < self.viewport_column {
            self.viewport_column = cell;
        } else if content_width > 0 && cell >= self.viewport_column + content_width {
            self.viewport_column = cell + 1 - content_width;
        }
    }

    fn line_text(&self, line: usize) -> String {
        self.document
            .text()
            .line(line.min(self.document.text().len_lines().saturating_sub(1)))
            .to_string()
            .trim_end_matches(['\r', '\n'])
            .to_owned()
    }

    fn gutter_width(&self) -> usize {
        self.document.text().len_lines().to_string().len().max(2) + 1
    }

    fn content_width(&self) -> usize {
        usize::from(self.last_area.width.saturating_sub(2)).saturating_sub(self.gutter_width())
    }

    fn selection_mode(&self) -> EditorMode {
        if self.keymap == KeymapPreset::HelixFirst {
            EditorMode::Select
        } else {
            EditorMode::Visual
        }
    }

    fn point_at(&mut self, event: MouseEvent, area: Rect, auto_scroll: bool) -> usize {
        let inner = Block::default().borders(Borders::ALL).inner(area);
        if auto_scroll {
            if event.row <= inner.y {
                self.viewport_line = self.viewport_line.saturating_sub(1);
            } else if event.row >= inner.bottom().saturating_sub(1) {
                self.viewport_line = (self.viewport_line + 1)
                    .min(self.document.text().len_lines().saturating_sub(1));
            }
            let content_x = inner
                .x
                .saturating_add(u16::try_from(self.gutter_width()).unwrap_or(u16::MAX));
            if event.column <= content_x {
                self.viewport_column = self.viewport_column.saturating_sub(1);
            } else if event.column >= inner.right().saturating_sub(1) {
                self.viewport_column = self.viewport_column.saturating_add(1);
            }
        }
        let visible_row = event
            .row
            .saturating_sub(inner.y)
            .min(inner.height.saturating_sub(1));
        let line = self
            .viewport_line
            .saturating_add(usize::from(visible_row))
            .min(self.document.text().len_lines().saturating_sub(1));
        let content_x = inner
            .x
            .saturating_add(u16::try_from(self.gutter_width()).unwrap_or(u16::MAX));
        let visible_cell = event.column.saturating_sub(content_x);
        let cell = self
            .viewport_column
            .saturating_add(usize::from(visible_cell));
        let line_text = self.line_text(line);
        self.document.text().line_to_char(line) + cell_to_char(&line_text, cell)
    }

    fn set_mouse_selection(&mut self, anchor: usize, head: usize) {
        self.document
            .set_selections(SelectionSet::single(Selection { anchor, head }));
        self.mode = self.selection_mode();
        self.ensure_cursor_visible();
    }

    fn selected_primary_text(&self) -> Option<String> {
        let range = self.document.selections().primary().range();
        (!range.is_empty()).then(|| self.document.text().slice(range).to_string())
    }

    fn text_point(&self, position: usize) -> TextPoint {
        let position = position.min(self.document.text().len_chars());
        let line = self.document.text().char_to_line(position);
        TextPoint {
            line,
            column: position.saturating_sub(self.document.text().line_to_char(line)),
        }
    }

    fn normalize_paste(text: &str) -> String {
        text.replace("\r\n", "\n").replace('\r', "\n")
    }

    fn normal_key(&mut self, event: InputEvent) -> WidgetAction {
        if event.modifiers.control {
            return match event.key {
                Key::Char('s') => WidgetAction::Commands(vec![AppCommand::Save]),
                Key::Char('r') => {
                    let _ = self.document.redo(0);
                    WidgetAction::Consumed
                }
                _ => WidgetAction::Ignored,
            };
        }
        match event.key {
            Key::Char('h') | Key::Left => self.move_selections(-1, 0, false),
            Key::Char('l') | Key::Right => self.move_selections(1, 0, false),
            Key::Char('j') | Key::Down => self.move_selections(0, 1, false),
            Key::Char('k') | Key::Up => self.move_selections(0, -1, false),
            Key::Char('w') => self.move_word(false),
            Key::Char('i') => self.mode = EditorMode::Insert,
            Key::Char('a') => {
                self.move_selections(1, 0, false);
                self.mode = EditorMode::Insert;
            }
            Key::Char('v') => {
                self.mode = if self.keymap == KeymapPreset::HelixFirst {
                    EditorMode::Select
                } else {
                    EditorMode::Visual
                }
            }
            Key::Char('x') if self.keymap == KeymapPreset::HelixFirst => {
                self.move_selections(0, 1, true);
                self.mode = EditorMode::Select;
            }
            Key::Char('x') => self.delete_ranges(false),
            Key::Char('d') => {
                self.pending_operator = Some('d');
                self.mode = EditorMode::OperatorPending;
            }
            Key::Char('u') => {
                let _ = self.document.undo();
            }
            Key::Char('r') => self.mode = EditorMode::Replace,
            _ => return WidgetAction::Ignored,
        }
        WidgetAction::Consumed
    }

    fn selection_key(&mut self, event: InputEvent) -> WidgetAction {
        match event.key {
            Key::Escape => {
                let cursors = self
                    .document
                    .selections()
                    .selections()
                    .iter()
                    .map(|selection| Selection::cursor(selection.head))
                    .collect();
                self.document.set_selections(SelectionSet::new(cursors, 0));
                self.mode = EditorMode::Normal;
            }
            Key::Char('h') | Key::Left => self.move_selections(-1, 0, true),
            Key::Char('l') | Key::Right => self.move_selections(1, 0, true),
            Key::Char('j') | Key::Down => self.move_selections(0, 1, true),
            Key::Char('k') | Key::Up => self.move_selections(0, -1, true),
            Key::Char('w') => self.move_word(true),
            Key::Char('d' | 'x') => {
                self.delete_ranges(false);
                self.mode = EditorMode::Normal;
            }
            Key::Char('c') => {
                self.delete_ranges(false);
                self.mode = EditorMode::Insert;
            }
            _ => return WidgetAction::Ignored,
        }
        WidgetAction::Consumed
    }
}

impl Widget for EditorPanel {
    fn kind(&self) -> PanelKind {
        PanelKind::Editor
    }

    fn title(&self) -> String {
        self.document
            .path()
            .as_deref()
            .and_then(Path::file_name)
            .map_or_else(
                || "[unnamed]".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            )
    }

    fn status(&self) -> Option<PanelStatus> {
        let selection = self.document.selections().primary();
        let line = self
            .document
            .text()
            .char_to_line(selection.head.min(self.document.text().len_chars()));
        let column = selection.head - self.document.text().line_to_char(line);
        Some(PanelStatus {
            mode: if self.document.is_busy() {
                "busy".into()
            } else {
                match self.mode {
                    EditorMode::Normal => "normal",
                    EditorMode::Insert => "insert",
                    EditorMode::Visual => "visual",
                    EditorMode::Select => "select",
                    EditorMode::OperatorPending => "operator pending",
                    EditorMode::Replace => "replace",
                    EditorMode::Command => "command",
                    EditorMode::SearchForward => "search forward",
                    EditorMode::SearchBackward => "search backward",
                    EditorMode::Prompt => "prompt",
                }
                .into()
            },
            resource: format!(
                "{}{} {:?}/{:?}",
                self.title(),
                if self.document.is_dirty() { " [+]" } else { "" },
                self.document.encoding(),
                self.document.line_ending()
            ),
            position: format!(
                "{}:{} selections:{}",
                line + 1,
                column + 1,
                self.document.selections().selections().len()
            ),
        })
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        let revision = self.document.revision();
        let action = match self.mode {
            EditorMode::Normal => self.normal_key(event),
            EditorMode::Insert => match event.key {
                Key::Escape => {
                    self.mode = EditorMode::Normal;
                    WidgetAction::Consumed
                }
                Key::Backspace => {
                    self.delete_ranges(true);
                    WidgetAction::Consumed
                }
                Key::Delete => {
                    self.delete_ranges(false);
                    WidgetAction::Consumed
                }
                Key::Enter => {
                    self.insert_text("\n");
                    WidgetAction::Consumed
                }
                Key::Tab => {
                    self.insert_text("    ");
                    WidgetAction::Consumed
                }
                Key::Char(character) if !event.modifiers.control && !event.modifiers.alt => {
                    self.insert_text(&character.to_string());
                    WidgetAction::Consumed
                }
                _ => WidgetAction::Ignored,
            },
            EditorMode::Visual | EditorMode::Select => self.selection_key(event),
            EditorMode::OperatorPending => {
                self.mode = EditorMode::Normal;
                let operator = self.pending_operator.take();
                match (operator, event.key) {
                    (Some('d'), Key::Char('d')) => self.delete_lines(),
                    (Some('d'), Key::Char('w')) => {
                        self.move_word(true);
                        self.delete_ranges(false);
                    }
                    _ => return WidgetAction::Ignored,
                }
                WidgetAction::Consumed
            }
            EditorMode::Replace => {
                self.mode = EditorMode::Normal;
                match event.key {
                    Key::Escape => WidgetAction::Consumed,
                    Key::Char(character) => {
                        self.delete_ranges(false);
                        self.insert_text(&character.to_string());
                        WidgetAction::Consumed
                    }
                    _ => WidgetAction::Ignored,
                }
            }
            EditorMode::Command
            | EditorMode::SearchForward
            | EditorMode::SearchBackward
            | EditorMode::Prompt => WidgetAction::Ignored,
        };
        if self.document.revision() != revision {
            self.refresh_syntax();
        }
        action
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: Rect) -> WidgetAction {
        self.last_area = area;
        let revision = self.document.revision();
        let action = match event.kind {
            MouseEventKind::ScrollUp if event.modifiers.shift => {
                self.viewport_column = self.viewport_column.saturating_sub(4);
                WidgetAction::Consumed
            }
            MouseEventKind::ScrollDown if event.modifiers.shift => {
                self.viewport_column = self.viewport_column.saturating_add(4);
                WidgetAction::Consumed
            }
            MouseEventKind::ScrollUp => {
                self.viewport_line = self.viewport_line.saturating_sub(3);
                WidgetAction::Consumed
            }
            MouseEventKind::ScrollDown => {
                self.viewport_line = (self.viewport_line + 3)
                    .min(self.document.text().len_lines().saturating_sub(1));
                WidgetAction::Consumed
            }
            MouseEventKind::ScrollLeft => {
                self.viewport_column = self.viewport_column.saturating_sub(4);
                WidgetAction::Consumed
            }
            MouseEventKind::ScrollRight => {
                self.viewport_column = self.viewport_column.saturating_add(4);
                WidgetAction::Consumed
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let target = self.point_at(event, area, false);
                if event.click_count >= 3 {
                    let line = self.document.text().char_to_line(target);
                    let start = self.document.text().line_to_char(line);
                    let end = if line + 1 < self.document.text().len_lines() {
                        self.document.text().line_to_char(line + 1)
                    } else {
                        self.document.text().len_chars()
                    };
                    self.set_mouse_selection(start, end);
                    self.mouse_drag_anchor = Some(start);
                } else if event.click_count == 2 {
                    let line = self.document.text().char_to_line(target);
                    let start = self.document.text().line_to_char(line);
                    let range = word_range(&self.line_text(line), target.saturating_sub(start));
                    self.set_mouse_selection(start + range.start, start + range.end);
                    self.mouse_drag_anchor = Some(start + range.start);
                } else if event.modifiers.control {
                    let mut selections = self.document.selections().selections().to_vec();
                    if let Some(index) = selections
                        .iter()
                        .position(|selection| selection.is_cursor() && selection.head == target)
                        && selections.len() > 1
                    {
                        selections.remove(index);
                        self.document
                            .set_selections(SelectionSet::new(selections, 0));
                    } else {
                        selections.push(Selection::cursor(target));
                        let primary = selections.len() - 1;
                        self.document
                            .set_selections(SelectionSet::new(selections, primary));
                    }
                    self.mouse_drag_anchor = None;
                } else if event.modifiers.shift {
                    let anchor = self.document.selections().primary().anchor;
                    self.set_mouse_selection(anchor, target);
                    self.mouse_drag_anchor = Some(anchor);
                } else {
                    self.document
                        .set_selections(SelectionSet::single(Selection::cursor(target)));
                    if matches!(self.mode, EditorMode::Visual | EditorMode::Select) {
                        self.mode = EditorMode::Normal;
                    }
                    self.mouse_drag_anchor = Some(target);
                }
                self.ensure_cursor_visible();
                WidgetAction::Consumed
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let target = self.point_at(event, area, true);
                if let Some(anchor) = self.mouse_drag_anchor {
                    self.set_mouse_selection(anchor, target);
                }
                WidgetAction::Consumed
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.mouse_drag_anchor = None;
                WidgetAction::Consumed
            }
            MouseEventKind::Down(MouseButton::Middle) => {
                let target = self.point_at(event, area, false);
                self.document
                    .set_selections(SelectionSet::single(Selection::cursor(target)));
                WidgetAction::Clipboard(ClipboardAction::Paste)
            }
            MouseEventKind::Down(MouseButton::Right) => {
                let target = self.point_at(event, area, false);
                let primary = self.document.selections().primary();
                if primary.is_cursor() || !primary.range().contains(&target) {
                    self.document
                        .set_selections(SelectionSet::single(Selection::cursor(target)));
                }
                let has_selection = self.selected_primary_text().is_some();
                WidgetAction::Menu(ContextMenu {
                    x: event.column,
                    y: event.row,
                    items: vec![
                        if has_selection {
                            MenuItem::enabled("clipboard.cut", "Cut")
                        } else {
                            MenuItem::disabled("clipboard.cut", "Cut")
                        },
                        if has_selection {
                            MenuItem::enabled("clipboard.copy", "Copy")
                        } else {
                            MenuItem::disabled("clipboard.copy", "Copy")
                        },
                        MenuItem::enabled("clipboard.paste", "Paste"),
                        MenuItem::enabled("clipboard.select_all", "Select all"),
                        MenuItem::enabled("editor.undo", "Undo"),
                        MenuItem::enabled("editor.redo", "Redo"),
                        MenuItem::enabled("editor.save", "Save"),
                    ],
                })
            }
            _ => WidgetAction::Ignored,
        };
        if self.document.revision() != revision {
            self.refresh_syntax();
        }
        action
    }

    fn handle_clipboard(
        &mut self,
        action: ClipboardAction,
        paste: Option<&str>,
    ) -> ClipboardOutcome {
        let mut outcome = ClipboardOutcome {
            consumed: true,
            write: None,
        };
        match action {
            ClipboardAction::Copy => outcome.write = self.selected_primary_text(),
            ClipboardAction::Cut => {
                if self.document.is_read_only() {
                    return ClipboardOutcome::default();
                }
                let selection = self.document.selections().primary();
                let range = selection.range();
                outcome.write = self.selected_primary_text();
                if outcome.write.is_some()
                    && self
                        .document
                        .apply(Transaction::delete(range.clone()))
                        .is_ok()
                {
                    self.document
                        .set_selections(SelectionSet::single(Selection::cursor(range.start)));
                    self.mode = EditorMode::Normal;
                    self.refresh_syntax();
                }
            }
            ClipboardAction::Paste => {
                if self.document.is_read_only() {
                    return ClipboardOutcome::default();
                }
                let Some(paste) = paste else {
                    return ClipboardOutcome::default();
                };
                let paste = Self::normalize_paste(paste);
                let range = self.document.selections().primary().range();
                let cursor = range.start + paste.chars().count();
                let transaction = Transaction::new(vec![TextEdit {
                    range,
                    insert: paste,
                }]);
                if let Ok(transaction) = transaction
                    && self.document.apply(transaction).is_ok()
                {
                    self.document
                        .set_selections(SelectionSet::single(Selection::cursor(cursor)));
                    self.refresh_syntax();
                }
            }
            ClipboardAction::SelectAll => {
                self.document
                    .set_selections(SelectionSet::single(Selection {
                        anchor: 0,
                        head: self.document.text().len_chars(),
                    }));
                self.mode = self.selection_mode();
            }
        }
        self.ensure_cursor_visible();
        outcome
    }

    fn handle_menu(&mut self, id: &str) -> WidgetAction {
        match id {
            "editor.undo" => {
                self.undo();
                WidgetAction::Consumed
            }
            "editor.redo" => {
                self.redo();
                WidgetAction::Consumed
            }
            "editor.save" => WidgetAction::Commands(vec![AppCommand::Save]),
            _ => WidgetAction::Ignored,
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        self.refresh_syntax();
        self.last_area = area;
        self.ensure_cursor_visible();
        let block = Block::default()
            .title(self.title())
            .borders(Borders::ALL)
            .border_style(if focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            });
        let inner = block.inner(area);
        let width = self.document.text().len_lines().to_string().len().max(2);
        let gutter_width = u16::try_from(width.saturating_add(1)).unwrap_or(u16::MAX);
        let columns = Layout::default()
            .direction(LayoutDirection::Horizontal)
            .constraints([Constraint::Length(gutter_width), Constraint::Min(0)])
            .split(inner);
        let mut captures = self
            .syntax
            .as_ref()
            .map(|syntax| syntax.captures.clone())
            .unwrap_or_default();
        captures.extend(self.semantic_captures.iter().cloned());
        captures.extend(self.diagnostic_captures.iter().cloned());
        captures.extend(self.search_captures.iter().cloned());
        // gutter 由统一装饰视图驱动：行号 + 诊断/改动等行首标记。
        self.decorations = self.project_decorations();
        let gutter = (0..usize::from(inner.height))
            .map(|offset| {
                let number = self.viewport_line + offset + 1;
                if number > self.document.text().len_lines() {
                    return Line::raw("");
                }
                let line_index = self.viewport_line + offset;
                let marker = self.gutter_marker(line_index);
                Line::styled(
                    format!("{marker}{number:>width$} "),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                )
            })
            .collect::<Vec<_>>();
        let mut lines = self
            .document
            .text()
            .lines()
            .skip(self.viewport_line)
            .take(inner.height as usize)
            .enumerate()
            .map(|(offset, line)| {
                let text = line.to_string();
                let text = text.trim_end_matches(['\r', '\n']);
                let line_index = self.viewport_line + offset;
                let line_byte = self.document.text().line_to_byte(line_index);
                let rendered = Line::from(styled_spans(text, line_byte, &captures));
                // Agent 写入后保留弱背景标记，直到保存或重新加载。
                if self
                    .changed_lines
                    .iter()
                    .any(|range| range.contains(&line_index))
                {
                    rendered.style(Style::default().bg(Color::DarkGray))
                } else {
                    rendered
                }
            })
            .collect::<Vec<_>>();
        for selection in self.document.selections().selections() {
            lines = style_lines_with_selection(
                &lines,
                Some(TextSelection {
                    anchor: self.text_point(selection.anchor),
                    head: self.text_point(selection.head),
                }),
                self.viewport_line,
            );
        }
        lines = expand_line_tabs(&lines);
        frame.render_widget(block, area);
        frame.render_widget(Paragraph::new(gutter), columns[0]);
        frame.render_widget(
            Paragraph::new(lines)
                .scroll((0, u16::try_from(self.viewport_column).unwrap_or(u16::MAX))),
            columns[1],
        );
    }

    fn cursor(&self, area: Rect) -> Option<(u16, u16)> {
        let head = self
            .document
            .selections()
            .primary()
            .head
            .min(self.document.text().len_chars());
        let line = self.document.text().char_to_line(head);
        let column = head - self.document.text().line_to_char(line);
        let line_text = self.line_text(line);
        let cell = char_to_cell(&line_text, column);
        let number_width = self.gutter_width();
        let y = u16::try_from(line.checked_sub(self.viewport_line)?)
            .ok()?
            .saturating_add(1);
        let x = u16::try_from(number_width)
            .ok()?
            .saturating_add(u16::try_from(cell.checked_sub(self.viewport_column)?).ok()?)
            .saturating_add(1);
        (x < area.width && y < area.height).then_some((x, y))
    }
}

fn styled_spans(text: &str, line_byte: usize, captures: &[Capture]) -> Vec<Span<'static>> {
    if text.is_empty() {
        return vec![Span::raw("")];
    }
    let line_end = line_byte + text.len();
    let relevant = captures
        .iter()
        .filter(|capture| capture.range.start < line_end && capture.range.end > line_byte)
        .collect::<Vec<_>>();
    let mut boundaries = vec![line_byte, line_end];
    for capture in &relevant {
        boundaries.push(capture.range.start.max(line_byte));
        boundaries.push(capture.range.end.min(line_end));
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries
        .windows(2)
        .filter_map(|range| {
            let start = range[0] - line_byte;
            let end = range[1] - line_byte;
            let segment = text.get(start..end)?;
            let capture = relevant
                .iter()
                .filter(|capture| capture.range.start <= range[0] && capture.range.end >= range[1])
                .max_by_key(|capture| capture.priority);
            Some(Span::styled(
                segment.to_owned(),
                capture.map_or_else(Style::default, |capture| capture_style(&capture.name)),
            ))
        })
        .collect()
}

fn capture_style(capture: &str) -> Style {
    match capture {
        "keyword" => Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
        "string" => Style::default().fg(Color::Green),
        "number" => Style::default().fg(Color::LightBlue),
        "comment" => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
        "function" => Style::default().fg(Color::Blue),
        "type" => Style::default().fg(Color::Yellow),
        "diagnostic.error" => Style::default()
            .fg(Color::Red)
            .add_modifier(Modifier::UNDERLINED),
        "diagnostic.warning" => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::UNDERLINED),
        _ => Style::default(),
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;
    use teal_workbench_core::{
        Document, InputEvent, Key, KeyModifiers, KeymapPreset, MouseButton, MouseEvent,
        MouseEventKind,
    };

    use super::EditorPanel;
    use super::super::{ClipboardAction, Widget};

    fn key(key: Key) -> InputEvent {
        InputEvent {
            key,
            modifiers: KeyModifiers::default(),
        }
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16, clicks: u8) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::default(),
            click_count: clicks,
        }
    }

    #[test]
    fn decorations_project_diagnostics_changes_and_search_into_gutter_and_ranges() {
        use super::GutterType;
        use crate::workbench::ui::DecorationKind;
        use teal_workbench_core::syntax::Capture;

        let mut panel = EditorPanel::new(
            Document::from_text("alpha\nbeta\ngamma\n"),
            KeymapPreset::VimFirst,
        );
        panel.set_diagnostic_captures(vec![Capture {
            name: "diagnostic".to_owned(),
            range: 6..10,
            priority: 1,
        }]);
        panel.set_search_captures(vec![Capture {
            name: "search".to_owned(),
            range: 11..16,
            priority: 1,
        }]);
        let first_line = 0..1;
        panel.mark_changed_lines(vec![first_line]);

        let decorations = panel.decorations();
        assert!(decorations.gutter(0).contains(&GutterType::Tool));
        assert!(decorations.gutter(1).contains(&GutterType::Warning));
        assert_eq!(decorations.ranges(1), &[(0, 4, DecorationKind::Diagnostic)]);
        assert_eq!(decorations.ranges(2), &[(0, 5, DecorationKind::Search)]);
    }

    #[test]
    fn vim_and_helix_modes_share_transactions_and_undo() {
        let mut panel = EditorPanel::new(Document::from_text("abc\n"), KeymapPreset::VimFirst);
        panel.handle_input(key(Key::Char('i')));
        panel.handle_input(key(Key::Char('界')));
        panel.handle_input(key(Key::Escape));
        assert_eq!(panel.document().to_string(), "\u{754c}abc\n");
        panel.handle_input(key(Key::Char('u')));
        assert_eq!(panel.document().to_string(), "abc\n");

        panel.set_keymap(KeymapPreset::HelixFirst);
        panel.handle_input(key(Key::Char('x')));
        panel.handle_input(key(Key::Char('d')));
        assert_ne!(panel.document().to_string(), "abc\n");
    }

    #[test]
    fn mouse_maps_tabs_and_wide_text_and_clipboard_uses_only_primary_selection() {
        let mut panel = EditorPanel::new(
            Document::from_text("a\t\u{754c}🙂 word\nsecond\n"),
            KeymapPreset::VimFirst,
        );
        let area = Rect::new(0, 0, 40, 8);
        panel.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 13, 1, 2),
            area,
        );
        assert_eq!(panel.document().selections().primary().range(), 5..9);
        let copy = panel.handle_clipboard(ClipboardAction::Copy, None);
        assert_eq!(copy.write.as_deref(), Some("word"));

        let cut = panel.handle_clipboard(ClipboardAction::Cut, None);
        assert_eq!(cut.write.as_deref(), Some("word"));
        assert_eq!(panel.document().to_string(), "a\t\u{754c}🙂 \nsecond\n");
        panel.undo();
        assert_eq!(panel.document().to_string(), "a\t\u{754c}🙂 word\nsecond\n");

        panel.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Middle), 8, 1, 1),
            area,
        );
        let _ = panel.handle_clipboard(ClipboardAction::Paste, Some("x\r\ny"));
        assert!(panel.document().to_string().contains("x\ny"));
    }

    #[test]
    fn backwards_drag_and_control_click_create_expected_selections() {
        let mut panel = EditorPanel::new(
            Document::from_text("alpha\nbeta\n"),
            KeymapPreset::HelixFirst,
        );
        let area = Rect::new(0, 0, 30, 8);
        panel.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 8, 2, 1),
            area,
        );
        panel.handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 5, 1, 0),
            area,
        );
        let selection = panel.document().selections().primary();
        assert!(selection.anchor > selection.head);

        let mut control = mouse(MouseEventKind::Down(MouseButton::Left), 4, 3, 1);
        control.modifiers.control = true;
        panel.handle_mouse(control, area);
        assert_eq!(panel.document().selections().selections().len(), 2);
    }
}
