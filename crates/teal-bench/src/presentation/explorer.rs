//! 惰性文件树与同步预览的面板状态。
//!
//! 模型加载和预览读取独立完成，结果必须匹配当前光标路径；等待期间继续显示可用模型。
//! 树根表示浏览位置，创建和相对资源操作使用光标上下文目录。

use super::{
    AppCommand, Block, Borders, Color, Constraint, ContextMenu, ExplorerSession, FileExplorer,
    Frame, InputEvent, Key, Layout, LayoutDirection, Line, List, ListItem, MenuItem, MouseButton,
    MouseEvent, MouseEventKind, Widget, WidgetAction, PanelKind, PanelStatus, Paragraph, Preview,
    Rect, Style, WorkspaceIntent, rect_contains,
};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
};

type ModelResult = Result<(FileExplorer, Option<PathBuf>), String>;

/// 带同步预览与地址提示框的惰性文件树。
pub struct ExplorerPanel {
    pub(super) explorer: FileExplorer,
    address_active: bool,
    address: String,
    pub(super) scroll: usize,
    pub(super) tree_area: Rect,
    model_job: Option<Receiver<ModelResult>>,
    preview_job: Option<Receiver<(PathBuf, Vec<Line<'static>>)>>,
    preview_path: Option<PathBuf>,
    preview: Vec<Line<'static>>,
    pending_action: Option<WidgetAction>,
    error: Option<String>,
}

impl ExplorerPanel {
    pub(crate) const fn address_active(&self) -> bool {
        self.address_active
    }

    /// 打开目录，并可选择聚焦其地址栏。
    pub fn new(
        path: &std::path::Path,
        address_active: bool,
    ) -> Result<Self, teal_workbench_services::ExplorerError> {
        let explorer = FileExplorer::open(path)?;
        Ok(Self::from_model(explorer, address_active))
    }

    pub(crate) fn from_model(explorer: FileExplorer, address_active: bool) -> Self {
        Self {
            address: explorer.root().to_string_lossy().into_owned(),
            explorer,
            address_active,
            scroll: 0,
            tree_area: Rect::default(),
            model_job: None,
            preview_job: None,
            preview_path: None,
            preview: Vec::new(),
            pending_action: None,
            error: None,
        }
    }

    /// 恢复类型化文件树会话。
    pub fn restore(
        state: &ExplorerSession,
        address_active: bool,
    ) -> Result<Self, teal_workbench_services::ExplorerError> {
        let explorer = FileExplorer::restore(
            &state.root,
            &state.expanded,
            state.cursor.as_deref(),
            state.show_hidden,
            state.sort,
        )?;
        let mut panel = Self::from_model(explorer, address_active);
        panel.scroll = state.scroll;
        panel.clamp_scroll();
        Ok(panel)
    }

    /// 返回当前树根。
    pub fn root(&self) -> &std::path::Path {
        self.explorer.root()
    }

    /// 返回用于创建和相对路径的光标上下文目录。
    pub fn operation_directory(&self) -> &std::path::Path {
        self.explorer.operation_directory()
    }

    /// 捕获类型化持久树状态。
    pub fn session_state(&self) -> ExplorerSession {
        ExplorerSession {
            root: self.root().to_path_buf(),
            expanded: self.explorer.expanded_paths(),
            cursor: self.explorer.hovered().map(|row| row.entry.path.clone()),
            scroll: self.scroll,
            show_hidden: self.explorer.show_hidden(),
            sort: self.explorer.sort(),
        }
    }

    /// 返回选中路径；没有显式选择时返回光标路径。
    pub fn selected_paths(&self) -> Vec<std::path::PathBuf> {
        self.explorer.selected_or_hovered()
    }

    /// 返回第一个选中或悬停的文件名，供重命名提示使用。
    pub fn selected_name(&self) -> Option<String> {
        self.explorer
            .selected_or_hovered()
            .first()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
    }

    /// 在当前目录创建一个文件。
    pub(crate) fn create_file(
        &mut self,
        name: &str,
    ) -> Result<WorkspaceIntent, teal_workbench_services::ExplorerError> {
        self.ready()?;
        checked_name(name)?;
        Ok(WorkspaceIntent::Create {
            path: self.operation_directory().join(name),
            directory: false,
        })
    }

    /// 在当前目录创建一个目录。
    pub(crate) fn create_directory(
        &mut self,
        name: &str,
    ) -> Result<WorkspaceIntent, teal_workbench_services::ExplorerError> {
        self.ready()?;
        checked_name(name)?;
        Ok(WorkspaceIntent::Create {
            path: self.operation_directory().join(name),
            directory: true,
        })
    }

    /// 重命名单个选中或悬停的条目。
    pub(crate) fn rename_selected(
        &mut self,
        name: &str,
    ) -> Result<WorkspaceIntent, teal_workbench_services::ExplorerError> {
        self.ready()?;
        checked_name(name)?;
        let paths = self.explorer.selected_or_hovered();
        let path = paths
            .first()
            .ok_or(teal_workbench_services::ExplorerError::InvalidName)?
            .clone();
        if path == self.root() {
            return Err(teal_workbench_services::ExplorerError::RootMutation);
        }
        let new = path
            .parent()
            .ok_or(teal_workbench_services::ExplorerError::InvalidName)?
            .join(name);
        Ok(WorkspaceIntent::Rename { old: path, new })
    }

    /// 复制选中的条目。
    pub(crate) fn copy_selected(
        &mut self,
        destination: &std::path::Path,
    ) -> Result<WorkspaceIntent, teal_workbench_services::ExplorerError> {
        self.ready()?;
        let paths = self.explorer.selected_or_hovered();
        Ok(WorkspaceIntent::Copy {
            paths,
            destination: destination.to_owned(),
        })
    }

    /// 移动选中的条目。
    pub(crate) fn move_selected(
        &mut self,
        destination: &std::path::Path,
    ) -> Result<WorkspaceIntent, teal_workbench_services::ExplorerError> {
        self.ready()?;
        let paths = self.explorer.selected_or_hovered();
        if paths.iter().any(|path| path == self.root()) {
            return Err(teal_workbench_services::ExplorerError::RootMutation);
        }
        Ok(WorkspaceIntent::Move {
            paths,
            destination: destination.to_owned(),
        })
    }

    /// 将选中条目移入回收站。
    pub(crate) fn trash_selected(
        &mut self,
    ) -> Result<WorkspaceIntent, teal_workbench_services::ExplorerError> {
        self.ready()?;
        let paths = self.explorer.selected_or_hovered();
        if paths.iter().any(|path| path == self.root()) {
            return Err(teal_workbench_services::ExplorerError::RootMutation);
        }
        Ok(WorkspaceIntent::Remove {
            paths,
            permanent: false,
        })
    }

    /// 调用方确认后永久删除选中条目。
    pub(crate) fn delete_selected(
        &mut self,
    ) -> Result<WorkspaceIntent, teal_workbench_services::ExplorerError> {
        self.ready()?;
        let paths = self.explorer.selected_or_hovered();
        if paths.iter().any(|path| path == self.root()) {
            return Err(teal_workbench_services::ExplorerError::RootMutation);
        }
        Ok(WorkspaceIntent::Remove {
            paths,
            permanent: true,
        })
    }

    fn ready(&self) -> Result<(), teal_workbench_services::ExplorerError> {
        if self.model_job.is_some() {
            Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "File tree is loading").into())
        } else {
            Ok(())
        }
    }

    pub(crate) fn is_loading(&self) -> bool {
        self.model_job.is_some()
    }

    fn schedule(
        &mut self,
        work: impl FnOnce(
            &mut FileExplorer,
        ) -> Result<Option<PathBuf>, teal_workbench_services::ExplorerError>
        + Send
        + 'static,
    ) {
        if self.model_job.is_some() {
            return;
        }

        // 工作线程更新拥有所有权的副本时，保留当前显示模型可用。
        let mut model = self.explorer.clone();
        let (output, input) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("teal-bench-explorer".into())
            .spawn(move || {
                let result = work(&mut model)
                    .map(|path| (model, path))
                    .map_err(|e| e.to_string());
                let _ = output.send(result);
            }) {
            Ok(_) => {
                self.model_job = Some(input);
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn poll_jobs(&mut self) {
        let result = self
            .model_job
            .as_ref()
            .and_then(|job| match job.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Disconnected) => {
                    Some(Err("File tree worker thread stopped".into()))
                }
                Err(TryRecvError::Empty) => None,
            });
        if let Some(result) = result {
            self.model_job = None;
            match result {
                Ok((model, path)) => {
                    self.explorer = model;
                    self.address = self.root().to_string_lossy().into_owned();
                    self.preview_path = None;
                    self.ensure_cursor_visible();
                    if let Some(path) = path {
                        self.pending_action =
                            Some(WidgetAction::Commands(vec![AppCommand::Open(path)]));
                    }
                }
                Err(error) => self.error = Some(error),
            }
        }

        if let Some(job) = &self.preview_job {
            match job.try_recv() {
                Ok((path, lines)) => {
                    self.preview_job = None;

                    // 导航可能快于预览读取；丢弃旧光标路径的结果。
                    if self
                        .explorer
                        .hovered()
                        .is_some_and(|row| row.entry.path == path)
                    {
                        self.preview_path = Some(path);
                        self.preview = lines;
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.preview_job = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
    }

    fn request_preview(&mut self) {
        let Some(path) = self.explorer.hovered().map(|row| row.entry.path.clone()) else {
            return;
        };
        if self.preview_path.as_ref() == Some(&path) || self.preview_job.is_some() {
            return;
        }

        self.preview = vec![Line::from("Loading preview…")];
        let (output, input) = mpsc::sync_channel(1);
        let spawn = std::thread::Builder::new()
            .name("teal-bench-preview".into())
            .spawn(move || {
                let preview = FileExplorer::preview(&path, 512 * 1024)
                    .unwrap_or_else(|error| Preview::Unavailable(error.to_string()));
                let _ = output.send((path, preview_lines(preview)));
            });
        match spawn {
            Ok(_) => self.preview_job = Some(input),
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub(crate) fn refresh_after(&mut self, intent: &WorkspaceIntent) {
        let intent = intent.clone();
        self.schedule(move |model| {
            model.refresh()?;
            match intent {
                WorkspaceIntent::Create { path, .. } => {
                    model.reveal(&path)?;
                }
                WorkspaceIntent::Rename { new, .. } => {
                    model.reveal(&new)?;
                }
                _ => {}
            }
            Ok(None)
        });
    }

    #[cfg(test)]
    pub(crate) fn settle(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while self.model_job.is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "File browser worker thread timed out"
            );
            self.poll_jobs();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn ensure_cursor_visible(&mut self) {
        let height = usize::from(self.tree_area.height);
        let cursor = self.explorer.cursor();
        if cursor < self.scroll {
            self.scroll = cursor;
        } else if height > 0 && cursor >= self.scroll + height {
            self.scroll = cursor + 1 - height;
        }
    }

    fn clamp_scroll(&mut self) {
        self.scroll = self
            .scroll
            .min(self.explorer.visible_entries().len().saturating_sub(1));
    }

    fn open_hovered_action(&mut self) -> WidgetAction {
        if self.explorer.hovered().is_some_and(|row| row.entry.is_dir) {
            self.schedule(FileExplorer::activate_hovered);
            return WidgetAction::Consumed;
        }
        match self.explorer.activate_hovered() {
            Ok(Some(path)) => WidgetAction::Commands(vec![AppCommand::Open(path)]),
            Ok(None) => {
                self.ensure_cursor_visible();
                WidgetAction::Consumed
            }
            Err(_) => WidgetAction::Ignored,
        }
    }

    fn navigate_right_action(&mut self) -> WidgetAction {
        if self
            .explorer
            .hovered()
            .is_some_and(|row| row.entry.is_dir && !row.expanded)
        {
            self.schedule(FileExplorer::navigate_right);
            return WidgetAction::Consumed;
        }
        match self.explorer.navigate_right() {
            Ok(Some(path)) => WidgetAction::Commands(vec![AppCommand::Open(path)]),
            Ok(None) => {
                self.ensure_cursor_visible();
                WidgetAction::Consumed
            }
            Err(_) => WidgetAction::Ignored,
        }
    }

    fn disclosure_column(&self, index: usize) -> Option<u16> {
        self.explorer.visible_entries().get(index).map(|row| {
            self.tree_area
                .x
                .saturating_add(2)
                .saturating_add(u16::try_from(row.depth.saturating_mul(2)).unwrap_or(u16::MAX))
        })
    }
}

impl Widget for ExplorerPanel {
    fn poll_action(&mut self) -> WidgetAction {
        self.poll_jobs();
        self.pending_action.take().unwrap_or(WidgetAction::Ignored)
    }

    fn kind(&self) -> PanelKind {
        PanelKind::Explorer
    }

    fn title(&self) -> String {
        "Files".to_owned()
    }

    fn status(&self) -> Option<PanelStatus> {
        Some(PanelStatus {
            mode: if self.is_loading() {
                "loading"
            } else if self.address_active {
                "address"
            } else {
                "normal"
            }
            .to_owned(),
            resource: self.explorer.root().to_string_lossy().into_owned(),
            position: format!("{} items visible", self.explorer.visible_entries().len()),
        })
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        self.poll_jobs();
        if self.is_loading() {
            return WidgetAction::Consumed;
        }
        if self.address_active {
            return match event.key {
                Key::Escape => {
                    self.address_active = false;
                    WidgetAction::Consumed
                }
                Key::Enter => {
                    self.address_active = false;
                    let path = std::path::PathBuf::from(&self.address);
                    self.schedule(move |model| {
                        if path.is_dir() {
                            *model = FileExplorer::open(&path)?;
                            Ok(None)
                        } else {
                            Ok(Some(path))
                        }
                    });
                    WidgetAction::Consumed
                }
                Key::Backspace => {
                    self.address.pop();
                    WidgetAction::Consumed
                }
                Key::Char(character) if !event.modifiers.control && !event.modifiers.alt => {
                    self.address.push(character);
                    WidgetAction::Consumed
                }
                _ => WidgetAction::Consumed,
            };
        }
        match event.key {
            Key::Char('j') | Key::Down => {
                self.explorer.move_cursor(1);
                self.ensure_cursor_visible();
            }
            Key::Char('k') | Key::Up => {
                self.explorer.move_cursor(-1);
                self.ensure_cursor_visible();
            }
            Key::Char('h') | Key::Left => {
                if self
                    .explorer
                    .hovered()
                    .is_some_and(|row| row.depth == 0 && !row.expanded)
                {
                    self.schedule(|model| model.navigate_left().map(|_| None));
                } else {
                    let _ = self.explorer.navigate_left();
                    self.ensure_cursor_visible();
                }
            }
            Key::Char('l') | Key::Right => return self.navigate_right_action(),
            Key::Enter => return self.open_hovered_action(),
            Key::Char(' ') => self.explorer.toggle_selection(),
            Key::Char('.') => {
                self.schedule(|model| model.set_show_hidden(!model.show_hidden()).map(|()| None));
            }
            Key::Char('g') => {
                self.explorer.move_cursor(isize::MIN);
                self.ensure_cursor_visible();
            }
            Key::Char('G') => {
                self.explorer.move_cursor(isize::MAX);
                self.ensure_cursor_visible();
            }
            Key::Char('a') => {
                self.address = self.explorer.root().to_string_lossy().into_owned();
                self.address_active = true;
            }
            Key::Char('r') => {
                self.schedule(|model| model.refresh().map(|()| None));
            }
            _ => return WidgetAction::Ignored,
        }
        WidgetAction::Consumed
    }

    fn handle_mouse(&mut self, event: MouseEvent, _area: Rect) -> WidgetAction {
        self.poll_jobs();
        if self.is_loading() {
            return WidgetAction::Consumed;
        }
        match event.kind {
            MouseEventKind::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(3);
                WidgetAction::Consumed
            }
            MouseEventKind::ScrollDown => {
                self.scroll =
                    (self.scroll + 3).min(self.explorer.visible_entries().len().saturating_sub(1));
                WidgetAction::Consumed
            }
            MouseEventKind::Down(MouseButton::Left | MouseButton::Right)
                if rect_contains(self.tree_area, event.column, event.row) =>
            {
                let index = self
                    .scroll
                    .saturating_add(usize::from(event.row.saturating_sub(self.tree_area.y)));
                if index >= self.explorer.visible_entries().len() {
                    return WidgetAction::Consumed;
                }
                if matches!(event.kind, MouseEventKind::Down(MouseButton::Right)) {
                    if self.explorer.is_selected(index) {
                        self.explorer.set_cursor(index);
                    } else {
                        self.explorer.select_only(index);
                    }
                    return WidgetAction::Menu(ContextMenu {
                        x: event.column,
                        y: event.row,
                        items: vec![
                            MenuItem::enabled("explorer.open", "Open"),
                            MenuItem::enabled("explorer.toggle", "Toggle selection"),
                            MenuItem::enabled("explorer.copy_path", "Copy path"),
                            MenuItem::enabled("explorer.rename", "Rename…"),
                            MenuItem::enabled("explorer.copy_to", "Copy to…"),
                            MenuItem::enabled("explorer.move_to", "Move to…"),
                            MenuItem::enabled("explorer.trash", "Move to recycle bin"),
                            MenuItem::enabled("explorer.delete", "Delete permanently"),
                            MenuItem::enabled("explorer.refresh", "Refresh"),
                        ],
                    });
                }
                if event.modifiers.control {
                    self.explorer.toggle_at(index);
                } else if event.modifiers.shift {
                    self.explorer.select_range(index);
                } else {
                    self.explorer.select_only(index);
                }
                self.ensure_cursor_visible();
                if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
                    && self.disclosure_column(index) == Some(event.column)
                    && self.explorer.visible_entries()[index].entry.is_dir
                {
                    self.schedule(move |model| model.toggle_expansion_at(index).map(|()| None));
                    self.ensure_cursor_visible();
                    return WidgetAction::Consumed;
                }
                if event.click_count >= 2 {
                    self.open_hovered_action()
                } else {
                    WidgetAction::Consumed
                }
            }
            _ => WidgetAction::Ignored,
        }
    }

    fn handle_menu(&mut self, id: &str) -> WidgetAction {
        self.poll_jobs();
        if self.is_loading() {
            return WidgetAction::Consumed;
        }
        match id {
            "explorer.open" => self.open_hovered_action(),
            "explorer.toggle" => {
                self.explorer.toggle_selection();
                WidgetAction::Consumed
            }
            "explorer.copy_path" => WidgetAction::WriteClipboard(
                self.explorer
                    .selected_or_hovered()
                    .iter()
                    .map(|path| path.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            "explorer.refresh" => {
                self.schedule(|model| model.refresh().map(|()| None));
                self.ensure_cursor_visible();
                WidgetAction::Consumed
            }
            _ => WidgetAction::Ignored,
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        self.poll_jobs();
        self.request_preview();
        let block = Block::default()
            .title(if self.address_active {
                format!("Files  {}", self.address)
            } else {
                "Files".to_owned()
            })
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused {
                Color::Cyan
            } else {
                Color::DarkGray
            }));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let columns = Layout::default()
            .direction(LayoutDirection::Horizontal)
            .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
            .split(inner);
        let tree_block = Block::default()
            .title("Directory tree")
            .borders(Borders::TOP | Borders::RIGHT);
        self.tree_area = tree_block.inner(columns[0]);
        self.clamp_scroll();

        let hovered = self.explorer.hovered().map(|row| row.entry.path.clone());
        let tree_items = self
            .explorer
            .visible_entries()
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(usize::from(self.tree_area.height))
            .map(|(index, row)| {
                let marker = if hovered.as_ref() == Some(&row.entry.path) {
                    ">"
                } else {
                    " "
                };
                let disclosure = if row.entry.is_dir {
                    if row.expanded { "▾" } else { "▸" }
                } else {
                    " "
                };
                let indent = "  ".repeat(row.depth);
                let suffix = if row.entry.is_dir { "/" } else { "" };
                ListItem::new(format!(
                    "{marker} {indent}{disclosure} {}{suffix}",
                    row.entry.name
                ))
                .style(if self.explorer.is_selected(index) {
                    Style::default().fg(Color::White).bg(Color::Blue)
                } else {
                    Style::default()
                })
            });
        frame.render_widget(List::new(tree_items).block(tree_block), columns[0]);

        let preview = self.error.as_ref().map_or_else(
            || self.preview.clone(),
            |error| vec![Line::from(error.clone())],
        );
        frame.render_widget(
            Paragraph::new(preview).block(Block::default().title("Preview")),
            columns[1],
        );
    }

    fn cursor(&self, _area: Rect) -> Option<(u16, u16)> {
        self.address_active.then_some((
            7_u16.saturating_add(u16::try_from(self.address.chars().count()).unwrap_or(u16::MAX)),
            0,
        ))
    }
}

fn preview_lines(preview: Preview) -> Vec<Line<'static>> {
    match preview {
        Preview::Text { content, .. } | Preview::Markdown(content) | Preview::Binary(content) => {
            content
                .lines()
                .map(|line| Line::from(line.to_owned()))
                .collect()
        }
        Preview::Directory(entries) => entries.into_iter().map(Line::from).collect(),
        Preview::Unavailable(reason) => vec![Line::from(reason)],
    }
}

fn checked_name(name: &str) -> Result<(), teal_workbench_services::ExplorerError> {
    let mut components = std::path::Path::new(name).components();
    if matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
        && !name.contains(['/', '\\'])
    {
        Ok(())
    } else {
        Err(teal_workbench_services::ExplorerError::InvalidName)
    }
}
