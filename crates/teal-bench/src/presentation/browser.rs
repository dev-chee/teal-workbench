//! 带独立历史、链接提示和文本选择的网页面板。
//!
//! 面板本地状态保存视图与导航意图，网络获取及外部打开由根应用执行。

use super::{
    AppCommand, Block, Borders, ClipboardAction, ClipboardOutcome, Color, Frame, InputEvent, Key,
    Line, MenuItem, MouseButton, MouseEvent, MouseEventKind, PanelKind, PanelStatus, Paragraph,
    Rect, Style, TextViewState, WebDocument, Widget, WidgetAction, plain_lines,
    render_lines_with_selection,
};

/// 带数字链接提示的可滚动网页文档。
pub struct BrowserPanel {
    document: WebDocument,
    history: Vec<WebDocument>,
    history_index: usize,
    view: TextViewState,
    hint: String,
    hint_mode: bool,
}

impl BrowserPanel {
    /// 为一个已获取文档创建浏览器面板。
    pub fn new(document: WebDocument) -> Self {
        Self {
            history: vec![document.clone()],
            history_index: 0,
            document,
            view: TextViewState::default(),
            hint: String::new(),
            hint_mode: false,
        }
    }

    /// 导航后替换当前文档。
    pub fn replace(&mut self, document: WebDocument) {
        if self.history.len() == 1 && self.document.status == 0 {
            self.history[0] = document.clone();
        } else {
            self.history.truncate(self.history_index + 1);
            self.history.push(document.clone());
        }
        if self.history.len() > 32 {
            self.history.remove(0);
        }
        self.history_index = self.history.len() - 1;
        self.document = document;
        self.view = TextViewState::default();
        self.hint.clear();
        self.hint_mode = false;
    }

    /// 返回当前 URL。
    pub fn url(&self) -> &url::Url {
        &self.document.url
    }

    pub(crate) fn navigate(&mut self, forward: bool) {
        self.history_index = if forward {
            (self.history_index + 1).min(self.history.len() - 1)
        } else {
            self.history_index.saturating_sub(1)
        };
        self.document = self.history[self.history_index].clone();
        self.view = TextViewState::default();
    }

    pub(crate) fn find(&mut self, query: &str) -> bool {
        if let Some((line, _)) = self.document.find(query).first() {
            self.view.vertical = *line;
            true
        } else {
            false
        }
    }

    pub(crate) fn session_state(&self) -> teal_workbench_services::PanelSessionState {
        let mut browser = teal_host::web::BrowserSession {
            history: self.history.clone(),
            index: self.history_index,
        };
        browser.bound();
        teal_workbench_services::PanelSessionState::Browser {
            browser,
            scroll: self.view.vertical,
            column: self.view.horizontal,
        }
    }

    pub(crate) fn restore(
        browser: &teal_host::web::BrowserSession,
        scroll: usize,
        column: usize,
    ) -> Result<Self, String> {
        browser.validate()?;
        let mut panel = Self::new(browser.history[browser.index].clone());
        panel.history.clone_from(&browser.history);
        panel.history_index = browser.index;
        panel.view.vertical = scroll.min(panel.document.lines.len().saturating_sub(1));
        panel.view.horizontal = column;
        Ok(panel)
    }

    fn display_lines(&self) -> Vec<Line<'static>> {
        let mut lines = self
            .document
            .lines
            .iter()
            .cloned()
            .map(Line::from)
            .collect::<Vec<_>>();
        if !self.document.links.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::styled("Link", Style::default().fg(Color::Cyan)));
            lines.extend(
                self.document.links.iter().map(|link| {
                    Line::from(format!("[{}] {} → {}", link.hint, link.label, link.url))
                }),
            );
        }
        lines
    }

    fn link_at_line(&self, line: usize) -> Option<usize> {
        let first = self.document.lines.len().saturating_add(2);
        let index = line.checked_sub(first)?;
        (index < self.document.links.len()).then_some(index)
    }
}

impl Widget for BrowserPanel {
    fn kind(&self) -> PanelKind {
        PanelKind::Browser
    }

    fn title(&self) -> String {
        self.document.title.clone()
    }

    fn status(&self) -> Option<PanelStatus> {
        Some(PanelStatus {
            mode: if self.hint_mode {
                format!("Link hint: {}", self.hint)
            } else {
                "Browse".to_owned()
            },
            resource: self.document.url.to_string(),
            position: format!(
                "HTTP {}  Links: {}  {}/{}  Horizontal: {}",
                self.document.status,
                self.document.links.len(),
                self.view.vertical.saturating_add(1),
                self.display_lines().len(),
                self.view.horizontal,
            ),
        })
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        if self.hint_mode {
            return match event.key {
                Key::Escape => {
                    self.hint_mode = false;
                    self.hint.clear();
                    WidgetAction::Consumed
                }
                Key::Char(character) if character.is_ascii_digit() => {
                    self.hint.push(character);
                    WidgetAction::Consumed
                }
                Key::Enter => {
                    self.hint_mode = false;
                    let target = self
                        .hint
                        .parse::<usize>()
                        .ok()
                        .and_then(|hint| self.document.links.iter().find(|link| link.hint == hint))
                        .map(|link| link.url.to_string());
                    self.hint.clear();
                    target.map_or(WidgetAction::Consumed, |url| {
                        WidgetAction::Commands(vec![AppCommand::Browse(url)])
                    })
                }
                _ => WidgetAction::Consumed,
            };
        }
        match event.key {
            Key::Char('j') | Key::Down => {
                self.view.vertical =
                    (self.view.vertical + 1).min(self.display_lines().len().saturating_sub(1));
            }
            Key::Char('k') | Key::Up => {
                self.view.vertical = self.view.vertical.saturating_sub(1);
            }
            Key::Char('f') => self.hint_mode = true,
            Key::Char('H') => return WidgetAction::Commands(vec![AppCommand::BrowseBack]),
            Key::Char('L') => return WidgetAction::Commands(vec![AppCommand::BrowseForward]),
            Key::Char('o') => return WidgetAction::Commands(vec![AppCommand::BrowseExternal]),
            _ => return WidgetAction::Ignored,
        }
        WidgetAction::Consumed
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: Rect) -> WidgetAction {
        let rendered = self.display_lines();
        let lines = plain_lines(&rendered);
        let point = self.view.point_at(&lines, event, area, false);
        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some(index) = self.link_at_line(point.line)
            && let Some(link) = self.document.links.get(index)
        {
            return WidgetAction::Commands(vec![AppCommand::Browse(link.url.to_string())]);
        }
        let link = self.link_at_line(point.line);
        let action = self.view.handle_mouse(&lines, event, area);
        match (action, link) {
            (WidgetAction::Menu(mut menu), Some(index)) => {
                menu.items.splice(
                    0..0,
                    [
                        MenuItem::enabled(format!("browser.open_link.{index}"), "Open link"),
                        MenuItem::enabled(
                            format!("browser.external_link.{index}"),
                            "Open link externally",
                        ),
                        MenuItem::enabled(format!("browser.copy_link.{index}"), "Copy link"),
                    ],
                );
                WidgetAction::Menu(menu)
            }
            (action, _) => action,
        }
    }

    fn handle_clipboard(
        &mut self,
        action: ClipboardAction,
        _paste: Option<&str>,
    ) -> ClipboardOutcome {
        let lines = plain_lines(&self.display_lines());
        self.view.handle_clipboard(&lines, action)
    }

    fn handle_menu(&mut self, id: &str) -> WidgetAction {
        let action = id
            .rsplit_once('.')
            .and_then(|(prefix, index)| index.parse::<usize>().ok().map(|index| (prefix, index)));
        match action {
            Some(("browser.open_link", index)) => self
                .document
                .links
                .get(index)
                .map_or(WidgetAction::Ignored, |link| {
                    WidgetAction::Commands(vec![AppCommand::Browse(link.url.to_string())])
                }),
            Some(("browser.external_link", index)) => {
                self.document
                    .links
                    .get(index)
                    .map_or(WidgetAction::Ignored, |link| {
                        WidgetAction::Commands(vec![AppCommand::BrowseExternalUrl(
                            link.url.to_string(),
                        )])
                    })
            }
            Some(("browser.copy_link", index)) => self
                .document
                .links
                .get(index)
                .map_or(WidgetAction::Ignored, |link| {
                    WidgetAction::WriteClipboard(link.url.to_string())
                }),
            _ => WidgetAction::Ignored,
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let lines = render_lines_with_selection(&self.display_lines(), self.view.selection);
        let block = Block::default()
            .title(self.document.title.clone())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused {
                Color::Cyan
            } else {
                Color::DarkGray
            }));
        frame.render_widget(
            Paragraph::new(lines)
                .scroll((
                    u16::try_from(self.view.vertical).unwrap_or(u16::MAX),
                    u16::try_from(self.view.horizontal).unwrap_or(u16::MAX),
                ))
                .block(block),
            area,
        );
    }
}
