//! 通用面板、实例存储与渲染几何契约。
//!
//! 面板存储和布局树独立维护；一次渲染返回的 `FrameGeometry` 同时是鼠标路由依据。
//! 状态行、边框和正文区域须使用相同的终端单元坐标。

use ratatui::{
    Frame,
    layout::Rect as TuiRect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};
use slotmap::SlotMap;
use unicode_width::UnicodeWidthStr;

use teal_workbench_core::{
    AppCommand, InputEvent, MessageLog, MouseEvent, PanelId, Rect, WindowId, WindowSet,
};

use super::CompletionMenu;
use super::Prompt;

/// 会话与状态渲染使用的稳定面板类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelKind {
    /// 宿主渲染的 Teal 组件面板。
    Custom,

    /// 可编辑代码或文本文档。
    Editor,

    /// 带同步预览的惰性文件树。
    Explorer,

    /// 结构化 Markdown 文档。
    Markdown,

    /// 不含 JavaScript 的文本网页文档。
    Browser,

    /// 工作区或缓冲区搜索结果。
    Search,

    /// 智能体对话与活动。
    Agent,

    /// 待审查的工作区变更。
    Diff,

    /// 结构化消息历史。
    Messages,

    /// 子进程输出。
    Output,
}

impl PanelKind {
    /// 返回稳定会话标识。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Custom => "teal-custom",
            Self::Editor => "editor",
            Self::Explorer => "explorer",
            Self::Markdown => "markdown",
            Self::Browser => "browser",
            Self::Search => "search",
            Self::Agent => "agent",
            Self::Diff => "diff",
            Self::Messages => "messages",
            Self::Output => "output",
        }
    }
}

/// 面板局部状态栏字段。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PanelStatus {
    /// 左对齐模式或状态。
    pub mode: String,

    /// 居中的资源名称。
    pub resource: String,

    /// 右对齐位置与服务状态。
    pub position: String,
}

impl PanelStatus {
    /// 把状态字段投影为元素列表。
    ///
    /// 元素是状态行的绘制单位：模式、资源、位置，以及附加的诊断计数、运行中与审批
    /// 标记。空字段不产生元素，因此短状态行不会留下多余分隔。
    #[must_use]
    pub fn elements(&self) -> Vec<StatusElement> {
        let mut elements = Vec::new();
        if !self.mode.is_empty() {
            elements.push(StatusElement::Mode(self.mode.clone()));
        }
        if !self.resource.is_empty() {
            elements.push(StatusElement::Resource(self.resource.clone()));
        }
        if !self.position.is_empty() {
            elements.push(StatusElement::Position(self.position.clone()));
        }
        elements
    }
}

/// 状态行上的一个元素。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusElement {
    /// 编辑模式或工作流状态。
    Mode(String),

    /// 聚焦资源名称。
    Resource(String),

    /// 行列或进度位置。
    Position(String),

    /// 诊断计数。
    Diagnostics(String),

    /// 运行中标记。
    Running,

    /// 待处理审批数量。
    Approvals(usize),

    /// 其他自由文本。
    Text(String),
}

impl StatusElement {
    /// 返回该元素的显示文本。
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Mode(value)
            | Self::Resource(value)
            | Self::Position(value)
            | Self::Text(value) => value.clone(),
            Self::Diagnostics(value) => format!("diag {value}"),
            Self::Running => "running".to_owned(),
            Self::Approvals(count) => format!("approvals {count}"),
        }
    }

    /// 返回该元素的显示样式。
    #[must_use]
    pub fn style(&self) -> Style {
        match self {
            Self::Mode(_) => Style::default().fg(Color::Cyan),
            Self::Resource(_) => Style::default(),
            Self::Position(_) => Style::default().fg(Color::DarkGray),
            Self::Diagnostics(_) => Style::default().fg(Color::Yellow),
            Self::Running => Style::default().fg(Color::Red),
            Self::Approvals(_) => Style::default().fg(Color::Yellow),
            Self::Text(_) => Style::default(),
        }
    }
}

/// 指针锚定上下文菜单中的一个条目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    /// 由应用或来源面板解释的稳定操作标识。
    pub id: String,

    /// 用户可见标签。
    pub label: String,

    /// 当前是否可接受此项。
    pub enabled: bool,
}

impl MenuItem {
    /// 创建一个启用的菜单项。
    #[must_use]
    pub fn enabled(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            enabled: true,
        }
    }

    /// 创建一个禁用的菜单项。
    #[must_use]
    pub fn disabled(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            enabled: false,
        }
    }
}

/// 面板在绝对终端位置请求的上下文菜单。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextMenu {
    /// 限制到终端范围前的首选左列。
    pub x: u16,

    /// 限制到终端范围前的首选顶部行。
    pub y: u16,

    /// 按显示顺序排列的菜单项。
    pub items: Vec<MenuItem>,
}

/// 菜单或中键点击请求的剪贴板操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardAction {
    /// 复制当前非空选择区。
    Copy,

    /// 复制并删除当前非空可编辑选择区。
    Cut,

    /// 读取系统剪贴板，并在目标选择区替换或插入。
    Paste,

    /// 选择全部可用文本。
    SelectAll,
}

/// 对一个局部文本所有者应用剪贴板操作的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipboardOutcome {
    /// 目标是否识别该操作。
    pub consumed: bool,

    /// 必须写入系统剪贴板的纯文本。
    pub write: Option<String>,
}

/// 将一个输入事件路由到面板的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum WidgetAction {
    /// 发给已注册 Teal 面板回调的类型化字符串事件。
    CustomEvent(std::collections::BTreeMap<String, String>),

    /// 事件未被识别，可以冒泡。
    #[default]
    Ignored,

    /// 事件更改了面板局部状态。
    Consumed,

    /// 事件产生一个或多个类型化应用命令。
    Commands(Vec<AppCommand>),

    /// 事件请求上下文菜单。
    Menu(ContextMenu),

    /// 事件请求系统剪贴板操作。
    Clipboard(ClipboardAction),

    /// 事件产生必须写入系统剪贴板的纯文本。
    WriteClipboard(String),
}

pub use super::widget::{Widget, WidgetChrome};

/// 一个面板的渲染与命中测试几何信息。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelGeometry {
    /// 面板标识。
    pub panel: PanelId,

    /// 拥有此面板的窗口。
    pub window: WindowId,

    /// 包含可选状态行的完整布局分配。
    pub area: Rect,

    /// 面板渲染的正文。
    pub body: Rect,

    /// 可选的单行面板状态分配。
    pub status: Option<Rect>,
}

/// 一个窗口的绘制与命中测试几何信息。
///
/// 停靠窗口没有标题栏与缩放手柄；浮动窗口的 `chrome` 与 `resize` 只在其自身矩形内有效，
/// 且只有 `Interactive + Title` 才记录这些命中区域。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowGeometry {
    /// 窗口标识。
    pub window: WindowId,

    /// 窗口取得的内容区分配；停靠窗口为全部内容区，浮动窗口为其矩形。
    pub area: Rect,

    /// 浮动窗口标题栏命中区域。
    pub chrome: Option<Rect>,

    /// 浮动窗口的边与角缩放手柄命中区域。
    pub resize: Vec<ResizeHandle>,
}

/// 浮动窗口边框上的一个可缩放区域。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResizeHandle {
    /// 命中区域。
    pub area: Rect,

    /// 该区域的缩放锚点。
    pub anchor: teal_workbench_core::ResizeAnchor,
}

/// 帧渲染与鼠标路由共享的精确几何信息。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameGeometry {
    /// 完整终端区域。
    pub full: Rect,

    /// 可选的窗口标签条分配。
    pub tab_strip: Option<Rect>,

    /// 标签条中每个窗口的命中区域。
    pub tabs: Vec<(WindowId, Rect)>,

    /// 命令/消息行上方的布局区域。
    pub content: Rect,

    /// 可选全局命令/消息行。
    pub command_line: Option<Rect>,

    /// 可选的命令补全候选行。
    pub completions: Option<Rect>,

    /// 按绘制顺序排列的窗口分配。
    pub windows: Vec<WindowGeometry>,

    /// 按布局顺序排列的面板分配。
    pub panels: Vec<PanelGeometry>,
}

impl FrameGeometry {
    /// 返回位于标签条中的窗口。
    #[must_use]
    pub fn tab_at(&self, x: u16, y: u16) -> Option<WindowId> {
        self.tabs
            .iter()
            .find(|(_, rect)| rect.contains(x, y))
            .map(|(id, _)| *id)
    }

    /// 返回包含绝对终端单元格的面板正文。
    #[must_use]
    pub fn panel_at(&self, x: u16, y: u16) -> Option<PanelGeometry> {
        self.panels
            .iter()
            .copied()
            .find(|geometry| geometry.body.contains(x, y))
    }

    /// 返回包含绝对终端单元格的完整面板分配。
    #[must_use]
    pub fn allocation_at(&self, x: u16, y: u16) -> Option<PanelGeometry> {
        self.panels
            .iter()
            .copied()
            .find(|geometry| geometry.area.contains(x, y))
    }

    /// 返回一个面板标识的几何信息。
    #[must_use]
    pub fn panel(&self, panel: PanelId) -> Option<PanelGeometry> {
        self.panels
            .iter()
            .copied()
            .find(|geometry| geometry.panel == panel)
    }

    /// 返回一个窗口的几何信息。
    #[must_use]
    pub fn window(&self, window: WindowId) -> Option<WindowGeometry> {
        self.windows
            .iter()
            .find(|geometry| geometry.window == window)
            .cloned()
    }

    /// 返回包含绝对终端单元格的窗口，按绘制顺序自上而下选择。
    ///
    /// 浮动窗口先于停靠窗口匹配，因此叠加区域属于上层窗口。
    #[must_use]
    pub fn window_at(&self, x: u16, y: u16) -> Option<WindowGeometry> {
        self.windows
            .iter()
            .rev()
            .find(|geometry| geometry.area.contains(x, y))
            .cloned()
    }

    /// 返回包含绝对终端单元格的浮动窗口边框命中部位。
    ///
    /// 命中优先级为角 > 边 > 标题栏，因此重叠的角区域先于相邻边被选中。
    #[must_use]
    pub fn window_frame_at(&self, x: u16, y: u16) -> Option<(WindowId, WindowFramePart)> {
        self.windows.iter().rev().find_map(|geometry| {
            if let Some(handle) = geometry
                .resize
                .iter()
                .find(|handle| handle.area.contains(x, y))
            {
                Some((geometry.window, WindowFramePart::Resize(handle.anchor)))
            } else if geometry.chrome.is_some_and(|area| area.contains(x, y)) {
                Some((geometry.window, WindowFramePart::Chrome))
            } else {
                None
            }
        })
    }
}

/// 浮动窗口边框上的可交互部位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowFramePart {
    /// 标题栏；拖动改变位置。
    Chrome,

    /// 边或角缩放手柄；拖动按锚点改变尺寸。
    Resize(teal_workbench_core::ResizeAnchor),
}

/// 拥有所有存活面板的代际存储。
pub struct PanelStore<P: Widget> {
    panels: SlotMap<PanelId, P>,
}

impl<P: Widget> Default for PanelStore<P> {
    fn default() -> Self {
        Self {
            panels: SlotMap::with_key(),
        }
    }
}

impl<P: Widget> PanelStore<P> {
    /// 插入面板并返回其稳定标识。
    pub fn insert(&mut self, panel: P) -> PanelId {
        self.panels.insert(panel)
    }

    /// 返回共享面板引用。
    pub fn get(&self, id: PanelId) -> Option<&P> {
        self.panels.get(id)
    }

    /// 返回可变面板引用。
    pub fn get_mut(&mut self, id: PanelId) -> Option<&mut P> {
        self.panels.get_mut(id)
    }

    /// 移除并返回一个面板。
    pub fn remove(&mut self, id: PanelId) -> Option<P> {
        self.panels.remove(id)
    }

    /// 迭代标识与面板。
    pub fn iter(&self) -> impl Iterator<Item = (PanelId, &P)> {
        self.panels.iter()
    }
}

/// 渲染窗口标签条、活动窗口的拆分树、各面板状态栏、全局命令行和最新消息。
pub fn render_frame<P: Widget>(
    frame: &mut Frame<'_>,
    windows: &WindowSet,
    panels: &mut PanelStore<P>,
    cmdline: Option<&Prompt>,
    completions: &CompletionMenu,
    messages: &MessageLog,
) -> FrameGeometry {
    let full = frame.area();
    let command_height = u16::from(cmdline.is_some() || messages.latest().is_some());
    let completion_height =
        u16::from(!completions.is_empty() && cmdline.is_some() && command_height == 1);
    let body = TuiRect {
        x: full.x,
        y: full.y,
        width: full.width,
        height: full
            .height
            .saturating_sub(command_height)
            .saturating_sub(completion_height),
    };
    // 单窗口不占标签条，保持既有布局高度；浮动窗口不进标签条。
    let tab_height = u16::from(windows.docked().count() > 1);
    let tab_strip = (tab_height == 1).then_some(TuiRect {
        x: body.x,
        y: body.y,
        width: body.width,
        height: 1,
    });
    let content = TuiRect {
        x: body.x,
        y: body.y.saturating_add(tab_height),
        width: body.width,
        height: body.height.saturating_sub(tab_height),
    };
    let core_area = Rect {
        x: content.x,
        y: content.y,
        width: content.width,
        height: content.height,
    };
    let mut geometry = FrameGeometry {
        full: Rect {
            x: full.x,
            y: full.y,
            width: full.width,
            height: full.height,
        },
        tab_strip: tab_strip.map(|area| Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: area.height,
        }),
        tabs: Vec::new(),
        content: core_area,
        command_line: (command_height == 1).then_some(Rect {
            x: full.x,
            y: full.bottom().saturating_sub(1),
            width: full.width,
            height: 1,
        }),
        completions: (completion_height == 1).then_some(Rect {
            x: full.x,
            y: body.bottom(),
            width: full.width,
            height: 1,
        }),
        windows: Vec::new(),
        panels: Vec::new(),
    };
    if let Some(area) = tab_strip {
        render_tabs(frame, windows, area, &mut geometry.tabs);
    }

    // 停靠窗口独占内容区；浮动窗口按其矩形在内容区之上叠加。
    let active = windows.active_id();
    geometry.windows.push(WindowGeometry {
        window: active,
        area: core_area,
        chrome: None,
        resize: Vec::new(),
    });
    render_window(frame, active, content, windows, panels, &mut geometry);

    for id in windows.floating() {
        if id == active {
            continue;
        }
        let Some(window) = windows.get(id) else {
            continue;
        };
        let Some(rect) = window.placement().rect() else {
            continue;
        };
        let area = clamp_rect(rect, core_area);
        let chrome = (area.height > 0).then_some(TuiRect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        });
        // 只有可交互且带标题框的浮动窗口才响应边角拖动。
        let resize = if window.is_draggable() {
            resize_handles(area)
        } else {
            Vec::new()
        };
        let inner = match chrome {
            Some(chrome) => TuiRect {
                x: area.x,
                y: area.y.saturating_add(1),
                width: area.width,
                height: area.height.saturating_sub(1),
            },
            None => area,
        };
        geometry.windows.push(WindowGeometry {
            window: id,
            area: to_core_rect(area),
            chrome: chrome.map(to_core_rect),
            resize,
        });
        if let Some(chrome) = chrome {
            let label = format!(" {} ", window.title());
            frame.render_widget(
                Paragraph::new(label).style(Style::default().add_modifier(Modifier::REVERSED)),
                chrome,
            );
        }
        if inner.width > 0 && inner.height > 0 {
            frame.render_widget(Clear, inner);
            render_window(frame, id, inner, windows, panels, &mut geometry);
        }
    }

    if command_height == 1 {
        let area = TuiRect {
            x: full.x,
            y: full.bottom().saturating_sub(1),
            width: full.width,
            height: 1,
        };
        let text = cmdline.map(|cmdline| cmdline.render_line()).or_else(|| {
            messages.latest().map(|message| {
                ratatui::text::Line::from(format!("[{}] {}", message.source, message.text))
            })
        });
        frame.render_widget(
            Paragraph::new(text.unwrap_or_default())
                .scroll((0, cmdline.map_or(0, |cmdline| cmdline.viewport()))),
            area,
        );
        if let Some(cmdline) = cmdline {
            frame.set_cursor_position((
                area.x.saturating_add(cmdline.cursor_cell(area.width)),
                area.y,
            ));
        }
    }
    if let Some(area) = geometry.completions {
        let area = TuiRect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: area.height,
        };
        let mut spans = Vec::new();
        for (index, candidate) in completions.candidates().iter().enumerate() {
            let style = if index == completions.selected() {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default().fg(ratatui::style::Color::DarkGray)
            };
            spans.push(Span::styled(candidate.clone(), style));
            if index + 1 < completions.candidates().len() {
                spans.push(Span::raw("  "));
            }
        }
        frame.render_widget(Paragraph::new(Line::from(spans)).scroll((0, 0)), area);
    }
    geometry
}

/// 渲染一个窗口的拆分树、各面板正文与状态行。
///
/// 命中几何以窗口归属记录，浮动窗口的标题栏与缩放手柄由调用方写入。
fn render_window<P: Widget>(
    frame: &mut Frame<'_>,
    window_id: WindowId,
    area: TuiRect,
    windows: &WindowSet,
    panels: &mut PanelStore<P>,
    geometry: &mut FrameGeometry,
) {
    let Some(window) = windows.get(window_id) else {
        return;
    };
    let layout = window.layout();
    let core_area = to_core_rect(area);
    for (panel_id, rect) in layout.rectangles(core_area) {
        let Some(panel) = panels.get_mut(panel_id) else {
            continue;
        };
        let area = TuiRect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        };
        let focused = panel_id == layout.focused();
        // 宿主负责状态行；Widget 自绘正文，并按需声明自己的边框与标题。
        let status = panel.chrome().status;
        let body_height = area.height.saturating_sub(u16::from(status.is_some()));
        let body = TuiRect {
            height: body_height,
            ..area
        };
        let status_area = status.as_ref().map(|_| TuiRect {
            y: area.y.saturating_add(body_height),
            height: 1,
            ..area
        });
        geometry.panels.push(PanelGeometry {
            panel: panel_id,
            window: window_id,
            area: rect,
            body: Rect {
                x: body.x,
                y: body.y,
                width: body.width,
                height: body.height,
            },
            status: status_area.map(|area| Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: area.height,
            }),
        });
        panel.render(frame, body, focused);
        if let Some(status) = status {
            let status_area =
                status_area.expect("status bar geometry must exist when the status bar is present");
            // 状态行是元素列表：每个元素独立着色，空字段不占位。
            let mut spans = vec![Span::raw(" ")];
            for element in status.elements() {
                spans.push(Span::styled(element.text(), element.style()));
                spans.push(Span::raw("  "));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), status_area);
        }
        if focused && let Some((x, y)) = panel.cursor(body) {
            frame.set_cursor_position((body.x.saturating_add(x), body.y.saturating_add(y)));
        }
    }
}

/// 把 TUI 矩形转换为工作台核心矩形。
fn to_core_rect(area: TuiRect) -> Rect {
    Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height,
    }
}

/// 返回一个浮动窗口边框上的全部可缩放区域。
///
/// 角区域先于边区域排列，因此角在重叠处优先命中；宽度或高度为 1 时对应区域退化为
/// 单格，但仍可命中。
fn resize_handles(area: TuiRect) -> Vec<ResizeHandle> {
    use teal_workbench_core::ResizeAnchor;

    let right = area.x.saturating_add(area.width).saturating_sub(1);
    let bottom = area.y.saturating_add(area.height).saturating_sub(1);
    let inner_height = area.height.saturating_sub(2);
    let inner_width = area.width.saturating_sub(2);
    let mut handles = Vec::with_capacity(8);
    for anchor in ResizeAnchor::all() {
        let rect = match anchor {
            ResizeAnchor::TopLeft => TuiRect::new(area.x, area.y, 1, 1),
            ResizeAnchor::TopRight => TuiRect::new(right, area.y, 1, 1),
            ResizeAnchor::BottomLeft => TuiRect::new(area.x, bottom, 1, 1),
            ResizeAnchor::BottomRight => TuiRect::new(right, bottom, 1, 1),
            ResizeAnchor::Top => TuiRect::new(area.x.saturating_add(1), area.y, inner_width, 1),
            ResizeAnchor::Bottom => TuiRect::new(area.x.saturating_add(1), bottom, inner_width, 1),
            ResizeAnchor::Left => TuiRect::new(area.x, area.y.saturating_add(1), 1, inner_height),
            ResizeAnchor::Right => TuiRect::new(right, area.y.saturating_add(1), 1, inner_height),
        };
        // 退化区域（宽或高为 0）不参与命中。
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        handles.push(ResizeHandle {
            area: to_core_rect(rect),
            anchor,
        });
    }
    handles
}

/// 把矩形钳制到内容区内。
fn clamp_rect(rect: Rect, bounds: Rect) -> TuiRect {
    let width = rect.width.min(bounds.width);
    let height = rect.height.min(bounds.height);
    let max_x = bounds.x.saturating_add(bounds.width).saturating_sub(width);
    let max_y = bounds
        .y
        .saturating_add(bounds.height)
        .saturating_sub(height);
    let x = rect.x.clamp(bounds.x, max_x);
    let y = rect.y.clamp(bounds.y, max_y);
    TuiRect {
        x,
        y,
        width,
        height,
    }
}

/// 渲染窗口标签条并记录每个标签的命中区域。
///
/// 标签顺序与窗口顺序一致；活动标签使用加粗样式，其余标签保留普通样式。
fn render_tabs(
    frame: &mut Frame<'_>,
    windows: &WindowSet,
    area: TuiRect,
    hits: &mut Vec<(WindowId, Rect)>,
) {
    let active = windows.active_id();
    let mut spans = Vec::new();
    let mut column = area.x;
    for (index, id) in windows.docked().enumerate() {
        let Some(window) = windows.get(id) else {
            continue;
        };
        let label = format!(
            " {} {} {} ",
            index + 1,
            window.kind().as_str(),
            window.title()
        );
        let width = u16::try_from(UnicodeWidthStr::width(label.as_str())).unwrap_or(u16::MAX);
        if column >= area.x.saturating_add(area.width) {
            break;
        }
        let visible = width.min(area.x.saturating_add(area.width).saturating_sub(column));
        hits.push((
            id,
            Rect {
                x: column,
                y: area.y,
                width: visible,
                height: 1,
            },
        ));
        let style = if id == active {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        spans.push(Span::styled(label, style));
        column = column.saturating_add(width);
    }
    if !spans.is_empty() {
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};
    use teal_workbench_core::{
        Document, KeymapPreset, LayoutTree, MessageLog, Window, WindowKind, WindowSet,
    };

    use super::{PanelStore, render_frame};
    use super::super::EditorPanel;
    use super::super::CompletionMenu;

    #[test]
    fn status_elements_skip_empty_fields_and_render_styled_text() {
        use super::{PanelStatus, StatusElement};

        let status = PanelStatus {
            mode: "NORMAL".to_owned(),
            resource: "src/main.tl".to_owned(),
            position: "12:4".to_owned(),
        };
        let elements = status.elements();
        assert_eq!(
            elements,
            vec![
                StatusElement::Mode("NORMAL".to_owned()),
                StatusElement::Resource("src/main.tl".to_owned()),
                StatusElement::Position("12:4".to_owned()),
            ]
        );
        assert_eq!(
            elements.iter().map(StatusElement::text).collect::<Vec<_>>(),
            vec!["NORMAL", "src/main.tl", "12:4"]
        );

        // 空字段不产生元素；附加标记使用固定文本。
        let empty = PanelStatus {
            mode: "INSERT".to_owned(),
            ..PanelStatus::default()
        };
        assert_eq!(
            empty.elements(),
            vec![StatusElement::Mode("INSERT".to_owned())]
        );
        assert_eq!(StatusElement::Running.text(), "running");
        assert_eq!(StatusElement::Diagnostics("2".to_owned()).text(), "diag 2");
        assert_eq!(StatusElement::Approvals(3).text(), "approvals 3");
    }

    #[test]
    fn floating_windows_are_composited_above_the_docked_window() {
        use teal_workbench_core::{Rect, WindowPlacement};

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut panels = PanelStore::default();
        let docked = panels.insert(EditorPanel::new(
            Document::from_text("docked"),
            KeymapPreset::VimFirst,
        ));
        let floating = panels.insert(EditorPanel::new(
            Document::from_text("floating"),
            KeymapPreset::VimFirst,
        ));
        let mut windows = WindowSet::new(Window::new(
            WindowKind::Editor,
            "primary",
            LayoutTree::new(docked),
        ));
        let floating_id = windows.insert(Window::new(
            WindowKind::Editor,
            "popup",
            LayoutTree::new(floating),
        ));
        assert!(windows.set_floating(
            floating_id,
            Rect {
                x: 5,
                y: 3,
                width: 30,
                height: 8,
            },
        ));
        assert!(windows.select(windows.primary()));

        let mut captured = None;
        terminal
            .draw(|frame| {
                captured = Some(render_frame(
                    frame,
                    &windows,
                    &mut panels,
                    None,
                    &CompletionMenu::default(),
                    &MessageLog::default(),
                ));
            })
            .expect("frame should render");
        let geometry = captured.expect("render_frame returns geometry");

        // 停靠窗口与浮动窗口都记录几何，浮动的标题栏和缩放手柄可命中。
        assert_eq!(geometry.windows.len(), 2);
        let popup = geometry
            .window(floating_id)
            .expect("floating window geometry");
        assert!(popup.chrome.is_some());
        assert_eq!(popup.resize.len(), 8, "four edges and four corners");
        assert_eq!(
            geometry.window_at(6, 4).map(|g| g.window),
            Some(floating_id)
        );
        assert_eq!(
            geometry.window_frame_at(6, 3).map(|(id, _)| id),
            Some(floating_id)
        );
        // 浮动窗口不进标签条；只有一个停靠窗口时标签条不占行。
        assert!(geometry.tabs.is_empty());
        assert!(geometry.tab_strip.is_none());
        // 角优先于边、边优先于标题栏：左上角命中角锚点，上边中间命中上边，
        // 标题栏中间命中标题栏。
        assert_eq!(
            geometry.window_frame_at(5, 3),
            Some((
                floating_id,
                super::WindowFramePart::Resize(teal_workbench_core::ResizeAnchor::TopLeft)
            ))
        );
        assert_eq!(
            geometry.window_frame_at(15, 3),
            Some((
                floating_id,
                super::WindowFramePart::Resize(teal_workbench_core::ResizeAnchor::Top)
            ))
        );
        // 右侧边从标题栏下一行开始，因此 (34,4) 命中右边而非右下角。
        assert_eq!(
            geometry.window_frame_at(34, 4).map(|(_, part)| part),
            Some(super::WindowFramePart::Resize(
                teal_workbench_core::ResizeAnchor::Right
            ))
        );
        assert_eq!(
            geometry.window_frame_at(34, 10).map(|(_, part)| part),
            Some(super::WindowFramePart::Resize(
                teal_workbench_core::ResizeAnchor::BottomRight
            ))
        );
        // 浮动标题命中时只有带标题的可交互窗口参与拖动。
        assert!(windows.get(floating_id).is_some_and(Window::is_draggable));
        assert!(matches!(
            windows.get(floating_id).map(Window::placement),
            Some(WindowPlacement::Floating { .. })
        ));
    }

    #[test]
    fn renders_small_and_wide_terminal_frames_with_wide_characters() {
        for (width, height) in [(80, 24), (120, 40)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
            let mut panels = PanelStore::default();
            let panel = panels.insert(EditorPanel::new(
                Document::from_text(
                    "fn \u{4e16}\u{754c}() {
    let value = 42;
}",
                ),
                KeymapPreset::VimFirst,
            ));
            let layout = LayoutTree::new(panel);
            terminal
                .draw(|frame| {
                    let windows = WindowSet::new(Window::new(
                        WindowKind::Editor,
                        WindowKind::Editor.default_title(),
                        layout,
                    ));
                    let _ = render_frame(
                        frame,
                        &windows,
                        &mut panels,
                        None,
                        &CompletionMenu::default(),
                        &MessageLog::default(),
                    );
                })
                .expect("frame should render");
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer.area.width, width);
            assert_eq!(buffer.area.height, height);
            let mut rendered = String::new();
            for y in 0..height {
                let mut x = 0;
                while x < width {
                    let symbol = buffer[(x, y)].symbol();
                    rendered.push_str(symbol);
                    x += u16::try_from(unicode_width::UnicodeWidthStr::width(symbol).max(1))
                        .unwrap();
                }
            }
            assert!(rendered.contains('世') && rendered.contains('界'));
            assert!(
                rendered.contains("normal"),
                "The single-line status bar must display text, not just a border"
            );
        }
    }
}
