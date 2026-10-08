//! 带稳定面板标识的拆分布局树。
//!
//! 布局只保存空间关系，不拥有面板实例；上层必须同步维护面板存储与布局中的叶节点。
//! 几何分配和分隔线命中共用同一棵树，避免视觉边界与交互边界不一致。

use serde::{Deserialize, Serialize};
use slotmap::{SlotMap, new_key_type};
use thiserror::Error;

new_key_type! {
    /// 一个面板实例的稳定标识。
    pub struct PanelId;
    /// 一个布局节点的稳定标识。
    pub struct LayoutId;
}

/// 整数终端矩形。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    /// 左侧列。
    pub x: u16,

    /// 顶部行。
    pub y: u16,

    /// 以单元格计的宽度。
    pub width: u16,

    /// 以单元格计的高度。
    pub height: u16,
}

impl Rect {
    /// 返回绝对终端单元格是否位于此矩形内。
    #[must_use]
    pub const fn contains(self, x: u16, y: u16) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(self.width)
            && y < self.y.saturating_add(self.height)
    }
}

/// 布局容器的拆分方向。
///
/// `Rows` 让子节点获得连续的上下带，`Columns` 让子节点左右并排。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SplitAxis {
    /// 子节点上下排列。
    Rows,

    /// 子节点左右排列。
    Columns,
}

/// 一个面板槽位在父容器主轴上的长度语义。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Length {
    /// 按权重分配固定约束之外的剩余空间。
    Fill(u16),

    /// 固定终端单元格数量。
    Cells(u16),

    /// 父容器主轴长度的百分比。
    Percent(u16),
}

/// 一个面板槽位的长度与边界约束。
///
/// `max` 为 0 表示没有上限。约束无法同时满足时布局优先保证铺满父容器，
/// 不会为了边界在父容器内留下空洞。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelSize {
    /// 主轴长度语义。
    pub length: Length,

    /// 最小长度；0 表示不设下限。
    pub min: u16,

    /// 最大长度；0 表示不设上限。
    pub max: u16,
}

impl PanelSize {
    /// 按权重填充剩余空间。
    #[must_use]
    pub const fn fill(weight: u16) -> Self {
        Self {
            length: Length::Fill(weight),
            min: 0,
            max: 0,
        }
    }

    /// 固定为给定终端单元格数量。
    #[must_use]
    pub const fn cells(cells: u16) -> Self {
        Self {
            length: Length::Cells(cells),
            min: 0,
            max: 0,
        }
    }

    /// 固定为父容器主轴长度的百分比。
    #[must_use]
    pub const fn percent(percent: u16) -> Self {
        Self {
            length: Length::Percent(percent),
            min: 0,
            max: 0,
        }
    }

    /// 设置最小长度。
    #[must_use]
    pub const fn with_min(mut self, min: u16) -> Self {
        self.min = min;
        self
    }

    /// 设置最大长度；0 表示不设上限。
    #[must_use]
    pub const fn with_max(mut self, max: u16) -> Self {
        self.max = max;
        self
    }

    /// 返回约束是否自洽；长度本身必须为正，上下限不得互换。
    fn is_valid(self) -> bool {
        let length_ok = match self.length {
            Length::Fill(weight) => weight > 0,
            Length::Cells(cells) => cells > 0,
            Length::Percent(percent) => percent > 0 && percent <= 100,
        };
        length_ok && (self.max == 0 || self.min <= self.max)
    }

    /// 返回把此约束应用到给定可用长度后的首选长度。
    fn preferred(self, available: u16) -> u16 {
        let raw = match self.length {
            // 权重在后续按剩余空间重新分配；此处返回权重本身以免丢失信息。
            Length::Fill(weight) => weight,
            Length::Cells(cells) => cells,
            Length::Percent(percent) => {
                u16::try_from((u32::from(available) * u32::from(percent)) / 100).unwrap_or(u16::MAX)
            }
        };
        self.clamp(raw)
    }

    /// 把长度限制到声明的上下限内。
    fn clamp(self, value: u16) -> u16 {
        let value = value.max(self.min);
        if self.max == 0 {
            value
        } else {
            value.min(self.max)
        }
    }

    /// 返回此槽位的填充权重；固定长度返回 0。
    fn weight(self) -> u16 {
        match self.length {
            Length::Fill(weight) => weight,
            Length::Cells(_) | Length::Percent(_) => 0,
        }
    }
}

/// 几何焦点方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// 聚焦上方最近的面板。
    Up,

    /// 聚焦下方最近的面板。
    Down,

    /// 聚焦左侧最近的面板。
    Left,

    /// 聚焦右侧最近的面板。
    Right,
}

/// 拆分中相邻子节点边界的稳定句柄。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitDivider {
    split: LayoutId,
    before: usize,
    axis: SplitAxis,
}

impl SplitDivider {
    /// 返回拖动改变分隔条位置的方向。
    #[must_use]
    pub const fn axis(self) -> SplitAxis {
        self.axis
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum LayoutNode {
    Leaf {
        panel: PanelId,
        parent: Option<LayoutId>,
    },
    Split {
        axis: SplitAxis,
        children: Vec<LayoutId>,
        sizes: Vec<PanelSize>,
        parent: Option<LayoutId>,
    },
}

/// 从模板一次性构造布局树的节点描述。
///
/// 叶节点只声明面板身份，槽位约束由父节点的 `children` 携带；根叶节点没有约束。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeSpec {
    /// 一个面板叶节点。
    Leaf(PanelId),

    /// 一个拆分容器；每个子项携带自己在父容器主轴上的约束。
    Split {
        /// 拆分方向。
        axis: SplitAxis,

        /// 按显示顺序排列的子节点与槽位约束。
        children: Vec<(PanelSize, NodeSpec)>,
    },
}

/// 一次性构造布局树的完整描述。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutSpec {
    /// 初始聚焦的面板。
    pub focused: PanelId,

    /// 根节点。
    pub root: NodeSpec,
}

impl NodeSpec {
    /// 返回此模板包含的面板，按深度优先顺序排列。
    fn panels(&self, output: &mut Vec<PanelId>) {
        match self {
            Self::Leaf(panel) => output.push(*panel),
            Self::Split { children, .. } => {
                for (_, child) in children {
                    child.panels(output);
                }
            }
        }
    }

    /// 返回此模板的槽位约束是否全部合法。
    fn sizes(&self) -> Option<()> {
        match self {
            Self::Leaf(_) => Some(()),
            Self::Split { children, .. } => {
                if children.len() < 2 {
                    return None;
                }
                for (size, child) in children {
                    if !size.is_valid() {
                        return None;
                    }
                    child.sizes()?;
                }
                Some(())
            }
        }
    }
}

impl LayoutNode {
    const fn parent(&self) -> Option<LayoutId> {
        match self {
            Self::Leaf { parent, .. } | Self::Split { parent, .. } => *parent,
        }
    }

    fn set_parent(&mut self, new_parent: Option<LayoutId>) {
        match self {
            Self::Leaf { parent, .. } | Self::Split { parent, .. } => *parent = new_parent,
        }
    }
}

/// 无效的面板或布局修改。
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum LayoutError {
    /// 持久化结构含有无效标识、链接、权重或过大深度。
    #[error("The layout structure is invalid")]
    InvalidStructure,

    /// 槽位长度约束自身矛盾或越界。
    #[error("The layout size constraint is invalid")]
    InvalidSize,

    /// 请求的面板不存在。
    #[error("unknown panel")]
    UnknownPanel,

    /// 不退出应用就不能关闭唯一面板。
    #[error("Unable to close last panel")]
    LastPanel,
}

/// 支持几何焦点导航的可序列化拆分树。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayoutTree {
    nodes: SlotMap<LayoutId, LayoutNode>,
    root: LayoutId,
    focused: PanelId,
}

impl LayoutTree {
    /// 在递归渲染前迭代验证不可信持久化链接，并返回唯一面板 ID。
    pub fn validate(&self) -> Result<Vec<PanelId>, LayoutError> {
        let mut visited = std::collections::HashSet::new();
        let mut panels = std::collections::HashSet::new();
        let mut stack = vec![(self.root, None, 0_usize)];
        while let Some((id, parent, depth)) = stack.pop() {
            let node = self.nodes.get(id).ok_or(LayoutError::InvalidStructure)?;
            if !visited.insert(id)
                || depth > 128
                || visited.len() > 16384
                || node.parent() != parent
            {
                return Err(LayoutError::InvalidStructure);
            }
            match node {
                LayoutNode::Leaf { panel, .. } => {
                    if !panels.insert(*panel) {
                        return Err(LayoutError::InvalidStructure);
                    }
                }
                LayoutNode::Split {
                    children, sizes, ..
                } => {
                    if children.len() < 2
                        || children.len() != sizes.len()
                        || sizes.iter().any(|size| !size.is_valid())
                    {
                        return Err(LayoutError::InvalidStructure);
                    }
                    stack.extend(children.iter().map(|child| (*child, Some(id), depth + 1)));
                }
            }
        }
        if visited.len() != self.nodes.len() || !panels.contains(&self.focused) {
            return Err(LayoutError::InvalidStructure);
        }
        Ok(panels.into_iter().collect())
    }

    /// 创建包含一个聚焦面板的布局。
    #[must_use]
    pub fn new(panel: PanelId) -> Self {
        let mut nodes = SlotMap::with_key();
        let root = nodes.insert(LayoutNode::Leaf {
            panel,
            parent: None,
        });
        Self {
            nodes,
            root,
            focused: panel,
        }
    }

    /// 返回聚焦面板。
    #[must_use]
    pub const fn focused(&self) -> PanelId {
        self.focused
    }

    /// 按模板构造布局树。
    ///
    /// 模板中的面板必须唯一，槽位约束必须自洽；失败时不产生部分布局。
    ///
    /// # Errors
    /// 面板重复、子节点少于两个、约束非法或聚焦面板不在模板中时返回
    /// [`LayoutError::InvalidStructure`] 或 [`LayoutError::InvalidSize`]。
    pub fn from_spec(spec: LayoutSpec) -> Result<Self, LayoutError> {
        let mut panels = Vec::new();
        spec.root.panels(&mut panels);
        let unique = panels.iter().collect::<std::collections::HashSet<_>>();
        if panels.is_empty() || unique.len() != panels.len() || !panels.contains(&spec.focused) {
            return Err(LayoutError::InvalidStructure);
        }
        if spec.root.sizes().is_none() {
            return Err(LayoutError::InvalidSize);
        }
        let mut nodes = SlotMap::with_key();
        let root = Self::build_spec_node(&mut nodes, spec.root, None)?;
        Ok(Self {
            nodes,
            root,
            focused: spec.focused,
        })
    }

    fn build_spec_node(
        nodes: &mut SlotMap<LayoutId, LayoutNode>,
        spec: NodeSpec,
        parent: Option<LayoutId>,
    ) -> Result<LayoutId, LayoutError> {
        match spec {
            NodeSpec::Leaf(panel) => Ok(nodes.insert(LayoutNode::Leaf { panel, parent })),
            NodeSpec::Split { axis, children } => {
                if children.len() < 2 {
                    return Err(LayoutError::InvalidStructure);
                }
                let id = nodes.insert(LayoutNode::Split {
                    axis,
                    children: Vec::new(),
                    sizes: Vec::new(),
                    parent,
                });
                let mut child_ids = Vec::with_capacity(children.len());
                let mut sizes = Vec::with_capacity(children.len());
                for (size, child) in children {
                    if !size.is_valid() {
                        return Err(LayoutError::InvalidSize);
                    }
                    child_ids.push(Self::build_spec_node(nodes, child, Some(id))?);
                    sizes.push(size);
                }
                let LayoutNode::Split {
                    children,
                    sizes: node_sizes,
                    ..
                } = &mut nodes[id]
                else {
                    unreachable!("The split node was just inserted")
                };
                *children = child_ids;
                *node_sizes = sizes;
                Ok(id)
            }
        }
    }

    /// 当面板属于此树时设置焦点。
    pub fn focus(&mut self, panel: PanelId) -> Result<(), LayoutError> {
        if self.leaf_for(panel).is_none() {
            return Err(LayoutError::UnknownPanel);
        }
        self.focused = panel;
        Ok(())
    }

    /// 拆分 `target` 并聚焦 `new_panel`。
    pub fn split(
        &mut self,
        target: PanelId,
        new_panel: PanelId,
        axis: SplitAxis,
    ) -> Result<(), LayoutError> {
        self.split_with_size(target, new_panel, axis, PanelSize::fill(1))
    }

    /// 以显式槽位约束拆分 `target` 并聚焦 `new_panel`。
    ///
    /// 新槽位插入在 `target` 之后；`target` 原有槽位保留自己的约束。
    ///
    /// # Errors
    /// 目标面板不存在或约束非法时返回错误，且不修改布局。
    pub fn split_with_size(
        &mut self,
        target: PanelId,
        new_panel: PanelId,
        axis: SplitAxis,
        size: PanelSize,
    ) -> Result<(), LayoutError> {
        if !size.is_valid() {
            return Err(LayoutError::InvalidSize);
        }
        let target_id = self.leaf_for(target).ok_or(LayoutError::UnknownPanel)?;
        let parent = self.nodes[target_id].parent();
        let same_axis_parent = parent.and_then(|parent_id| match &self.nodes[parent_id] {
            LayoutNode::Split {
                axis: parent_axis,
                children,
                ..
            } if *parent_axis == axis => Some((
                parent_id,
                children
                    .iter()
                    .position(|child| *child == target_id)
                    .expect("The parent node must contain child nodes"),
            )),
            _ => None,
        });
        if let Some((parent_id, position)) = same_axis_parent {
            let child = self.nodes.insert(LayoutNode::Leaf {
                panel: new_panel,
                parent: Some(parent_id),
            });
            if let LayoutNode::Split {
                children, sizes, ..
            } = &mut self.nodes[parent_id]
            {
                children.insert(position + 1, child);
                sizes.insert(position + 1, size);
            }
            self.focused = new_panel;
            return Ok(());
        }

        let old = self.nodes.insert(LayoutNode::Leaf {
            panel: target,
            parent: None,
        });
        let new = self.nodes.insert(LayoutNode::Leaf {
            panel: new_panel,
            parent: None,
        });
        let split = self.nodes.insert(LayoutNode::Split {
            axis,
            children: vec![old, new],
            sizes: vec![PanelSize::fill(1), size],
            parent,
        });
        self.nodes[old].set_parent(Some(split));
        self.nodes[new].set_parent(Some(split));
        if let Some(parent_id) = parent {
            if let LayoutNode::Split { children, .. } = &mut self.nodes[parent_id]
                && let Some(position) = children.iter().position(|child| *child == target_id)
            {
                children[position] = split;
            }
        } else {
            self.root = split;
        }
        self.nodes.remove(target_id);
        self.focused = new_panel;
        Ok(())
    }

    /// 关闭一个面板并折叠只有一个子节点的父节点。
    pub fn close(&mut self, panel: PanelId) -> Result<(), LayoutError> {
        let leaf = self.leaf_for(panel).ok_or(LayoutError::UnknownPanel)?;
        let Some(parent) = self.nodes[leaf].parent() else {
            return Err(LayoutError::LastPanel);
        };
        let grandparent = self.nodes[parent].parent();
        let remaining = if let LayoutNode::Split {
            children, sizes, ..
        } = &mut self.nodes[parent]
        {
            let position = children
                .iter()
                .position(|child| *child == leaf)
                .expect("The parent node must contain child nodes");
            children.remove(position);
            sizes.remove(position);
            children.clone()
        } else {
            unreachable!("The parent node of a leaf node is always a split container")
        };
        self.nodes.remove(leaf);
        let replacement = if remaining.len() == 1 {
            let replacement = remaining[0];
            self.nodes[replacement].set_parent(grandparent);
            if let Some(grandparent) = grandparent {
                if let LayoutNode::Split { children, .. } = &mut self.nodes[grandparent]
                    && let Some(position) = children.iter().position(|child| *child == parent)
                {
                    children[position] = replacement;
                }
            } else {
                self.root = replacement;
            }
            self.nodes.remove(parent);
            replacement
        } else {
            parent
        };
        self.focused = self
            .first_panel(replacement)
            .expect("Non-empty replacements must contain panels");
        Ok(())
    }

    /// 按树顺序计算叶节点矩形。
    #[must_use]
    pub fn rectangles(&self, area: Rect) -> Vec<(PanelId, Rect)> {
        let mut result = Vec::new();
        self.layout_node(self.root, area, &mut result);
        result
    }

    /// 查找绝对终端单元格处的拆分分隔条。分隔条两侧相邻的单元格都可命中，
    /// 使相邻面板共享的边框在终端中易于拖动。
    #[must_use]
    pub fn divider_at(&self, area: Rect, x: u16, y: u16) -> Option<SplitDivider> {
        self.divider_in_node(self.root, area, x, y)
    }

    /// 移动分隔条，同时保持权重为正且满足子树最小尺寸。`minimum_width` 和
    /// `minimum_height` 是每个叶节点以终端单元格计的尺寸；句柄失效或空间无法同时满足
    /// 相邻两棵子树时返回 `false`。
    ///
    /// 拖动只改写可伸缩的一侧：固定单元格或百分比的一侧保持约束不变，另一侧改写为
    /// `Fill` 权重；两侧都是 `Fill` 时同时改写权重。
    pub fn resize_divider(
        &mut self,
        divider: SplitDivider,
        area: Rect,
        x: u16,
        y: u16,
        minimum_width: u16,
        minimum_height: u16,
    ) -> bool {
        let Some(split_area) = self.node_area(self.root, area, divider.split) else {
            return false;
        };
        let Some(LayoutNode::Split {
            axis,
            children,
            sizes,
            ..
        }) = self.nodes.get(divider.split)
        else {
            return false;
        };
        if *axis != divider.axis
            || sizes.len() != children.len()
            || divider.before + 1 >= children.len()
        {
            return false;
        }
        let child_areas = split_rectangles(*axis, sizes, split_area);
        let before_area = child_areas[divider.before];
        let after_area = child_areas[divider.before + 1];
        let before_minimum = self
            .minimum_extent(
                children[divider.before],
                *axis,
                minimum_width,
                minimum_height,
            )
            .max(sizes[divider.before].min);
        let after_minimum = self
            .minimum_extent(
                children[divider.before + 1],
                *axis,
                minimum_width,
                minimum_height,
            )
            .max(sizes[divider.before + 1].min);
        let (before_start, combined, pointer) = match axis {
            SplitAxis::Rows => (
                before_area.y,
                before_area.height.saturating_add(after_area.height),
                y,
            ),
            SplitAxis::Columns => (
                before_area.x,
                before_area.width.saturating_add(after_area.width),
                x,
            ),
        };
        if combined < before_minimum.saturating_add(after_minimum) {
            return false;
        }
        let desired = pointer
            .saturating_sub(before_start)
            .clamp(before_minimum, combined.saturating_sub(after_minimum));
        let remaining = combined.saturating_sub(desired).max(1);
        let mut new_sizes = sizes.clone();
        let before_fixed = new_sizes[divider.before].weight() == 0;
        let after_fixed = new_sizes[divider.before + 1].weight() == 0;
        match (before_fixed, after_fixed) {
            // 固定的一侧保持自己的约束，可伸缩的一侧吸收拖动结果。
            (true, _) => new_sizes[divider.before + 1].length = Length::Fill(remaining),
            (false, true) => new_sizes[divider.before].length = Length::Fill(desired.max(1)),
            // 两侧都可伸缩时同时改写权重，保持拖动后的实际比例。
            (false, false) => {
                new_sizes[divider.before].length = Length::Fill(desired.max(1));
                new_sizes[divider.before + 1].length = Length::Fill(remaining);
            }
        }
        if !new_sizes.iter().all(|size| size.is_valid()) {
            return false;
        }
        let Some(LayoutNode::Split { sizes, .. }) = self.nodes.get_mut(divider.split) else {
            return false;
        };
        *sizes = new_sizes;
        true
    }

    /// 返回包含此面板的槽位约束；根叶节点没有约束。
    #[must_use]
    pub fn panel_size(&self, panel: PanelId) -> Option<PanelSize> {
        let leaf = self.leaf_for(panel)?;
        let parent = self.nodes[leaf].parent()?;
        let LayoutNode::Split {
            children, sizes, ..
        } = &self.nodes[parent]
        else {
            return None;
        };
        let position = children.iter().position(|child| *child == leaf)?;
        sizes.get(position).copied()
    }

    /// 改写包含此面板的槽位约束。
    ///
    /// # Errors
    /// 面板为根叶节点、面板不存在或约束非法时返回错误，且不修改布局。
    pub fn set_panel_size(&mut self, panel: PanelId, size: PanelSize) -> Result<(), LayoutError> {
        if !size.is_valid() {
            return Err(LayoutError::InvalidSize);
        }
        let leaf = self.leaf_for(panel).ok_or(LayoutError::UnknownPanel)?;
        let parent = self.nodes[leaf].parent().ok_or(LayoutError::InvalidSize)?;
        let LayoutNode::Split {
            children, sizes, ..
        } = &mut self.nodes[parent]
        else {
            return Err(LayoutError::InvalidSize);
        };
        let position = children
            .iter()
            .position(|child| *child == leaf)
            .ok_or(LayoutError::InvalidStructure)?;
        let slot = sizes
            .get_mut(position)
            .ok_or(LayoutError::InvalidStructure)?;
        *slot = size;
        Ok(())
    }

    /// 查找并聚焦指定几何方向上最近的面板。
    pub fn focus_direction(&mut self, direction: Direction, area: Rect) -> Option<PanelId> {
        let rectangles = self.rectangles(area);
        let (_, current) = rectangles
            .iter()
            .find(|(panel, _)| *panel == self.focused)?;
        let current_center = center(*current);
        let candidate = rectangles
            .iter()
            .filter(|(panel, _)| *panel != self.focused)
            .filter_map(|(panel, rect)| {
                let other = center(*rect);
                let in_direction = match direction {
                    Direction::Up => other.1 < current_center.1,
                    Direction::Down => other.1 > current_center.1,
                    Direction::Left => other.0 < current_center.0,
                    Direction::Right => other.0 > current_center.0,
                };
                in_direction.then(|| {
                    let primary = match direction {
                        Direction::Up | Direction::Down => other.1.abs_diff(current_center.1),
                        Direction::Left | Direction::Right => other.0.abs_diff(current_center.0),
                    };
                    let secondary = match direction {
                        Direction::Up | Direction::Down => other.0.abs_diff(current_center.0),
                        Direction::Left | Direction::Right => other.1.abs_diff(current_center.1),
                    };
                    (*panel, (primary, secondary))
                })
            })
            .min_by_key(|(_, distance)| *distance)
            .map(|(panel, _)| panel)?;
        self.focused = candidate;
        Some(candidate)
    }

    /// 运行时重建面板后重写序列化面板标识。
    pub fn remap_panels(
        &mut self,
        mapping: &std::collections::HashMap<PanelId, PanelId>,
    ) -> Result<(), LayoutError> {
        for node in self.nodes.values_mut() {
            if let LayoutNode::Leaf { panel, .. } = node {
                *panel = *mapping.get(panel).ok_or(LayoutError::UnknownPanel)?;
            }
        }
        self.focused = *mapping
            .get(&self.focused)
            .ok_or(LayoutError::UnknownPanel)?;
        Ok(())
    }

    fn leaf_for(&self, panel: PanelId) -> Option<LayoutId> {
        self.nodes.iter().find_map(|(id, node)| match node {
            LayoutNode::Leaf { panel: found, .. } if *found == panel => Some(id),
            _ => None,
        })
    }

    fn first_panel(&self, node: LayoutId) -> Option<PanelId> {
        match &self.nodes[node] {
            LayoutNode::Leaf { panel, .. } => Some(*panel),
            LayoutNode::Split { children, .. } => {
                children.first().and_then(|child| self.first_panel(*child))
            }
        }
    }

    fn layout_node(&self, node: LayoutId, area: Rect, output: &mut Vec<(PanelId, Rect)>) {
        match &self.nodes[node] {
            LayoutNode::Leaf { panel, .. } => output.push((*panel, area)),
            LayoutNode::Split {
                axis,
                children,
                sizes,
                ..
            } => {
                for (child, child_area) in children.iter().zip(split_rectangles(*axis, sizes, area))
                {
                    self.layout_node(*child, child_area, output);
                }
            }
        }
    }

    fn divider_in_node(&self, node: LayoutId, area: Rect, x: u16, y: u16) -> Option<SplitDivider> {
        let LayoutNode::Split {
            axis,
            children,
            sizes,
            ..
        } = &self.nodes[node]
        else {
            return None;
        };
        let child_areas = split_rectangles(*axis, sizes, area);
        for (index, child_area) in child_areas
            .iter()
            .enumerate()
            .take(children.len().saturating_sub(1))
        {
            let boundary = match axis {
                SplitAxis::Rows => child_area.y.saturating_add(child_area.height),
                SplitAxis::Columns => child_area.x.saturating_add(child_area.width),
            };
            let on_boundary = match axis {
                SplitAxis::Rows => {
                    (y == boundary || y.saturating_add(1) == boundary)
                        && x >= area.x
                        && x < area.x.saturating_add(area.width)
                }
                SplitAxis::Columns => {
                    (x == boundary || x.saturating_add(1) == boundary)
                        && y >= area.y
                        && y < area.y.saturating_add(area.height)
                }
            };
            if on_boundary {
                return Some(SplitDivider {
                    split: node,
                    before: index,
                    axis: *axis,
                });
            }
        }
        children
            .iter()
            .zip(child_areas)
            .find_map(|(child, child_area)| {
                child_area
                    .contains(x, y)
                    .then(|| self.divider_in_node(*child, child_area, x, y))
                    .flatten()
            })
    }

    fn node_area(&self, node: LayoutId, area: Rect, target: LayoutId) -> Option<Rect> {
        if node == target {
            return Some(area);
        }
        let LayoutNode::Split {
            axis,
            children,
            sizes,
            ..
        } = &self.nodes[node]
        else {
            return None;
        };
        children
            .iter()
            .zip(split_rectangles(*axis, sizes, area))
            .find_map(|(child, child_area)| self.node_area(*child, child_area, target))
    }

    fn minimum_extent(
        &self,
        node: LayoutId,
        axis: SplitAxis,
        minimum_width: u16,
        minimum_height: u16,
    ) -> u16 {
        match &self.nodes[node] {
            LayoutNode::Leaf { .. } => match axis {
                SplitAxis::Rows => minimum_height,
                SplitAxis::Columns => minimum_width,
            },
            LayoutNode::Split {
                axis: child_axis,
                children,
                ..
            } => {
                let extents = children
                    .iter()
                    .map(|child| self.minimum_extent(*child, axis, minimum_width, minimum_height));
                if *child_axis == axis {
                    extents.fold(0, u16::saturating_add)
                } else {
                    extents.max().unwrap_or(0)
                }
            }
        }
    }
}

/// 按槽位约束把主轴长度分配给所有子节点。
///
/// 先满足 `Cells` 与 `Percent`，再按 `Fill` 权重分配剩余空间，然后应用上下限。
/// 约束无法同时满足时以铺满父容器为准，多余或不足的部分由最后一个子节点吸收，
/// 因此返回值之和始终等于父容器主轴长度。
fn split_extents(axis: SplitAxis, sizes: &[PanelSize], area: Rect) -> Vec<u16> {
    let available = match axis {
        SplitAxis::Rows => area.height,
        SplitAxis::Columns => area.width,
    };
    let mut extents = vec![0_u16; sizes.len()];
    let mut fixed = 0_u32;
    let mut fill_weight = 0_u32;
    for (index, size) in sizes.iter().enumerate() {
        match size.length {
            Length::Fill(weight) => fill_weight = fill_weight.saturating_add(u32::from(weight)),
            Length::Cells(_) | Length::Percent(_) => {
                let extent = size.clamp(size.preferred(available));
                extents[index] = extent;
                fixed = fixed.saturating_add(u32::from(extent));
            }
        }
    }
    let remaining = u32::from(available).saturating_sub(fixed);
    let mut last_fill = None;
    for (index, size) in sizes.iter().enumerate() {
        let Length::Fill(weight) = size.length else {
            continue;
        };
        last_fill = Some(index);
        let share = (remaining * u32::from(weight)) / fill_weight.max(1);
        let extent = size.clamp(u16::try_from(share).unwrap_or(u16::MAX));
        extents[index] = extent;
    }
    // 舍入与上下限都会留下差值；拖动语义要求布局始终铺满，因此由最后一个
    // 可伸缩槽位（没有可伸缩槽位时由最后一个槽位）吸收。
    let total = extents.iter().copied().map(u32::from).sum::<u32>();
    let target = last_fill
        .or_else(|| (!sizes.is_empty()).then_some(sizes.len() - 1))
        .expect("Split nodes always have children");
    let adjusted = u32::from(extents[target])
        .saturating_add(u32::from(available))
        .saturating_sub(total);
    extents[target] = u16::try_from(adjusted).unwrap_or(u16::MAX);
    extents
}

/// 按槽位约束把区域切分给子节点。
///
/// 与布局树使用同一套长度语义，供组合 Widget 在不引入新面板类别的条件下切分自己的区域。
#[must_use]
pub fn split_rectangles(axis: SplitAxis, sizes: &[PanelSize], area: Rect) -> Vec<Rect> {
    let extents = split_extents(axis, sizes, area);
    let mut offset = 0_u16;
    extents
        .into_iter()
        .map(|size| {
            let rect = match axis {
                SplitAxis::Rows => Rect {
                    x: area.x,
                    y: area.y.saturating_add(offset),
                    width: area.width,
                    height: size,
                },
                SplitAxis::Columns => Rect {
                    x: area.x.saturating_add(offset),
                    y: area.y,
                    width: size,
                    height: area.height,
                },
            };
            offset = offset.saturating_add(size);
            rect
        })
        .collect()
}

fn center(rect: Rect) -> (u16, u16) {
    (
        rect.x.saturating_add(rect.width / 2),
        rect.y.saturating_add(rect.height / 2),
    )
}

#[cfg(test)]
mod tests {
    use slotmap::KeyData;

    use super::{
        Direction, LayoutSpec, LayoutTree, Length, NodeSpec, PanelId, PanelSize, Rect, SplitAxis,
    };

    fn panel(id: u64) -> PanelId {
        PanelId::from(KeyData::from_ffi(id))
    }

    #[test]
    fn splits_collapses_and_navigates_rectangles() {
        let mut tree = LayoutTree::new(panel(1));
        tree.split(panel(1), panel(2), SplitAxis::Columns)
            .expect("split vertically");
        tree.focus(panel(1)).expect("Already have a panel");
        tree.split(panel(1), panel(3), SplitAxis::Rows)
            .expect("split horizontally");
        let rects = tree.rectangles(Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 40,
        });
        assert_eq!(rects.len(), 3);
        tree.focus(panel(3)).expect("Already have a panel");
        assert_eq!(
            tree.focus_direction(
                Direction::Right,
                Rect {
                    x: 0,
                    y: 0,
                    width: 120,
                    height: 40
                }
            ),
            Some(panel(2))
        );
        tree.close(panel(1)).expect("Close a child node");
        assert_eq!(
            tree.rectangles(Rect {
                x: 0,
                y: 0,
                width: 120,
                height: 40
            })
            .len(),
            2
        );
    }

    #[test]
    fn hit_tests_and_resizes_nested_dividers_with_minimums() {
        let mut tree = LayoutTree::new(panel(1));
        tree.split(panel(1), panel(2), SplitAxis::Columns)
            .expect("split vertically");
        tree.focus(panel(1)).expect("Already have a panel");
        tree.split(panel(1), panel(3), SplitAxis::Rows)
            .expect("Nested horizontal split");
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        };
        let divider = tree
            .divider_at(area, 49, 20)
            .expect("Should be able to hit the root divider");
        assert_eq!(divider.axis(), SplitAxis::Columns);
        assert!(tree.resize_divider(divider, area, 70, 20, 12, 4));
        let rectangles = tree.rectangles(area);
        let right = rectangles
            .iter()
            .find(|(id, _)| *id == panel(2))
            .map(|(_, rect)| *rect)
            .expect("right panel");
        assert_eq!(right.x, 70);
        assert!(tree.resize_divider(divider, area, 99, 20, 12, 4));
        let right = tree
            .rectangles(area)
            .into_iter()
            .find(|(id, _)| *id == panel(2))
            .map(|(_, rect)| rect)
            .expect("right panel");
        assert!(right.width >= 12);
        assert!(area.contains(99, 39));
        assert!(!area.contains(100, 39));
    }

    #[test]
    fn honours_fixed_percentage_and_fill_slots() {
        let tree = LayoutTree::from_spec(LayoutSpec {
            focused: panel(1),
            root: NodeSpec::Split {
                axis: SplitAxis::Rows,
                children: vec![
                    (PanelSize::fill(1).with_min(4), NodeSpec::Leaf(panel(1))),
                    (PanelSize::cells(4), NodeSpec::Leaf(panel(2))),
                ],
            },
        })
        .expect("template must be valid");
        let rows = tree.rectangles(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 30,
        });
        assert_eq!(rows[0].1.height, 26);
        assert_eq!(rows[1].1.height, 4);
        assert_eq!(rows[1].1.y, 26);

        let tree = LayoutTree::from_spec(LayoutSpec {
            focused: panel(1),
            root: NodeSpec::Split {
                axis: SplitAxis::Columns,
                children: vec![
                    (PanelSize::percent(25), NodeSpec::Leaf(panel(1))),
                    (PanelSize::fill(3), NodeSpec::Leaf(panel(2))),
                ],
            },
        })
        .expect("template must be valid");
        let columns = tree.rectangles(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 10,
        });
        assert_eq!(columns[0].1.width, 20);
        assert_eq!(columns[1].1.width, 60);
    }

    #[test]
    fn keeps_tiling_when_constraints_cannot_be_satisfied() {
        let tree = LayoutTree::from_spec(LayoutSpec {
            focused: panel(1),
            root: NodeSpec::Split {
                axis: SplitAxis::Rows,
                children: vec![
                    (PanelSize::cells(10), NodeSpec::Leaf(panel(1))),
                    (PanelSize::fill(1).with_min(8), NodeSpec::Leaf(panel(2))),
                ],
            },
        })
        .expect("template must be valid");
        let rows = tree.rectangles(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 12,
        });
        let total = rows.iter().map(|(_, rect)| rect.height).sum::<u16>();
        assert_eq!(total, 12);
        assert_eq!(rows[1].1.y, rows[0].1.height);
    }

    #[test]
    fn resize_keeps_fixed_slots_and_rewrites_the_other_side() {
        let mut tree = LayoutTree::from_spec(LayoutSpec {
            focused: panel(1),
            root: NodeSpec::Split {
                axis: SplitAxis::Rows,
                children: vec![
                    (PanelSize::fill(1), NodeSpec::Leaf(panel(1))),
                    (PanelSize::cells(4), NodeSpec::Leaf(panel(2))),
                ],
            },
        })
        .expect("template must be valid");
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 30,
        };
        let divider = tree
            .divider_at(area, 10, 25)
            .expect("the divider above the fixed slot must be reachable");
        assert!(tree.resize_divider(divider, area, 10, 20, 1, 1));
        let rows = tree.rectangles(area);
        assert_eq!(rows[1].1.height, 4, "the fixed slot must not move");
        assert_eq!(rows[0].1.height, 26);
    }

    #[test]
    fn rejects_invalid_slots_and_duplicate_panels() {
        let invalid = LayoutTree::from_spec(LayoutSpec {
            focused: panel(1),
            root: NodeSpec::Split {
                axis: SplitAxis::Rows,
                children: vec![
                    (PanelSize::fill(0), NodeSpec::Leaf(panel(1))),
                    (PanelSize::fill(1), NodeSpec::Leaf(panel(2))),
                ],
            },
        });
        assert!(invalid.is_err());

        let duplicate = LayoutTree::from_spec(LayoutSpec {
            focused: panel(1),
            root: NodeSpec::Split {
                axis: SplitAxis::Rows,
                children: vec![
                    (PanelSize::fill(1), NodeSpec::Leaf(panel(1))),
                    (PanelSize::fill(1), NodeSpec::Leaf(panel(1))),
                ],
            },
        });
        assert!(duplicate.is_err());

        let mut tree = LayoutTree::new(panel(1));
        assert!(
            tree.set_panel_size(panel(1), PanelSize::fill(1)).is_err(),
            "the root leaf has no slot constraint"
        );
    }

    #[test]
    fn reads_and_writes_slot_constraints() {
        let mut tree = LayoutTree::new(panel(1));
        tree.split_with_size(panel(1), panel(2), SplitAxis::Rows, PanelSize::cells(3))
            .expect("split with an explicit size");
        assert_eq!(tree.panel_size(panel(2)), Some(PanelSize::cells(3)));
        tree.set_panel_size(panel(2), PanelSize::cells(6).with_max(8))
            .expect("rewrite the slot constraint");
        assert_eq!(
            tree.panel_size(panel(2)).map(|size| size.length),
            Some(Length::Cells(6))
        );
        assert_eq!(tree.validate().map(|panels| panels.len()), Ok(2));
    }
}
