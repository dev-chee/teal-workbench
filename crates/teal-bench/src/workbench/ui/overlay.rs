//! 按栈优先处理的确认、选择器与上下文菜单。
//!
//! 浮层消费输入时，下层面板不能同时收到同一事件；鼠标只响应实际绘制的有效区域。
//! 中文按钮的命中宽度来自终端显示宽度，与 UTF-8 字节长度无关。

use std::time::{Duration, Instant};

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};
use teal_workbench_core::{
    InputEvent, Key, LayoutTree, MouseButton, MouseEvent, MouseEventKind, PanelId,
    Rect as CoreRect, Window, WindowChrome, WindowId, WindowInteraction, WindowKind,
    WindowPlacement, WindowSet,
};
use unicode_width::UnicodeWidthStr as _;

use super::{ContextMenu, MenuItem, Widget};

const YES_BUTTON: &str = "[ Yes(y) ]";
const NO_BUTTON: &str = "[ No(n) ]";
const ONCE_BUTTON: &str = "[ Once(o) ]";
const TASK_BUTTON: &str = "[ Task(t) ]";
const WORKSPACE_BUTTON: &str = "[ Workspace(w) ]";
const SECRET_REVEAL_DURATION: Duration = Duration::from_secs(1);

fn confirmation_buttons(popup: Rect) -> (Rect, Rect) {
    // 与实际渲染的边框内起点、标签显示宽度和两个空格间隔保持一致。
    let yes = Rect::new(
        popup.x.saturating_add(1),
        popup.y.saturating_add(3),
        u16::try_from(YES_BUTTON.width()).expect("fixed button width must fit u16"),
        1,
    );
    let no = Rect::new(
        yes.right().saturating_add(2),
        yes.y,
        u16::try_from(NO_BUTTON.width()).expect("fixed button width must fit u16"),
        1,
    );
    (yes.intersection(popup), no.intersection(popup))
}

/// 浮层输入结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlayAction {
    /// 浮层消费输入并保持打开。
    Consumed,

    /// 浮层接受了一个值。
    Accepted(String),

    /// 浮层接受了不得出现在 Debug 输出中的敏感值。
    SensitiveAccepted(SensitiveValue),

    /// 浮层未产生值即关闭。
    Cancelled,

    /// 事件可以冒泡到较低层。
    Ignored,
}

/// 不在调试输出中暴露内容的短期敏感输入。
#[derive(Clone, PartialEq, Eq)]
pub struct SensitiveValue(String);

impl SensitiveValue {
    /// 消耗包装并把内容交给唯一的认证接收方。
    #[must_use]
    pub fn expose(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for SensitiveValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SensitiveValue(<hidden>)")
    }
}

/// 渲染在拆分面板之上的模态层。
pub trait Overlay {
    /// 在任何面板接收输入前路由输入。
    fn handle_input(&mut self, event: InputEvent) -> OverlayAction;

    /// 在任何较低层接收前路由鼠标输入。
    fn handle_mouse(&mut self, _event: MouseEvent, _area: Rect) -> OverlayAction {
        OverlayAction::Ignored
    }

    /// 在完整终端区域内渲染。
    fn render(&mut self, frame: &mut Frame<'_>, area: Rect);

    /// 返回浮层的首选尺寸：宽度占终端宽度的百分比与固定高度（含边框）。
    ///
    /// 该尺寸同时用于绘制与窗口登记，二者必须保持一致。
    fn preferred_size(&self) -> (u16, u16);

    /// 返回浮层的窗口描述：标题、尺寸与尺寸类型。
    fn window(&self) -> OverlayWindow {
        let (width, height) = self.preferred_size();
        OverlayWindow {
            title: "Popup".to_owned(),
            size: (width, height),
            absolute_width: false,
        }
    }
}

/// 浮层窗口登记所需的描述：标题与尺寸。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlayWindow {
    /// 稳定标题，用于窗口标题与登记查询。
    pub title: String,

    /// 首选尺寸：宽度百分比（`Picker`/`ContextMenu` 为绝对宽度）与固定高度。
    pub size: (u16, u16),

    /// 宽度语义：`true` 表示绝对值，`false` 表示终端宽度百分比。
    pub absolute_width: bool,
}

/// 后进先出的浮层合成器。
///
/// 每个浮层同时登记为一个瞬态浮动窗口：它不进入标签条与会话，关闭浮层时同步移除。
/// 窗口几何由浮层自己的首选尺寸在内容区内居中计算，与该浮层实际绘制位置一致。
#[derive(Default)]
pub struct OverlayStack {
    overlays: Vec<Box<dyn Overlay>>,
    descriptors: Vec<OverlayWindow>,
    windows: Vec<WindowId>,
}

impl OverlayStack {
    /// 压入最前方浮层，并记录其窗口描述。
    ///
    /// 窗口本身由 [`OverlayStack::render`] 依内容区登记，因此调用方无需提供几何。
    pub fn push(&mut self, overlay: impl Overlay + 'static) {
        self.descriptors.push(overlay.window());
        self.overlays.push(Box::new(overlay));
    }

    /// 将输入路由到最前方浮层。
    pub fn handle_input(&mut self, event: InputEvent) -> OverlayAction {
        let Some(overlay) = self.overlays.last_mut() else {
            return OverlayAction::Ignored;
        };
        let action = overlay.handle_input(event);
        if matches!(
            action,
            OverlayAction::Accepted(_)
                | OverlayAction::SensitiveAccepted(_)
                | OverlayAction::Cancelled
        ) {
            self.retire_top();
        }
        action
    }

    /// 将鼠标事件路由到最前方浮层。
    pub fn handle_mouse(&mut self, event: MouseEvent, area: Rect) -> OverlayAction {
        let Some(overlay) = self.overlays.last_mut() else {
            return OverlayAction::Ignored;
        };
        let action = overlay.handle_mouse(event, area);
        if matches!(
            action,
            OverlayAction::Accepted(_)
                | OverlayAction::SensitiveAccepted(_)
                | OverlayAction::Cancelled
        ) {
            self.retire_top();
        }
        action
    }

    /// 移除最前方浮层，并丢弃它的窗口登记。
    ///
    /// 登记窗口本体由下一次 [`OverlayStack::sync_windows`] 对账时从窗口集合移除。
    fn retire_top(&mut self) {
        self.overlays.pop();
        self.descriptors.pop();
        self.windows.pop();
    }

    /// 从后向前渲染浮层。
    ///
    /// 窗口登记由 [`OverlayStack::sync_windows`] 单独负责，二者使用同一套首选尺寸。
    pub fn render(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        for overlay in &mut self.overlays {
            overlay.render(frame, area);
        }
    }

    /// 用渲染结果对账窗口集合：登记新浮层窗口、移除已关闭浮层窗口。
    pub fn sync_windows(&mut self, windows: &mut WindowSet, area: CoreRect) {
        // 关闭多余的窗口（浮层被弹出或窗口被外部关闭）。
        while self.windows.len() > self.overlays.len() {
            if let Some(id) = self.windows.pop() {
                let _ = windows.close_passive(id);
            }
        }
        for index in 0..self.descriptors.len() {
            let Some(descriptor) = self.descriptors.get(index).cloned() else {
                break;
            };
            let rect = geometry(&descriptor, area);
            match self.windows.get(index).copied() {
                Some(id) if windows.get(id).is_some() => {
                    windows.set_floating_rect(id, rect);
                }
                _ => {
                    let mut window = popup_window(&descriptor);
                    window.set_placement(WindowPlacement::Floating { rect, z: 1 });
                    let id = windows.insert_passive(window);
                    if index < self.windows.len() {
                        self.windows[index] = id;
                    } else {
                        self.windows.push(id);
                    }
                }
            }
        }
    }

    /// 返回模态层是否处于活动状态。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.overlays.is_empty()
    }

    /// 返回当前浮层窗口标识，按栈顺序由底到顶排列。
    #[must_use]
    pub fn window_ids(&self) -> &[WindowId] {
        &self.windows
    }
}

/// 按浮层描述与内容区计算登记矩形。
///
/// 尺寸语义与浮层绘制保持一致：百分比宽度按内容区宽度换算，绝对宽度直接使用。
fn geometry(descriptor: &OverlayWindow, area: CoreRect) -> CoreRect {
    let (width_spec, height) = descriptor.size;
    let width = if descriptor.absolute_width {
        width_spec.min(area.width).max(1)
    } else {
        area.width
            .saturating_mul(width_spec)
            .saturating_div(100)
            .max(10)
            .min(area.width)
    };
    let height = height.max(1).min(area.height);
    CoreRect {
        x: area.x.saturating_add(area.width.saturating_sub(width) / 2),
        y: area
            .y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    }
}

/// 把一个浮层描述转换为瞬态浮动窗口。
fn popup_window(descriptor: &OverlayWindow) -> Window {
    let mut window = Window::new(
        WindowKind::Editor,
        descriptor.title.clone(),
        LayoutTree::new(PanelId::default()),
    );
    window.set_interaction(WindowInteraction::Passive);
    window.set_chrome(WindowChrome::None);
    window
}

/// 把 TUI 矩形转换为工作台核心矩形。
fn to_core(area: Rect) -> CoreRect {
    CoreRect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height,
    }
}

/// 是/否确认浮层。
pub struct ConfirmOverlay {
    title: String,
    message: String,
}

impl ConfirmOverlay {
    /// 创建确认提示。
    #[must_use]
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
        }
    }
}

impl Overlay for ConfirmOverlay {
    fn preferred_size(&self) -> (u16, u16) {
        (60, 5)
    }

    fn window(&self) -> OverlayWindow {
        OverlayWindow {
            title: self.title.clone(),
            size: (60, 5),
            absolute_width: false,
        }
    }

    fn handle_input(&mut self, event: InputEvent) -> OverlayAction {
        match event.key {
            Key::Char('y' | 'Y') | Key::Enter => OverlayAction::Accepted("yes".to_owned()),
            Key::Char('n' | 'N') | Key::Escape => OverlayAction::Cancelled,
            _ => OverlayAction::Consumed,
        }
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: Rect) -> OverlayAction {
        let popup = centered(area, 60, 5);
        if !matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            return OverlayAction::Consumed;
        }
        let (yes, no) = confirmation_buttons(popup);
        if event.column >= yes.x
            && event.column < yes.right()
            && event.row >= yes.y
            && event.row < yes.bottom()
        {
            OverlayAction::Accepted("yes".to_owned())
        } else if event.column >= no.x
            && event.column < no.right()
            && event.row >= no.y
            && event.row < no.bottom()
        {
            OverlayAction::Cancelled
        } else {
            OverlayAction::Consumed
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let popup = centered(area, 60, 5);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(format!("{}\n\n{YES_BUTTON}  {NO_BUTTON}", self.message)).block(
                Block::default()
                    .title(self.title.as_str())
                    .borders(Borders::ALL),
            ),
            popup,
        );
    }
}

/// 允许用户为权限请求选择单次、任务或工作区复用范围的确认浮层。
pub struct PermissionOverlay {
    title: String,
    message: String,
    allow_task: bool,
    allow_workspace: bool,
}

impl PermissionOverlay {
    /// 创建权限范围选择器；不适用的较大范围不会响应快捷键。
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        message: impl Into<String>,
        allow_task: bool,
        allow_workspace: bool,
    ) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            allow_task,
            allow_workspace,
        }
    }

    fn buttons(&self) -> String {
        let mut buttons = vec![ONCE_BUTTON];
        if self.allow_task {
            buttons.push(TASK_BUTTON);
        }
        if self.allow_workspace {
            buttons.push(WORKSPACE_BUTTON);
        }
        buttons.push(NO_BUTTON);
        buttons.join("  ")
    }
}

impl Overlay for PermissionOverlay {
    fn preferred_size(&self) -> (u16, u16) {
        (78, 9)
    }

    fn window(&self) -> OverlayWindow {
        OverlayWindow {
            title: self.title.clone(),
            size: (78, 9),
            absolute_width: false,
        }
    }

    fn handle_input(&mut self, event: InputEvent) -> OverlayAction {
        match event.key {
            Key::Enter if self.allow_task => OverlayAction::Accepted("task".to_owned()),
            Key::Char('o' | 'O') | Key::Enter => OverlayAction::Accepted("once".to_owned()),
            Key::Char('t' | 'T') if self.allow_task => OverlayAction::Accepted("task".to_owned()),
            Key::Char('w' | 'W') if self.allow_workspace => {
                OverlayAction::Accepted("workspace".to_owned())
            }
            Key::Char('n' | 'N') | Key::Escape => OverlayAction::Cancelled,
            _ => OverlayAction::Consumed,
        }
    }

    fn handle_mouse(&mut self, _event: MouseEvent, _area: Rect) -> OverlayAction {
        // 权限范围必须由明确快捷键选择；鼠标事件不能落到浮层后的工作区。
        OverlayAction::Consumed
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let popup = centered(area, 78, 9);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(format!("{}\n\n{}", self.message, self.buttons())).block(
                Block::default()
                    .title(self.title.as_str())
                    .borders(Borders::ALL),
            ),
            popup,
        );
    }
}

/// 新输入字符短暂显示后转为掩码且不把正文写入普通调试值的秘密输入浮层。
pub struct SecretInputOverlay {
    title: String,
    label: String,
    value: String,
    visible_until: Vec<Instant>,
}

impl SecretInputOverlay {
    /// 创建空的秘密输入框。
    #[must_use]
    pub fn new(title: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            label: label.into(),
            value: String::new(),
            visible_until: Vec::new(),
        }
    }
}

impl Overlay for SecretInputOverlay {
    fn preferred_size(&self) -> (u16, u16) {
        (60, 5)
    }

    fn window(&self) -> OverlayWindow {
        OverlayWindow {
            title: self.title.clone(),
            size: (60, 5),
            absolute_width: false,
        }
    }

    fn handle_input(&mut self, event: InputEvent) -> OverlayAction {
        match event.key {
            Key::Enter if !self.value.is_empty() => {
                self.visible_until.clear();
                OverlayAction::SensitiveAccepted(SensitiveValue(std::mem::take(&mut self.value)))
            }
            Key::Escape => {
                self.value.clear();
                self.visible_until.clear();
                OverlayAction::Cancelled
            }
            Key::Backspace => {
                self.value.pop();
                self.visible_until.pop();
                OverlayAction::Consumed
            }
            Key::Char(character) if !event.modifiers.control && !event.modifiers.alt => {
                self.value.push(character);
                self.visible_until
                    .push(Instant::now() + SECRET_REVEAL_DURATION);
                OverlayAction::Consumed
            }
            _ => OverlayAction::Consumed,
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let popup = centered(area, 60, 5);
        // 工作台每 100 ms 重绘一次，用户停止输入后仍会准时转为掩码。
        let now = Instant::now();
        let mask = self
            .value
            .chars()
            .enumerate()
            .map(|(index, character)| {
                if self
                    .visible_until
                    .get(index)
                    .is_some_and(|deadline| *deadline > now)
                {
                    character
                } else {
                    '*'
                }
            })
            .collect::<String>();
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(format!("{}\n\n{mask}", self.label)).block(
                Block::default()
                    .title(self.title.as_str())
                    .borders(Borders::ALL),
            ),
            popup,
        );
    }
}

/// 键盘驱动的字符串选择器。
///
/// 内容与输入都由 [`PickerWidget`] 提供：它持有条目、过滤输入与选择状态；本浮层负责
/// 弹出窗口尺寸与命中区域，并把 widget 动作映射回浮层结果。
pub struct PickerOverlay {
    picker: crate::workbench::ui::PickerWidget,
}

impl PickerOverlay {
    /// 为已筛选条目创建选择器。
    #[must_use]
    pub fn new(title: impl Into<String>, items: Vec<String>) -> Self {
        Self {
            picker: crate::workbench::ui::PickerWidget::with_labels(title, items),
        }
    }

    /// 创建显示标签与接受值不同的选择器。
    #[must_use]
    pub fn with_items(title: impl Into<String>, items: Vec<MenuItem>) -> Self {
        Self {
            picker: crate::workbench::ui::PickerWidget::new(title, items),
        }
    }
}

impl Overlay for PickerOverlay {
    fn preferred_size(&self) -> (u16, u16) {
        let height = u16::try_from(self.picker.items().len())
            .unwrap_or(u16::MAX)
            .saturating_add(2)
            .max(3);
        (70, height)
    }

    fn window(&self) -> OverlayWindow {
        OverlayWindow {
            title: self.picker.title(),
            size: self.preferred_size(),
            absolute_width: false,
        }
    }

    fn handle_input(&mut self, event: InputEvent) -> OverlayAction {
        // `j`/`k` 与方向键一致；其余按键交给选择器，用于过滤输入。
        let event = match event.key {
            Key::Char('j') => InputEvent {
                key: Key::Down,
                ..event
            },
            Key::Char('k') => InputEvent {
                key: Key::Up,
                ..event
            },
            _ => event,
        };
        match self.picker.handle_input(event) {
            crate::workbench::ui::WidgetAction::Commands(commands) => {
                for command in commands {
                    if let teal_workbench_core::AppCommand::Command(line) = command {
                        if let Some(value) = line.strip_prefix("pick ") {
                            return OverlayAction::Accepted(value.to_owned());
                        }
                        if line == "picker-cancel" {
                            return OverlayAction::Cancelled;
                        }
                    }
                }
                OverlayAction::Consumed
            }
            // 过滤输入会改变可见集合；选择变化也视为消费。
            _ => OverlayAction::Consumed,
        }
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: Rect) -> OverlayAction {
        let popup = picker_rect(area, self.picker.items().len());
        let Some(index) = menu_row(popup, event.column, event.row, self.picker.items().len())
        else {
            return if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
                OverlayAction::Cancelled
            } else {
                OverlayAction::Consumed
            };
        };
        let _ = index;
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.picker
                    .items()
                    .get(index)
                    .map_or(OverlayAction::Consumed, |item| {
                        if item.enabled {
                            OverlayAction::Accepted(item.id.clone())
                        } else {
                            OverlayAction::Consumed
                        }
                    })
            }
            _ => OverlayAction::Consumed,
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let popup = picker_rect(area, self.picker.items().len());
        crate::workbench::ui::Widget::render(&mut self.picker, frame, popup, true);
    }
}

/// 同时支持键盘导航、由指针锚定的操作菜单。
pub struct ContextMenuOverlay {
    menu: ContextMenu,
    selected: usize,
}

impl ContextMenuOverlay {
    /// 在请求的终端位置创建上下文菜单。
    #[must_use]
    pub fn new(menu: ContextMenu) -> Self {
        let selected = menu.items.iter().position(|item| item.enabled).unwrap_or(0);
        Self { menu, selected }
    }

    fn next_enabled(&mut self, delta: isize) {
        if self.menu.items.is_empty() {
            return;
        }
        let mut candidate = self.selected;
        for _ in 0..self.menu.items.len() {
            let next = candidate
                .saturating_add_signed(delta)
                .min(self.menu.items.len() - 1);
            if next == candidate {
                return;
            }
            candidate = next;
            if self.menu.items[candidate].enabled {
                self.selected = candidate;
                return;
            }
        }
    }
}

impl Overlay for ContextMenuOverlay {
    fn preferred_size(&self) -> (u16, u16) {
        let width = self
            .menu
            .items
            .iter()
            .map(|item| item.label.chars().count())
            .max()
            .and_then(|width| u16::try_from(width.saturating_add(4)).ok())
            .unwrap_or(12)
            .max(12);
        let height = u16::try_from(self.menu.items.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2)
            .max(3);
        (width, height)
    }

    fn window(&self) -> OverlayWindow {
        OverlayWindow {
            title: "Actions".to_owned(),
            size: self.preferred_size(),
            absolute_width: true,
        }
    }

    fn handle_input(&mut self, event: InputEvent) -> OverlayAction {
        match event.key {
            Key::Up | Key::Char('k') => {
                self.next_enabled(-1);
                OverlayAction::Consumed
            }
            Key::Down | Key::Char('j') => {
                self.next_enabled(1);
                OverlayAction::Consumed
            }
            Key::Enter => {
                self.menu
                    .items
                    .get(self.selected)
                    .map_or(OverlayAction::Cancelled, |item| {
                        if item.enabled {
                            OverlayAction::Accepted(item.id.clone())
                        } else {
                            OverlayAction::Consumed
                        }
                    })
            }
            Key::Escape => OverlayAction::Cancelled,
            _ => OverlayAction::Consumed,
        }
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: Rect) -> OverlayAction {
        let popup = context_rect(area, &self.menu);
        let Some(index) = menu_row(popup, event.column, event.row, self.menu.items.len()) else {
            return if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
                OverlayAction::Cancelled
            } else {
                OverlayAction::Consumed
            };
        };
        self.selected = index;
        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            let item = &self.menu.items[index];
            if item.enabled {
                OverlayAction::Accepted(item.id.clone())
            } else {
                OverlayAction::Consumed
            }
        } else {
            OverlayAction::Consumed
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let popup = context_rect(area, &self.menu);
        let items = self.menu.items.iter().enumerate().map(|(index, item)| {
            let label = if index == self.selected {
                format!("> {}", item.label)
            } else {
                format!("  {}", item.label)
            };
            ListItem::new(label).style(if item.enabled {
                Style::default()
            } else {
                Style::default().fg(Color::DarkGray)
            })
        });
        frame.render_widget(Clear, popup);
        frame.render_widget(
            List::new(items).block(Block::default().title("Actions").borders(Borders::ALL)),
            popup,
        );
    }
}

fn picker_rect(area: Rect, item_count: usize) -> Rect {
    let height = u16::try_from(item_count)
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(area.height.saturating_sub(2))
        .max(3);
    centered(area, 70, height)
}

fn context_rect(area: Rect, menu: &ContextMenu) -> Rect {
    let width = menu
        .items
        .iter()
        .map(|item| item.label.chars().count())
        .max()
        .and_then(|width| u16::try_from(width.saturating_add(4)).ok())
        .unwrap_or(12)
        .max(12)
        .min(area.width);
    let height = u16::try_from(menu.items.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(area.height)
        .max(3);
    Rect {
        x: menu.x.min(area.right().saturating_sub(width)),
        y: menu.y.min(area.bottom().saturating_sub(height)),
        width,
        height,
    }
}

fn menu_row(popup: Rect, x: u16, y: u16, item_count: usize) -> Option<usize> {
    if x <= popup.x
        || x >= popup.right().saturating_sub(1)
        || y <= popup.y
        || y >= popup.bottom().saturating_sub(1)
    {
        return None;
    }
    let row = usize::from(y.saturating_sub(popup.y).saturating_sub(1));
    (row < item_count).then_some(row)
}

fn centered(area: Rect, width_percent: u16, height: u16) -> Rect {
    let width = area
        .width
        .saturating_mul(width_percent)
        .saturating_div(100)
        .max(10);
    Rect {
        x: area.x.saturating_add(area.width.saturating_sub(width) / 2),
        y: area
            .y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use teal_workbench_core::{InputEvent, Key, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    use super::super::{ContextMenu, MenuItem};

    use super::{
        ConfirmOverlay, ContextMenuOverlay, Overlay, OverlayAction, PickerOverlay,
        SecretInputOverlay,
    };

    #[test]
    fn picker_filters_by_typing_and_accepts_the_narrowed_selection() {
        let mut overlay = PickerOverlay::new(
            "runtime profile",
            vec!["core".to_owned(), "agent".to_owned(), "full".to_owned()],
        );
        let key = |key| InputEvent {
            key,
            modifiers: KeyModifiers::default(),
        };
        // 过滤输入把候选收窄到 `agent`。
        assert_eq!(
            overlay.handle_input(key(Key::Char('a'))),
            OverlayAction::Consumed
        );
        assert_eq!(
            overlay.handle_input(key(Key::Char('g'))),
            OverlayAction::Consumed
        );
        assert_eq!(
            overlay.handle_input(key(Key::Enter)),
            OverlayAction::Accepted("agent".to_owned())
        );
    }

    #[test]
    fn picker_keeps_jk_navigation_and_escape_cancellation() {
        let mut overlay = PickerOverlay::new(
            "buffer",
            vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
        );
        let key = |key| InputEvent {
            key,
            modifiers: KeyModifiers::default(),
        };
        // j 下移一格后回车接受第二个条目。
        assert_eq!(
            overlay.handle_input(key(Key::Char('j'))),
            OverlayAction::Consumed
        );
        assert_eq!(
            overlay.handle_input(key(Key::Enter)),
            OverlayAction::Accepted("two".to_owned())
        );
        assert_eq!(
            overlay.handle_input(key(Key::Escape)),
            OverlayAction::Cancelled
        );
    }

    #[test]
    fn confirmation_accepts_only_its_visible_button() {
        let mut overlay = ConfirmOverlay::new("Confirm", "Continue?");
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::default(),
            click_count: 1,
        };
        assert_eq!(
            overlay.handle_mouse(click(40, 12), area),
            OverlayAction::Consumed
        );
        assert_eq!(
            overlay.handle_mouse(click(18, 12), area),
            OverlayAction::Accepted("yes".to_owned())
        );
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| overlay.render(frame, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let yes = buffer
            .content()
            .iter()
            .position(|cell| cell.symbol() == "Y")
            .unwrap();
        let no = buffer
            .content()
            .iter()
            .position(|cell| cell.symbol() == "N")
            .unwrap();
        for (position, expected) in [
            (yes, OverlayAction::Accepted("yes".into())),
            (no, OverlayAction::Cancelled),
        ] {
            let x = u16::try_from(position % 80).unwrap();
            let y = u16::try_from(position / 80).unwrap();
            assert_eq!(overlay.handle_mouse(click(x, y), area), expected);
            assert_eq!(overlay.handle_mouse(click(x + 1, y), area), expected);
        }
    }

    #[test]
    fn context_menu_clamps_to_terminal_and_ignores_disabled_items() {
        let area = ratatui::layout::Rect::new(0, 0, 30, 8);
        let mut overlay = ContextMenuOverlay::new(ContextMenu {
            x: 29,
            y: 7,
            items: vec![
                MenuItem::disabled("disabled", "Disabled"),
                MenuItem::enabled("copy", "Copy"),
            ],
        });
        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::default(),
            click_count: 1,
        };
        assert_eq!(
            overlay.handle_mouse(click(19, 5), area),
            OverlayAction::Consumed
        );
        assert_eq!(
            overlay.handle_mouse(click(19, 6), area),
            OverlayAction::Accepted("copy".to_owned())
        );
    }

    #[test]
    fn secret_input_reveals_new_characters_then_masks_each_expired_character() {
        let mut overlay = SecretInputOverlay::new("Authentication", "Token");
        for character in "ab".chars() {
            assert_eq!(
                overlay.handle_input(InputEvent {
                    key: Key::Char(character),
                    modifiers: KeyModifiers::default(),
                }),
                OverlayAction::Consumed
            );
        }
        let now = Instant::now();
        overlay.visible_until = vec![now - Duration::from_secs(1), now + Duration::from_secs(60)];
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| overlay.render(frame, frame.area()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("*b"));

        overlay.visible_until.fill(now - Duration::from_secs(1));
        terminal
            .draw(|frame| overlay.render(frame, frame.area()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("**"));
        assert!(!rendered.contains("ab"));
    }

    #[test]
    fn secret_input_acceptance_debug_never_contains_the_value() {
        let mut overlay = SecretInputOverlay::new("Authentication", "Token");
        for character in "private-token".chars() {
            assert_eq!(
                overlay.handle_input(InputEvent {
                    key: Key::Char(character),
                    modifiers: KeyModifiers::default(),
                }),
                OverlayAction::Consumed
            );
        }

        let action = overlay.handle_input(InputEvent {
            key: Key::Enter,
            modifiers: KeyModifiers::default(),
        });
        assert!(!format!("{action:?}").contains("private-token"));
        let OverlayAction::SensitiveAccepted(value) = action else {
            panic!("sensitive value should be returned");
        };
        assert_eq!(value.expose(), "private-token");
    }
}
