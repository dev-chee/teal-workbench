//! 工作台窗口模型。
//!
//! Frame 是整终端，窗口是 Frame 内可堆叠的工作上下文，并且拥有自己的拆分布局；
//! 面板实例仍由应用存储持有，窗口只维护身份、类型、标题和布局，不启动模型、
//! 不执行工作区操作，也不保存界面会话。

use serde::{Deserialize, Serialize};
use slotmap::{SlotMap, new_key_type};

use crate::layout::{Direction, LayoutError, LayoutSpec, LayoutTree, PanelId, Rect, SplitDivider};

new_key_type! {
    /// 一个窗口的稳定标识。
    pub struct WindowId;
}

/// 窗口承载的工作流类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowKind {
    /// Agent 会话与输入工作流。
    Agent,

    /// 调试工作流。
    Debug,

    /// 编辑工作流。
    Editor,

    /// 方案探索视图。
    Plan,

    /// 工作区脚本清单与脚手架视图。
    Scripts,

    /// 交付记录与收据视图。
    Delivery,

    /// 聚合诊断视图。
    Diagnostics,

    /// 差异视图。
    Diff,

    /// `@test` 测试结果视图。
    Tests,
}

impl WindowKind {
    /// 返回命令与会话使用的稳定名称。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Debug => "debug",
            Self::Editor => "editor",
            Self::Plan => "plan",
            Self::Scripts => "scripts",
            Self::Delivery => "delivery",
            Self::Diagnostics => "diagnostics",
            Self::Diff => "diff",
            Self::Tests => "tests",
        }
    }

    /// 从命令名解析窗口类型。
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "agent" => Some(Self::Agent),
            "debug" => Some(Self::Debug),
            "editor" => Some(Self::Editor),
            "plan" => Some(Self::Plan),
            "scripts" => Some(Self::Scripts),
            "delivery" => Some(Self::Delivery),
            "diagnostics" => Some(Self::Diagnostics),
            "diff" => Some(Self::Diff),
            "tests" => Some(Self::Tests),
            _ => None,
        }
    }

    /// 返回默认标题。
    #[must_use]
    pub const fn default_title(self) -> &'static str {
        match self {
            Self::Agent => "Agent",
            Self::Debug => "Debug",
            Self::Editor => "Editor",
            Self::Plan => "Plan",
            Self::Scripts => "Scripts",
            Self::Delivery => "Delivery",
            Self::Diagnostics => "Diagnostics",
            Self::Diff => "Diff",
            Self::Tests => "Tests",
        }
    }
}

/// 窗口在 Frame 中的占据方式。
///
/// `Docked` 窗口独占内容区并在标签条中显示，同一时刻只有一个 `Docked` 窗口可见；
/// `Floating` 窗口使用自己的矩形叠加在内容区之上，由 `z` 在浮动层级内排序。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "placement", rename_all = "kebab-case")]
pub enum WindowPlacement {
    /// 独占内容区并在标签条中显示。
    #[default]
    Docked,

    /// 使用显式矩形叠加显示。
    Floating {
        /// 相对终端内容区的矩形。
        rect: Rect,

        /// 浮动层级内的排序值；较大者绘制在上层。
        z: i32,
    },
}

impl WindowPlacement {
    /// 返回是否停靠。
    #[must_use]
    pub const fn is_docked(self) -> bool {
        matches!(self, Self::Docked)
    }

    /// 返回浮动矩形；停靠窗口没有。
    #[must_use]
    pub const fn rect(self) -> Option<Rect> {
        match self {
            Self::Docked => None,
            Self::Floating { rect, .. } => Some(rect),
        }
    }
}

/// 窗口是否绘制标题框。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowChrome {
    /// 绘制标题框；浮动窗口的标题框同时是拖动与缩放手柄。
    #[default]
    Title,

    /// 不绘制标题框，也不响应标题栏交互。
    None,
}

/// 窗口是否参与焦点与标签轮转。
///
/// `Passive` 窗口（补全、hover、toast）不进入标签条与 `Ctrl-w` 轮转，鼠标事件下传，
/// 位置由 `anchor` 决定。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowInteraction {
    /// 可聚焦，参与标签条与窗口轮转。
    #[default]
    Interactive,

    /// 不聚焦，事件下传给下层窗口。
    Passive,
}

/// 窗口是否随会话保存。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowLifetime {
    /// 随会话保存并在恢复时重建。
    #[default]
    Persistent,

    /// 仅本次运行有效，不进入会话。
    Transient,
}

/// 浮动窗口的锚点来源。
///
/// `Frame` 由应用居中放置；`Cursor` 与 `Rect` 由发起交互的面板或指针提供。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "anchor", rename_all = "kebab-case")]
pub enum WindowAnchor {
    /// 相对内容区居中。
    #[default]
    Frame,

    /// 贴近指针所在单元格。
    Cursor,

    /// 使用发起者的显式矩形。
    Rect {
        /// 发起者提供的矩形。
        rect: Rect,
    },
}

/// 拖动浮动窗口边框时的缩放锚点。
///
/// 角锚点同时改变两个方向，边锚点只改变一个方向；触及左边或上边时窗口原点随之移动。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeAnchor {
    /// 上边;只改变高度与顶部位置。
    Top,

    /// 下边;只改变高度。
    Bottom,

    /// 左边;只改变宽度与左侧位置。
    Left,

    /// 右边;只改变宽度。
    Right,

    /// 左上角。
    TopLeft,

    /// 右上角。
    TopRight,

    /// 左下角。
    BottomLeft,

    /// 右下角。
    BottomRight,
}

impl ResizeAnchor {
    /// 是否触及左边。
    #[must_use]
    pub const fn touches_left(self) -> bool {
        matches!(self, Self::Left | Self::TopLeft | Self::BottomLeft)
    }

    /// 是否触及右边。
    #[must_use]
    pub const fn touches_right(self) -> bool {
        matches!(self, Self::Right | Self::TopRight | Self::BottomRight)
    }

    /// 是否触及上边。
    #[must_use]
    pub const fn touches_top(self) -> bool {
        matches!(self, Self::Top | Self::TopLeft | Self::TopRight)
    }

    /// 是否触及下边。
    #[must_use]
    pub const fn touches_bottom(self) -> bool {
        matches!(self, Self::Bottom | Self::BottomLeft | Self::BottomRight)
    }

    /// 返回全部锚点，按命中优先级排序（角优先于边）。
    #[must_use]
    pub const fn all() -> [Self; 8] {
        [
            Self::TopLeft,
            Self::TopRight,
            Self::BottomLeft,
            Self::BottomRight,
            Self::Top,
            Self::Bottom,
            Self::Left,
            Self::Right,
        ]
    }
}

/// 关闭窗口失败的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowCloseError {
    /// 请求的窗口不存在。
    Unknown,

    /// 主窗口不能关闭。
    Primary,
}

/// 一个窗口：工作流身份、标题与自己的拆分布局。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Window {
    kind: WindowKind,
    title: String,
    layout: LayoutTree,

    placement: WindowPlacement,

    chrome: WindowChrome,

    interaction: WindowInteraction,

    lifetime: WindowLifetime,

    anchor: WindowAnchor,

    modal: bool,
}

impl Window {
    /// 使用已有布局创建窗口。
    #[must_use]
    pub fn new(kind: WindowKind, title: impl Into<String>, layout: LayoutTree) -> Self {
        Self {
            kind,
            title: title.into(),
            layout,
            placement: WindowPlacement::Docked,
            chrome: WindowChrome::Title,
            interaction: WindowInteraction::Interactive,
            lifetime: WindowLifetime::Persistent,
            anchor: WindowAnchor::Frame,
            modal: false,
        }
    }

    /// 返回窗口占据方式。
    #[must_use]
    pub const fn placement(&self) -> WindowPlacement {
        self.placement
    }

    /// 改写窗口占据方式。
    pub const fn set_placement(&mut self, placement: WindowPlacement) {
        self.placement = placement;
    }

    /// 返回是否停靠。
    #[must_use]
    pub const fn is_docked(&self) -> bool {
        self.placement.is_docked()
    }

    /// 返回是否浮动。
    #[must_use]
    pub const fn is_floating(&self) -> bool {
        !self.placement.is_docked()
    }

    /// 返回界面属性集合，供渲染与命中测试使用。
    #[must_use]
    pub const fn chrome(&self) -> WindowChrome {
        self.chrome
    }

    /// 改写界面属性集合。
    pub const fn set_chrome(&mut self, chrome: WindowChrome) {
        self.chrome = chrome;
    }

    /// 返回交互类别。
    #[must_use]
    pub const fn interaction(&self) -> WindowInteraction {
        self.interaction
    }

    /// 改写交互类别。
    pub const fn set_interaction(&mut self, interaction: WindowInteraction) {
        self.interaction = interaction;
    }

    /// 返回是否被动窗口。
    #[must_use]
    pub const fn is_passive(&self) -> bool {
        matches!(self.interaction, WindowInteraction::Passive)
    }

    /// 返回是否进入会话。
    #[must_use]
    pub const fn is_persistent(&self) -> bool {
        matches!(self.lifetime, WindowLifetime::Persistent)
    }

    /// 返回生命周期。
    #[must_use]
    pub const fn lifetime(&self) -> WindowLifetime {
        self.lifetime
    }

    /// 改写生命周期。
    pub const fn set_lifetime(&mut self, lifetime: WindowLifetime) {
        self.lifetime = lifetime;
    }

    /// 返回锚点。
    #[must_use]
    pub const fn anchor(&self) -> WindowAnchor {
        self.anchor
    }

    /// 改写锚点。
    pub const fn set_anchor(&mut self, anchor: WindowAnchor) {
        self.anchor = anchor;
    }

    /// 返回是否模态浮窗。
    #[must_use]
    pub const fn is_modal(&self) -> bool {
        self.modal
    }

    /// 改写模态标记。
    pub const fn set_modal(&mut self, modal: bool) {
        self.modal = modal;
    }

    /// 是否在当前配置下响应标题栏拖动与缩放。
    ///
    /// 只有可交互且带标题框的浮动窗口才允许被拖动或缩放。
    #[must_use]
    pub const fn is_draggable(&self) -> bool {
        self.is_floating()
            && matches!(self.chrome, WindowChrome::Title)
            && matches!(self.interaction, WindowInteraction::Interactive)
    }

    /// 返回浮动矩形的可变引用；停靠窗口没有。
    pub const fn floating_rect_mut(&mut self) -> Option<&mut Rect> {
        match &mut self.placement {
            WindowPlacement::Floating { rect, .. } => Some(rect),
            WindowPlacement::Docked => None,
        }
    }

    /// 按模板创建窗口。
    ///
    /// # Errors
    /// 模板中的面板重复、约束非法或焦点不在模板中时返回布局错误。
    pub fn from_spec(
        kind: WindowKind,
        title: impl Into<String>,
        spec: LayoutSpec,
    ) -> Result<Self, LayoutError> {
        Ok(Self::new(kind, title, LayoutTree::from_spec(spec)?))
    }

    /// 返回窗口类型。
    #[must_use]
    pub const fn kind(&self) -> WindowKind {
        self.kind
    }

    /// 返回标题。
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// 改写标题。
    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    /// 改写窗口类型。
    ///
    /// 会话迁移需要在保留标识、面板与属性的前提下改写工作流类型；新建窗口仍应
    /// 使用对应模板函数而不是事后改写。
    pub const fn set_kind(&mut self, kind: WindowKind) {
        self.kind = kind;
    }

    /// 返回布局。
    #[must_use]
    pub const fn layout(&self) -> &LayoutTree {
        &self.layout
    }

    /// 返回可变布局。
    pub const fn layout_mut(&mut self) -> &mut LayoutTree {
        &mut self.layout
    }

    /// 返回聚焦面板。
    #[must_use]
    pub const fn focused(&self) -> PanelId {
        self.layout.focused()
    }

    /// 校验布局并返回其中的面板。
    ///
    /// # Errors
    /// 布局结构与槽位约束非法时返回布局错误。
    pub fn validate(&self) -> Result<Vec<PanelId>, LayoutError> {
        self.layout.validate()
    }
}

/// Frame 中按顺序排列的窗口集合。
///
/// 集合始终至少包含一个主窗口；主窗口不可关闭，新窗口插入末尾并成为活动窗口。
/// 所有布局操作都作用于活动窗口，非活动窗口保留自己的布局与面板。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowSet {
    windows: SlotMap<WindowId, Window>,
    order: Vec<WindowId>,
    active: WindowId,

    // 由底到顶的绘制顺序；`order` 仍然决定停靠窗口的标签顺序与轮转，
    // 因此浮动窗口获得焦点不会打乱标签条。
    stack: Vec<WindowId>,
}

impl WindowSet {
    /// 使用第一个窗口创建集合；该窗口成为不可关闭的主窗口。
    #[must_use]
    pub fn new(window: Window) -> Self {
        let mut windows = SlotMap::with_key();
        let id = windows.insert(window);
        Self {
            windows,
            order: vec![id],
            active: id,
            stack: vec![id],
        }
    }

    /// 返回主窗口标识。
    #[must_use]
    pub fn primary(&self) -> WindowId {
        self.order[0]
    }

    /// 整体替换主窗口，保留窗口标识与其余窗口。
    ///
    /// 启动阶段先用骨架窗口创建集合，再在应用状态就绪后装配真正的主窗口，
    /// 因此需要在不改变标识的前提下改写主窗口的内容。
    pub fn replace_primary(&mut self, window: Window) -> WindowId {
        let id = self.primary();
        self.windows[id] = window;
        id
    }

    /// 返回窗口数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// 返回是否没有窗口；仅损坏的持久状态会得到该结果。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// 按显示顺序返回窗口标识。
    #[must_use]
    pub fn order(&self) -> &[WindowId] {
        &self.order
    }

    /// 返回活动窗口标识。
    #[must_use]
    pub const fn active_id(&self) -> WindowId {
        self.active
    }

    /// 返回活动窗口。
    #[must_use]
    pub fn active(&self) -> &Window {
        self.get(self.active)
            .expect("The active window always exists in the window set")
    }

    /// 返回活动窗口的可变引用。
    pub fn active_mut(&mut self) -> &mut Window {
        self.get_mut(self.active)
            .expect("The active window always exists in the window set")
    }

    /// 返回指定窗口。
    #[must_use]
    pub fn get(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(id)
    }

    /// 返回指定窗口的可变引用。
    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.get_mut(id)
    }

    /// 按显示顺序迭代窗口。
    pub fn iter(&self) -> impl Iterator<Item = (WindowId, &Window)> {
        self.order
            .iter()
            .filter_map(|id| self.windows.get(*id).map(|window| (*id, window)))
    }

    /// 追加窗口并使其成为活动窗口。
    ///
    /// `Passive` 窗口只进入绘制栈，不进入标签顺序，也不成为活动窗口。
    pub fn insert(&mut self, window: Window) -> WindowId {
        if window.is_passive() {
            return self.insert_passive(window);
        }
        let id = self.windows.insert(window);
        self.stack.push(id);
        self.order.push(id);
        self.active = id;
        id
    }

    /// 追加一个被动窗口：只进入绘制栈，不进入标签顺序，也不改变活动窗口。
    ///
    /// 被动窗口强制为 `Transient`：它不属于任何工作流会话，重启后不恢复。
    /// 被动窗口同时强制为 `Floating`：它按自身矩形叠加显示，不会独占内容区。
    pub fn insert_passive(&mut self, window: Window) -> WindowId {
        let mut window = window;
        window.set_lifetime(WindowLifetime::Transient);
        if window.is_docked() {
            let rect = window.placement().rect().unwrap_or_default();
            window.set_placement(WindowPlacement::Floating { rect, z: 1 });
        }
        let id = self.windows.insert(window);
        self.stack.push(id);
        id
    }

    /// 把窗口提升到绘制栈顶；被动窗口也可提升，但不改变活动窗口。
    fn raise(&mut self, id: WindowId) {
        if let Some(position) = self.stack.iter().position(|candidate| *candidate == id) {
            self.stack.remove(position);
        }
        self.stack.push(id);
    }

    /// 把被动窗口提升到绘制栈顶，不改变活动窗口。
    pub fn raise_passive(&mut self, id: WindowId) -> bool {
        let Some(window) = self.windows.get(id) else {
            return false;
        };
        if !window.is_passive() {
            return false;
        }
        self.raise(id);
        true
    }

    /// 移除一个被动窗口；非被动窗口返回 `false`。
    ///
    /// 被动窗口不参与标签顺序，只从绘制栈与窗口表中移除。
    pub fn close_passive(&mut self, id: WindowId) -> bool {
        let Some(window) = self.windows.get(id) else {
            return false;
        };
        if !window.is_passive() {
            return false;
        }
        self.stack.retain(|candidate| *candidate != id);
        self.windows.remove(id);
        true
    }

    /// 按由底到顶的绘制顺序返回窗口标识。
    #[must_use]
    pub fn stack(&self) -> &[WindowId] {
        &self.stack
    }

    /// 返回停靠窗口标识，按标签顺序排列。
    pub fn docked(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.order
            .iter()
            .copied()
            .filter(|id| self.windows.get(*id).is_some_and(Window::is_docked))
    }

    /// 返回浮动窗口标识，按 `z` 由低到高排列；`z` 相同时保持绘制栈顺序。
    #[must_use]
    pub fn floating(&self) -> Vec<WindowId> {
        let mut ids = self
            .stack
            .iter()
            .copied()
            .filter(|id| self.windows.get(*id).is_some_and(Window::is_floating))
            .collect::<Vec<_>>();
        ids.sort_by_key(|id| match self.windows.get(*id).map(Window::placement) {
            Some(WindowPlacement::Floating { z, .. }) => z,
            _ => 0,
        });
        ids
    }

    /// 按绘制顺序返回所有窗口，浮动窗口在停靠窗口之上。
    #[must_use]
    pub fn paint_order(&self) -> Vec<WindowId> {
        let mut ids = self.docked().collect::<Vec<_>>();
        ids.extend(self.floating());
        ids
    }

    /// 把窗口设为浮动，并置于浮动层级最高处。
    ///
    /// 已是浮动窗口时保留原矩形，只提升 `z`。
    pub fn set_floating(&mut self, id: WindowId, rect: Rect) -> bool {
        let top = self
            .windows
            .iter()
            .filter_map(|(_, window)| match window.placement() {
                WindowPlacement::Floating { z, .. } => Some(z),
                WindowPlacement::Docked => None,
            })
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let Some(window) = self.windows.get_mut(id) else {
            return false;
        };
        window.set_placement(WindowPlacement::Floating { rect, z: top });
        self.raise(id);
        true
    }

    /// 把窗口设为停靠；保留浮动矩形以便再次浮动。
    pub fn set_docked(&mut self, id: WindowId) -> bool {
        let Some(window) = self.windows.get_mut(id) else {
            return false;
        };
        window.set_placement(WindowPlacement::Docked);
        true
    }

    /// 切换停靠与浮动。
    ///
    /// 转为浮动时使用 `rect` 作为初始矩形。返回切换后的窗口是否处于浮动状态。
    pub fn toggle_placement(&mut self, id: WindowId, rect: Rect) -> Option<bool> {
        let window = self.windows.get(id)?;
        if window.is_floating() {
            self.set_docked(id);
            Some(false)
        } else {
            let rect = window.placement.rect().unwrap_or(rect);
            self.set_floating(id, rect);
            Some(true)
        }
    }

    /// 改写浮动窗口矩形；停靠窗口返回 `false`。
    pub fn set_floating_rect(&mut self, id: WindowId, rect: Rect) -> bool {
        match self.windows.get_mut(id).and_then(Window::floating_rect_mut) {
            Some(slot) => {
                *slot = rect;
                true
            }
            None => false,
        }
    }

    /// 按指针位移移动浮动窗口，并钳制到内容区内。
    ///
    /// `minimum` 是浮动窗口的最小宽高；命中该下界时窗口转为停靠。返回移动后的矩形。
    pub fn move_floating(
        &mut self,
        id: WindowId,
        dx: i32,
        dy: i32,
        bounds: Rect,
        minimum: (u16, u16),
    ) -> Option<Rect> {
        let window = self.windows.get(id)?;
        let mut rect = window.placement.rect()?;
        rect.x = clamp_axis(
            i32::from(rect.x).saturating_add(dx),
            bounds.x,
            bounds
                .x
                .saturating_add(bounds.width)
                .saturating_sub(rect.width),
        );
        rect.y = clamp_axis(
            i32::from(rect.y).saturating_add(dy),
            bounds.y,
            bounds
                .y
                .saturating_add(bounds.height)
                .saturating_sub(rect.height),
        );
        if rect.width < minimum.0 || rect.height < minimum.1 {
            self.set_docked(id);
            return None;
        }
        self.set_floating_rect(id, rect);
        Some(rect)
    }

    /// 拖动浮动窗口右下角改变尺寸，并钳制到内容区内。
    ///
    /// `minimum` 是浮动窗口的最小宽高；缩到该下界以下时窗口转为停靠。返回新矩形。
    pub fn resize_floating(
        &mut self,
        id: WindowId,
        dx: i32,
        dy: i32,
        bounds: Rect,
        minimum: (u16, u16),
    ) -> Option<Rect> {
        self.resize_floating_anchored(id, ResizeAnchor::BottomRight, dx, dy, bounds, minimum)
    }

    /// 按拖动锚点改变浮动窗口尺寸，并钳制到内容区内。
    ///
    /// 触及左边或上边时窗口原点同向移动、尺寸反向变化；`minimum` 是最小宽高，
    /// 缩到该下界以下时窗口转为停靠。返回新矩形；窗口非浮动时返回 `None`。
    pub fn resize_floating_anchored(
        &mut self,
        id: WindowId,
        anchor: ResizeAnchor,
        dx: i32,
        dy: i32,
        bounds: Rect,
        minimum: (u16, u16),
    ) -> Option<Rect> {
        let window = self.windows.get(id)?;
        let mut rect = window.placement.rect()?;
        if anchor.touches_left() {
            // 向左不能越出内容区，向右不能小于最小宽度；两个方向都改变原点与宽度。
            let lower = -i32::from(rect.x.saturating_sub(bounds.x));
            let upper = i32::from(rect.width.saturating_sub(minimum.0));
            // 先满足最小尺寸，再满足内容区边界：两者冲突时以最小尺寸为准（越界后由调用方
            // 的钳制与停靠规则收尾）。
            let dx = dx.min(upper).max(lower);
            rect.x = i32::from(rect.x).saturating_add(dx).max(0) as u16;
            rect.width = clamp_axis(
                i32::from(rect.width).saturating_sub(dx),
                minimum.0,
                bounds.width,
            );
        }
        if anchor.touches_right() {
            let max_width = bounds.x.saturating_add(bounds.width).saturating_sub(rect.x);
            rect.width = clamp_axis(
                i32::from(rect.width).saturating_add(dx),
                minimum.0,
                max_width.max(minimum.0),
            );
        }
        if anchor.touches_top() {
            let lower = -i32::from(rect.y.saturating_sub(bounds.y));
            let upper = i32::from(rect.height.saturating_sub(minimum.1));
            let dy = dy.min(upper).max(lower);
            rect.y = i32::from(rect.y).saturating_add(dy).max(0) as u16;
            rect.height = clamp_axis(
                i32::from(rect.height).saturating_sub(dy),
                minimum.1,
                bounds.height,
            );
        }
        if anchor.touches_bottom() {
            let max_height = bounds
                .y
                .saturating_add(bounds.height)
                .saturating_sub(rect.y);
            rect.height = clamp_axis(
                i32::from(rect.height).saturating_add(dy),
                minimum.1,
                max_height.max(minimum.1),
            );
        }
        if rect.width < minimum.0 || rect.height < minimum.1 {
            self.set_docked(id);
            return None;
        }
        self.set_floating_rect(id, rect);
        Some(rect)
    }

    /// 聚焦指定窗口。
    ///
    /// `Passive` 窗口不接收焦点，返回 `false`。
    pub fn select(&mut self, id: WindowId) -> bool {
        let Some(window) = self.windows.get(id) else {
            return false;
        };
        if window.is_passive() {
            return false;
        }
        self.raise(id);
        self.active = id;
        true
    }

    /// 按从 1 开始的序号聚焦窗口。
    pub fn select_index(&mut self, index: usize) -> bool {
        match index.checked_sub(1).and_then(|index| self.order.get(index)) {
            Some(id) => self.select(*id),
            None => false,
        }
    }

    /// 聚焦下一个窗口。
    pub fn select_next(&mut self) -> bool {
        self.cycle(1)
    }

    /// 聚焦上一个窗口。
    pub fn select_previous(&mut self) -> bool {
        self.cycle(self.order.len().saturating_sub(1))
    }

    fn cycle(&mut self, offset: usize) -> bool {
        if self.order.len() < 2 {
            return false;
        }
        let position = self
            .order
            .iter()
            .position(|id| *id == self.active)
            .unwrap_or_default();
        let next = (position + offset) % self.order.len();
        self.active = self.order[next];
        true
    }

    /// 关闭窗口；主窗口与未知标识被拒绝。
    ///
    /// 关闭活动窗口后聚焦顺序中的相邻窗口。
    ///
    /// # Errors
    /// 未知窗口返回 [`WindowCloseError::Unknown`]，主窗口返回 [`WindowCloseError::Primary`]。
    pub fn close(&mut self, id: WindowId) -> Result<(), WindowCloseError> {
        if !self.windows.contains_key(id) {
            return Err(WindowCloseError::Unknown);
        }
        if id == self.primary() {
            return Err(WindowCloseError::Primary);
        }
        let position = self
            .order
            .iter()
            .position(|candidate| *candidate == id)
            .ok_or(WindowCloseError::Unknown)?;
        self.order.remove(position);
        self.stack.retain(|candidate| *candidate != id);
        self.windows.remove(id);
        if self.active == id {
            let next = position.min(self.order.len().saturating_sub(1));
            self.active = self.order[next];
        }
        Ok(())
    }

    /// 关闭一个窗口及其浮动/瞬态后代。
    ///
    /// 归属窗口消失后，其模态与被动浮窗不再有意义，因此随主窗口一起关闭。
    /// 主窗口仍然不可关闭。
    ///
    /// # Errors
    /// 与 [`WindowSet::close`] 相同。
    pub fn close_with_dependents(&mut self, id: WindowId) -> Result<usize, WindowCloseError> {
        if id == self.primary() {
            return Err(WindowCloseError::Primary);
        }
        if !self.windows.contains_key(id) {
            return Err(WindowCloseError::Unknown);
        }
        let mut removed = 0;
        for candidate in self.paint_order() {
            if candidate == id {
                continue;
            }
            let transient = self
                .windows
                .get(candidate)
                .is_some_and(|window| !window.is_persistent());
            if transient {
                self.close_cascade(candidate);
                removed += 1;
            }
        }
        self.close_cascade(id);
        Ok(removed + 1)
    }

    /// 不校验主窗口身份地移除窗口并保持顺序、栈与活动窗口自洽。
    fn close_cascade(&mut self, id: WindowId) {
        let Some(position) = self.order.iter().position(|candidate| *candidate == id) else {
            self.stack.retain(|candidate| *candidate != id);
            self.windows.remove(id);
            return;
        };
        self.order.remove(position);
        self.stack.retain(|candidate| *candidate != id);
        self.windows.remove(id);
        if self.active == id {
            let next = position.min(self.order.len().saturating_sub(1));
            if let Some(id) = self.order.get(next) {
                self.active = *id;
            }
        }
    }

    /// 丢弃所有瞬态窗口，使会话只序列化 `Persistent` 窗口。
    ///
    /// 主窗口一定保留。返回丢弃的窗口数量。
    pub fn retain_persistent(&mut self) -> usize {
        let transient = self
            .stack
            .iter()
            .copied()
            .filter(|id| {
                self.windows
                    .get(*id)
                    .is_some_and(|window| !window.is_persistent())
            })
            .collect::<Vec<_>>();
        let removed = transient.len();
        for id in transient {
            self.close_cascade(id);
        }
        // 被动但持久的窗口不进标签顺序；这里保证活动窗口仍然有效。
        if !self.order.contains(&self.active) {
            self.active = self.primary();
        }
        removed
    }

    /// 校验窗口顺序、活动标识与每个窗口的布局，并返回每窗口的面板。
    ///
    /// # Errors
    /// 顺序为空、活动窗口缺失、布局非法或面板重复时返回布局错误。
    ///
    /// 校验以“可达性”为准：标签顺序或绘制栈中的窗口都必须真实存在，两者共同覆盖
    /// 窗口集合；额外的被动窗口只进入绘制栈。
    pub fn validate(&self) -> Result<Vec<(WindowId, Vec<PanelId>)>, LayoutError> {
        if self.order.is_empty() || !self.order.contains(&self.active) {
            return Err(LayoutError::InvalidStructure);
        }
        let mut ids = Vec::with_capacity(self.windows.len());
        let mut seen = std::collections::HashSet::new();
        for id in self.order.iter().chain(self.stack.iter()) {
            if seen.insert(*id) {
                ids.push(*id);
            }
        }
        let mut panels = std::collections::HashSet::new();
        let mut validated = Vec::with_capacity(ids.len());
        for id in ids {
            let window = self.windows.get(id).ok_or(LayoutError::InvalidStructure)?;
            let window_panels = window.validate()?;
            if window_panels.iter().any(|panel| !panels.insert(*panel)) {
                return Err(LayoutError::InvalidStructure);
            }
            validated.push((id, window_panels));
        }
        Ok(validated)
    }

    /// 返回包含指定面板的窗口。
    #[must_use]
    pub fn window_of(&self, panel: PanelId) -> Option<WindowId> {
        self.iter()
            .find(|(_, window)| {
                window
                    .validate()
                    .is_ok_and(|panels| panels.contains(&panel))
            })
            .map(|(id, _)| id)
    }

    /// 返回指定面板是否仍属于某个窗口。
    #[must_use]
    pub fn contains_panel(&self, panel: PanelId) -> bool {
        self.window_of(panel).is_some()
    }
}

impl WindowSet {
    /// 返回活动窗口的聚焦面板。
    #[must_use]
    pub fn focused(&self) -> PanelId {
        self.active().focused()
    }

    /// 在活动窗口中聚焦面板。
    ///
    /// # Errors
    /// 面板不属于活动窗口时返回布局错误。
    pub fn focus(&mut self, panel: PanelId) -> Result<(), LayoutError> {
        self.active_mut().layout_mut().focus(panel)
    }

    /// 在活动窗口中拆分面板。
    ///
    /// # Errors
    /// 目标面板不存在时返回布局错误。
    pub fn split(
        &mut self,
        target: PanelId,
        new_panel: PanelId,
        axis: crate::layout::SplitAxis,
    ) -> Result<(), LayoutError> {
        self.active_mut()
            .layout_mut()
            .split(target, new_panel, axis)
    }

    /// 关闭活动窗口中的面板。
    ///
    /// # Errors
    /// 面板是唯一面板时返回布局错误。
    pub fn close_panel(&mut self, panel: PanelId) -> Result<(), LayoutError> {
        self.active_mut().layout_mut().close(panel)
    }

    /// 计算活动窗口的叶节点矩形。
    #[must_use]
    pub fn rectangles(&self, area: Rect) -> Vec<(PanelId, Rect)> {
        self.active().layout().rectangles(area)
    }

    /// 在活动窗口中按方向聚焦。
    pub fn focus_direction(&mut self, direction: Direction, area: Rect) -> Option<PanelId> {
        self.active_mut()
            .layout_mut()
            .focus_direction(direction, area)
    }

    /// 查找活动窗口中的分隔条。
    #[must_use]
    pub fn divider_at(&self, area: Rect, x: u16, y: u16) -> Option<SplitDivider> {
        self.active().layout().divider_at(area, x, y)
    }

    /// 拖动活动窗口中的分隔条。
    #[must_use]
    pub fn panel_size(&self, panel: PanelId) -> Option<crate::layout::PanelSize> {
        self.active().layout().panel_size(panel)
    }

    /// 改写活动窗口中包含此面板的槽位约束。
    ///
    /// # Errors
    /// 面板不属于活动窗口或约束非法时返回布局错误。
    pub fn set_panel_size(
        &mut self,
        panel: PanelId,
        size: crate::layout::PanelSize,
    ) -> Result<(), LayoutError> {
        self.active_mut().layout_mut().set_panel_size(panel, size)
    }

    /// 按指针位置拖动活动窗口中的分隔条。
    ///
    /// 传入分隔条身份、活动窗口内容区与当前指针坐标；`minimum_width`/`minimum_height`
    /// 是两个相邻面板的最小尺寸。只有可伸缩一侧的槽位约束会被改写，固定单元格或百分比
    /// 的一侧保持原约束，因此窗口大小变化后布局仍然稳定。返回是否实际改写了布局。
    #[must_use]
    pub fn resize_divider(
        &mut self,
        divider: SplitDivider,
        area: Rect,
        x: u16,
        y: u16,
        minimum_width: u16,
        minimum_height: u16,
    ) -> bool {
        self.active_mut().layout_mut().resize_divider(
            divider,
            area,
            x,
            y,
            minimum_width,
            minimum_height,
        )
    }
}

/// 把坐标限制到闭区间内，并转换为终端单元。
fn clamp_axis(value: i32, minimum: u16, maximum: u16) -> u16 {
    let maximum = maximum.max(minimum);
    let clamped = value.clamp(i32::from(minimum), i32::from(maximum));
    u16::try_from(clamped).unwrap_or(minimum)
}

#[cfg(test)]
mod tests {
    use slotmap::KeyData;

    use super::{Window, WindowCloseError, WindowKind, WindowSet};
    use crate::layout::{LayoutTree, PanelId, Rect};

    fn panel(id: u64) -> PanelId {
        PanelId::from(KeyData::from_ffi(id))
    }

    fn window(kind: WindowKind, id: u64) -> Window {
        Window::new(kind, kind.default_title(), LayoutTree::new(panel(id)))
    }

    #[test]
    fn keeps_the_primary_window_and_cycles_between_windows() {
        for kind in [
            WindowKind::Agent,
            WindowKind::Debug,
            WindowKind::Editor,
            WindowKind::Plan,
            WindowKind::Diagnostics,
            WindowKind::Diff,
            WindowKind::Tests,
        ] {
            assert_eq!(WindowKind::parse(kind.as_str()), Some(kind));
            assert!(!kind.default_title().is_empty());
        }

        let mut windows = WindowSet::new(window(WindowKind::Editor, 1));
        let primary = windows.primary();
        let second = windows.insert(window(WindowKind::Debug, 3));
        assert_eq!(windows.len(), 2);
        assert_eq!(windows.active_id(), second);
        assert_eq!(windows.close(primary), Err(WindowCloseError::Primary));
        assert!(windows.select_next());
        assert_eq!(windows.active_id(), primary);
        assert!(windows.select_previous());
        assert_eq!(windows.active_id(), second);
        assert!(windows.select_index(1));
        assert_eq!(windows.active_id(), primary);
        assert!(!windows.select_index(3));
        assert_eq!(windows.close(second), Ok(()));
        assert_eq!(windows.len(), 1);
        assert_eq!(windows.active_id(), primary);
    }

    #[test]
    fn routes_layout_operations_to_the_active_window() {
        let mut windows = WindowSet::new(window(WindowKind::Editor, 1));
        let first = panel(1);
        let second = panel(3);
        windows
            .split(first, second, crate::layout::SplitAxis::Rows)
            .expect("split the active window");
        assert_eq!(windows.focused(), second);
        assert_eq!(
            windows.validate().map(|windows| windows.len()),
            Ok(1),
            "only one window holds panels"
        );
        assert_eq!(windows.window_of(second), Some(windows.active_id()));
        windows.focus(first).expect("focus the first panel");
        assert_eq!(windows.focused(), first);

        // 非活动窗口保留自己的布局，切换后焦点随之改变。
        let other = windows.insert(window(WindowKind::Agent, 5));
        assert_eq!(windows.focused(), panel(5));
        assert!(windows.select(windows.primary()));
        assert_eq!(windows.focused(), first);
        assert!(windows.select(other));
        assert_eq!(windows.focused(), panel(5));
    }

    #[test]
    fn rejects_unknown_windows_and_duplicate_panels() {
        let mut windows = WindowSet::new(window(WindowKind::Editor, 1));
        assert_eq!(
            windows.close(windows.primary()),
            Err(WindowCloseError::Primary)
        );
        let primary = windows.primary();
        assert_eq!(windows.close(primary), Err(WindowCloseError::Primary));
        assert!(!windows.select(slotmap::KeyData::from_ffi(7).into()));

        // 两个窗口声明同一面板时必须整体拒绝。
        let duplicate = Window::new(WindowKind::Debug, "duplicate", LayoutTree::new(panel(1)));
        windows.insert(duplicate);
        assert!(windows.validate().is_err());
    }

    #[test]
    fn anchored_resize_moves_origin_for_left_and_top_edges() {
        use crate::window::{ResizeAnchor, WindowPlacement};

        let mut windows = WindowSet::new(window(WindowKind::Editor, 1));
        let id = windows.primary();
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        };
        let start = Rect {
            x: 20,
            y: 10,
            width: 40,
            height: 12,
        };
        assert!(windows.set_floating(id, start));

        // 拖左上角：原点上移、尺寸相应增大。
        let grown = windows
            .resize_floating_anchored(id, ResizeAnchor::TopLeft, -5, -3, bounds, (10, 4))
            .expect("floating window");
        assert_eq!(grown.x, 15);
        assert_eq!(grown.y, 7);
        assert_eq!(grown.width, 45);
        assert_eq!(grown.height, 15);

        // 拖左边界向右收窄：原点右移、宽度减小。
        let narrowed = windows
            .resize_floating_anchored(id, ResizeAnchor::Left, 5, 0, bounds, (10, 4))
            .expect("floating window");
        assert_eq!(narrowed.x, 20);
        assert_eq!(narrowed.width, 40);
        assert_eq!(narrowed.y, 7);
        assert_eq!(narrowed.height, 15);

        // 拖右边界只改宽度，高度与原点不变。
        let widened = windows
            .resize_floating_anchored(id, ResizeAnchor::Right, 8, 0, bounds, (10, 4))
            .expect("floating window");
        assert_eq!(widened.width, 48);
        assert_eq!(widened.x, 20);
        assert_eq!(widened.height, 15);

        // 最小尺寸无法满足时转为停靠：把最小宽度抬到当前宽度之上。
        let docked =
            windows.resize_floating_anchored(id, ResizeAnchor::Top, 0, 1, bounds, (100, 4));
        assert!(docked.is_none());
        assert!(matches!(
            windows.get(id).map(Window::placement),
            Some(WindowPlacement::Docked)
        ));
    }

    #[test]
    fn floating_windows_keep_their_own_rectangle_and_stack_order() {
        use crate::window::{WindowChrome, WindowInteraction, WindowLifetime, WindowPlacement};

        let mut windows = WindowSet::new(window(WindowKind::Editor, 1));
        let primary = windows.primary();
        let rect = Rect {
            x: 4,
            y: 3,
            width: 40,
            height: 12,
        };
        assert!(windows.set_floating(primary, rect));
        assert!(windows.get(primary).is_some_and(Window::is_floating));
        assert_eq!(windows.floating(), vec![primary]);
        // 浮动的被动窗口只进入绘制栈，不进入标签顺序。
        let mut popup = window(WindowKind::Editor, 7);
        popup.set_interaction(WindowInteraction::Passive);
        popup.set_lifetime(WindowLifetime::Transient);
        popup.set_chrome(WindowChrome::None);
        let popup_id = windows.insert(popup);
        assert_eq!(windows.stack().last(), Some(&popup_id));
        assert_eq!(windows.docked().count(), 0);
        assert!(
            !windows.select(popup_id),
            "passive windows never take focus"
        );

        // 移动与缩放都钳制在内容区内，并可确定性回退到停靠。
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 30,
        };
        let moved = windows
            .move_floating(primary, 10, 100, bounds, (40, 10))
            .expect("floating windows can be moved");
        assert!(moved.y + moved.height <= bounds.height);
        assert!(
            windows
                .resize_floating(primary, 5, 5, bounds, (40, 10))
                .is_some()
        );
        assert!(windows.set_docked(primary));
        assert!(matches!(
            windows.get(primary).map(Window::placement),
            Some(WindowPlacement::Docked)
        ));

        // 关掉浮层窗口后，瞬态窗口被清理，持久窗口保留。
        assert_eq!(windows.retain_persistent(), 1);
        assert!(windows.get(popup_id).is_none());
        assert_eq!(windows.len(), 1);
    }

    #[test]
    fn closing_a_window_drops_transient_dependents() {
        use crate::window::{WindowInteraction, WindowLifetime};

        let mut windows = WindowSet::new(window(WindowKind::Editor, 1));
        let second = windows.insert(window(WindowKind::Debug, 3));
        let mut popup = window(WindowKind::Editor, 5);
        popup.set_interaction(WindowInteraction::Passive);
        popup.set_lifetime(WindowLifetime::Transient);
        // 被动窗口必须经 `insert_passive` 登记：它只进入绘制栈，不参与标签顺序。
        let popup_id = windows.insert_passive(popup);
        assert!(windows.stack().contains(&popup_id));
        assert!(windows.get(popup_id).is_some_and(|w| !w.is_persistent()));
        // 浮层窗口是瞬态的：关闭工作流窗口时一并丢弃，避免留下无主弹层。
        assert_eq!(windows.close_with_dependents(second), Ok(2));
        assert!(windows.get(second).is_none());
        assert!(windows.get(popup_id).is_none());
    }
}
