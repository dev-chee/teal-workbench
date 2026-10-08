//! 类型化 JSON 组件树、局部控件状态与输入事件。
//!
//! 替换树前先检查版本、字段和资源界限；失败时保留现有规格与状态。
//! 稳定控件 ID 关联输入值、焦点和选择，最近绘制的命中表关联鼠标位置与控件。

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap},
};
use teal_workbench_core::{InputEvent, Key, MouseButton, MouseEvent, MouseEventKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u16,
    root: Node,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Node {
    id: String,
    width: Length,
    height: Length,
    content: Component,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum Length {
    Auto(String),
    Cells { cells: u16 },
    Percent { percent: u16 },
    Fill { fill: u16 },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Component {
    Row {
        children: Vec<Node>,
        gap: u16,
    },
    Column {
        children: Vec<Node>,
        gap: u16,
    },
    Grid {
        children: Vec<Node>,
        columns: u16,
        gap: u16,
    },
    Text {
        text: String,
        style: u8,
        wrap: bool,
    },
    Button {
        label: String,
        enabled: bool,
    },
    Input {
        value: String,
        placeholder: String,
    },
    List {
        items: Vec<String>,
        selected: i32,
    },
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        selected: i32,
    },
    Tree {
        items: Vec<TreeItem>,
        selected: String,
    },
    Progress {
        value: u32,
        maximum: u32,
        label: String,
    },
    Spacer {
        cells: u16,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TreeItem {
    id: String,
    label: String,
    expanded: bool,
    children: Vec<TreeItem>,
}

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    #[serde(default)]
    command_mode: bool,
    focus: Option<String>,
    values: BTreeMap<String, String>,
    selected: BTreeMap<String, usize>,
    expanded: BTreeMap<String, bool>,
}

#[derive(Clone)]
struct Hit {
    id: String,
    area: Rect,
    content: Component,
}

/// 经过校验的宿主无关终端组件树，使用稳定的局部控件状态。
pub struct DeclarativePanel {
    spec: Envelope,
    state: State,
    hits: Vec<Hit>,
}

impl DeclarativePanel {
    /// 渲染前解析 UI 数据并校验深度、节点数量和大小限制。
    pub fn from_json(json: &str) -> Result<Self, String> {
        let spec = parse(json)?;
        Ok(Self {
            spec,
            state: State::default(),
            hits: Vec::new(),
        })
    }

    /// 验证完整新规格后替换组件树，并保留仍存在 ID 的状态。
    ///
    /// 解析或资源界限检查失败时保持原树；成功后删除已消失控件的状态并清空旧命中区域。
    /// 新鼠标几何在下一次渲染时建立，不能继续使用旧树的坐标响应输入。
    pub fn replace(&mut self, json: &str) -> Result<(), String> {
        // 先解析并验证，确保拒绝更新时不影响当前树与状态。
        let spec = parse(json)?;
        let ids = ids(&spec.root);

        self.state.values.retain(|id, _| ids.contains(id));
        self.state.selected.retain(|id, _| ids.contains(id));
        self.state.expanded.retain(|id, _| ids.contains(id));
        self.hits.clear();
        if self
            .state
            .focus
            .as_ref()
            .is_some_and(|id| !ids.contains(id))
        {
            self.state.focus = None;
        }

        self.spec = spec;
        Ok(())
    }

    /// 返回用于会话保存的局部控件状态。
    ///
    /// 序列化失败或结果超过 256 KiB 时返回 JSON `null`，不修改当前控件状态。
    /// 规格与状态分开保存，恢复时仍须用有效规格筛除不存在的控件 ID。
    pub fn state(&self) -> serde_json::Value {
        let value = serde_json::to_value(&self.state).unwrap_or_default();
        if value.to_string().len() > 256 * 1024 {
            serde_json::Value::Null
        } else {
            value
        }
    }

    /// 恢复有界控件状态，拒绝畸形数据。
    pub fn restore_state(&mut self, value: serde_json::Value) -> Result<(), String> {
        if value.is_null() {
            return Ok(());
        }
        if value.to_string().len() > 256 * 1024 {
            return Err("custom state exceeds 256 KiB".into());
        }
        self.state = serde_json::from_value(value).map_err(|e| e.to_string())?;
        self.replace(&self.specification())
    }

    /// 返回最近接受的组件规格供离线恢复。
    pub fn specification(&self) -> String {
        serde_json::to_string(&self.spec).unwrap_or_default()
    }

    /// 将键盘输入路由到聚焦控件，并发出类型化字符串事件信封。
    pub fn input(&mut self, event: InputEvent) -> Option<BTreeMap<String, String>> {
        if event.key == Key::Escape {
            self.state.command_mode = true;
            return Some(key_event("", event));
        }
        if event.key == Key::Tab || event.key == Key::BackTab {
            self.state.command_mode = false;
            if self.hits.is_empty() {
                return None;
            }
            let index = self
                .hits
                .iter()
                .position(|h| Some(&h.id) == self.state.focus.as_ref())
                .unwrap_or(0);
            let next = if event.key == Key::BackTab {
                (index + self.hits.len() - 1) % self.hits.len()
            } else {
                (index + 1) % self.hits.len()
            };
            let id = self.hits[next].id.clone();
            self.state.focus = Some(id.clone());
            return Some(envelope("focus", &id));
        }
        let hit = self
            .hits
            .iter()
            .find(|h| Some(&h.id) == self.state.focus.as_ref())
            .or_else(|| self.hits.first())?
            .clone();
        self.state.focus = Some(hit.id.clone());
        if self.state.command_mode {
            if event.key == Key::Enter {
                self.state.command_mode = false;
                return Some(envelope("focus", &hit.id));
            }
            return Some(key_event(&hit.id, event));
        }
        match hit.content {
            Component::Button { enabled: true, .. }
                if matches!(event.key, Key::Enter | Key::Char(' ')) =>
            {
                Some(envelope("activate", &hit.id))
            }
            Component::Input { value, .. } => {
                let text = self.state.values.entry(hit.id.clone()).or_insert(value);
                match event.key {
                    Key::Backspace => {
                        text.pop();
                    }
                    Key::Char(c) if !event.modifiers.control && !event.modifiers.alt => {
                        text.push(c);
                    }
                    Key::Enter => return Some(envelope("activate", &hit.id)),
                    _ => return Some(key_event(&hit.id, event)),
                }
                let mut e = envelope("change", &hit.id);
                e.insert("value".into(), text.clone());
                Some(e)
            }
            Component::List { items, selected } => {
                self.select_key(&hit.id, items.len(), selected, event, None)
            }
            Component::Table { rows, selected, .. } => {
                self.select_key(&hit.id, rows.len(), selected, event, None)
            }
            Component::Tree { items, selected } => {
                let rows = tree_rows(&items, &self.state.expanded);
                let initial = rows
                    .iter()
                    .position(|(_, id, _)| id == &selected)
                    .unwrap_or(0);
                let cursor = *self.state.selected.entry(hit.id.clone()).or_insert(initial);
                if let Some((_, id, _)) = rows.get(cursor)
                    && matches!(event.key, Key::Left | Key::Right)
                {
                    self.state
                        .expanded
                        .insert(id.clone(), event.key == Key::Right);
                    let mut e = envelope("selection", &hit.id);
                    e.insert("index".into(), cursor.to_string());
                    e.insert("key".into(), id.clone());
                    return Some(e);
                }
                self.select_key(
                    &hit.id,
                    rows.len(),
                    i32::try_from(initial).unwrap_or(i32::MAX),
                    event,
                    Some(rows.iter().map(|(_, id, _)| id.clone()).collect()),
                )
            }
            _ => Some(key_event(&hit.id, event)),
        }
    }

    fn select_key(
        &mut self,
        id: &str,
        len: usize,
        initial: i32,
        event: InputEvent,
        keys: Option<Vec<String>>,
    ) -> Option<BTreeMap<String, String>> {
        if len == 0 {
            return None;
        }
        let selected = self
            .state
            .selected
            .entry(id.to_owned())
            .or_insert(usize::try_from(initial).unwrap_or(0));
        if event.key == Key::Enter {
            return Some(envelope("activate", id));
        }
        match event.key {
            Key::Up => *selected = selected.saturating_sub(1),
            Key::Down => *selected = selected.saturating_add(1).min(len.saturating_sub(1)),
            _ => return Some(key_event(id, event)),
        }
        let mut e = envelope("selection", id);
        e.insert("index".into(), selected.to_string());
        e.insert(
            "key".into(),
            keys.and_then(|keys| keys.get(*selected).cloned())
                .unwrap_or_default(),
        );
        Some(e)
    }

    /// 使用上次渲染产生的几何信息路由鼠标按下事件。
    pub fn mouse(&mut self, event: MouseEvent) -> Option<BTreeMap<String, String>> {
        self.state.command_mode = false;
        if !matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            return None;
        }
        let hit = self
            .hits
            .iter()
            .find(|h| {
                event.column >= h.area.x
                    && event.column < h.area.right()
                    && event.row >= h.area.y
                    && event.row < h.area.bottom()
            })?
            .clone();
        self.state.focus = Some(hit.id.clone());
        let mut row = usize::from(event.row.saturating_sub(hit.area.y));
        let selected = self.state.selected.get(&hit.id).copied().unwrap_or(0);
        let height = usize::from(hit.area.height)
            .saturating_sub(usize::from(matches!(hit.content, Component::Table { .. })));
        row += selected.saturating_sub(height.saturating_sub(1));
        match &hit.content {
            Component::Button { enabled: true, .. } => Some(envelope("activate", &hit.id)),
            Component::List { .. } | Component::Tree { .. } => {
                let (len, key) = match &hit.content {
                    Component::List { items, .. } => (items.len(), String::new()),
                    Component::Tree { items, .. } => {
                        let rows = tree_rows(items, &self.state.expanded);
                        (
                            rows.len(),
                            rows.get(row)
                                .map(|(_, id, _)| id.clone())
                                .unwrap_or_default(),
                        )
                    }
                    _ => (0, String::new()),
                };
                if row >= len {
                    return None;
                }
                self.state.selected.insert(hit.id.clone(), row);
                let mut e = envelope("selection", &hit.id);
                e.insert("index".into(), row.to_string());
                e.insert("key".into(), key);
                Some(e)
            }
            Component::Table { rows, .. } => {
                if event.row == hit.area.y {
                    return Some(envelope("focus", &hit.id));
                }
                let row = row.saturating_sub(1);
                if row >= rows.len() {
                    return None;
                }
                self.state.selected.insert(hit.id.clone(), row);
                let mut e = envelope("selection", &hit.id);
                e.insert("index".into(), row.to_string());
                e.insert("key".into(), String::new());
                Some(e)
            }
            _ => Some(envelope("focus", &hit.id)),
        }
    }

    /// 渲染经过校验的组件，不执行文件、网络或脚本操作。
    pub fn render(&mut self, frame: &mut Frame<'_>, area: Rect) {
        self.hits.clear();
        let root = self.spec.root.clone();
        self.render_node(frame, &root, area);
        if self.state.focus.is_none() {
            self.state.focus = self.hits.first().map(|h| h.id.clone());
        }
    }

    /// 返回聚焦输入框的终端单元格光标坐标，并限制到已渲染矩形内。
    pub fn cursor(&self) -> Option<(u16, u16)> {
        if self.state.command_mode {
            return None;
        }
        let hit = self.hits.iter().find(|hit| {
            Some(&hit.id) == self.state.focus.as_ref()
                && matches!(hit.content, Component::Input { .. })
        })?;
        let value = self.state.values.get(&hit.id).map_or("", String::as_str);
        let width = unicode_width::UnicodeWidthStr::width(value);
        Some((
            hit.area.x
                + u16::try_from(width)
                    .unwrap_or(u16::MAX)
                    .min(hit.area.width.saturating_sub(1)),
            hit.area.y,
        ))
    }

    /// 聚焦输入框是否拥有可打印按键；Escape 返回宿主命令模式。
    pub fn text_input_active(&self) -> bool {
        !self.state.command_mode
            && self.hits.iter().any(|hit| {
                Some(&hit.id) == self.state.focus.as_ref()
                    && matches!(hit.content, Component::Input { .. })
            })
    }

    fn render_node(&mut self, frame: &mut Frame<'_>, node: &Node, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let focused = self.state.focus.as_ref() == Some(&node.id);
        let style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };
        match &node.content {
            Component::Row { children, gap } | Component::Column { children, gap } => {
                let horizontal = matches!(node.content, Component::Row { .. });
                let constraints = children
                    .iter()
                    .map(|child| {
                        constraint(
                            if horizontal {
                                &child.width
                            } else {
                                &child.height
                            },
                            child,
                            horizontal,
                        )
                    })
                    .collect::<Vec<_>>();
                let areas = Layout::default()
                    .direction(if horizontal {
                        Direction::Horizontal
                    } else {
                        Direction::Vertical
                    })
                    .constraints(constraints)
                    .spacing(*gap)
                    .split(area);
                for (child, area) in children.iter().zip(areas.iter()) {
                    self.render_node(frame, child, *area);
                }
            }
            Component::Grid {
                children,
                columns,
                gap,
            } => {
                let rows = children.len().div_ceil(usize::from(*columns)).max(1);
                let row_areas = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(vec![Constraint::Fill(1); rows])
                    .spacing(*gap)
                    .split(area);
                for (row, nodes) in children.chunks(usize::from(*columns)).enumerate() {
                    let areas = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints(vec![Constraint::Fill(1); nodes.len()])
                        .spacing(*gap)
                        .split(row_areas[row]);
                    for (node, area) in nodes.iter().zip(areas.iter()) {
                        self.render_node(frame, node, *area);
                    }
                }
            }
            Component::Text { text, style, wrap } => {
                let p = Paragraph::new(text.clone()).style(role(*style));
                if *wrap {
                    frame.render_widget(p.wrap(Wrap { trim: false }), area);
                } else {
                    frame.render_widget(p, area);
                }
            }
            Component::Button { label, enabled } => {
                if *enabled {
                    self.hits.push(Hit {
                        id: node.id.clone(),
                        area,
                        content: node.content.clone(),
                    });
                }
                frame.render_widget(
                    Paragraph::new(format!("[ {label} ]")).style(if *enabled {
                        style
                    } else {
                        Style::default().fg(Color::DarkGray)
                    }),
                    area,
                );
            }
            Component::Input { value, placeholder } => {
                self.hits.push(Hit {
                    id: node.id.clone(),
                    area,
                    content: node.content.clone(),
                });
                let value = self
                    .state
                    .values
                    .entry(node.id.clone())
                    .or_insert(value.clone());
                frame.render_widget(
                    Paragraph::new(if value.is_empty() {
                        placeholder.clone()
                    } else {
                        value.clone()
                    })
                    .style(style)
                    .block(Block::default().borders(Borders::BOTTOM)),
                    area,
                );
            }
            Component::List { items, selected } => {
                self.hits.push(Hit {
                    id: node.id.clone(),
                    area,
                    content: node.content.clone(),
                });
                let selected = *self
                    .state
                    .selected
                    .entry(node.id.clone())
                    .or_insert(usize::try_from(*selected).unwrap_or(0));
                let selected = selected.min(items.len().saturating_sub(1));
                self.state.selected.insert(node.id.clone(), selected);
                render_rows(frame, area, items, selected, style);
            }
            Component::Table {
                headers,
                rows,
                selected,
            } => {
                self.hits.push(Hit {
                    id: node.id.clone(),
                    area,
                    content: node.content.clone(),
                });
                let selected = *self
                    .state
                    .selected
                    .entry(node.id.clone())
                    .or_insert(usize::try_from(*selected).unwrap_or(0));
                let selected = selected.min(rows.len().saturating_sub(1));
                self.state.selected.insert(node.id.clone(), selected);
                frame.render_widget(
                    Paragraph::new(headers.join(" | ")).style(style),
                    Rect { height: 1, ..area },
                );
                let body = Rect {
                    y: area.y.saturating_add(1),
                    height: area.height.saturating_sub(1),
                    ..area
                };
                let lines = rows.iter().map(|row| row.join(" | ")).collect::<Vec<_>>();
                render_rows(frame, body, &lines, selected, style);
            }
            Component::Tree { items, selected } => {
                self.hits.push(Hit {
                    id: node.id.clone(),
                    area,
                    content: node.content.clone(),
                });
                let rows = tree_rows(items, &self.state.expanded);
                let initial = rows
                    .iter()
                    .position(|(_, id, _)| id == selected)
                    .unwrap_or(0);
                let selected = *self
                    .state
                    .selected
                    .entry(node.id.clone())
                    .or_insert(initial);
                let selected = selected.min(rows.len().saturating_sub(1));
                self.state.selected.insert(node.id.clone(), selected);
                render_rows(
                    frame,
                    area,
                    &rows
                        .iter()
                        .map(|(depth, _, label)| format!("{}{label}", "  ".repeat(*depth)))
                        .collect::<Vec<_>>(),
                    selected,
                    style,
                );
            }
            Component::Progress {
                value,
                maximum,
                label,
            } => frame.render_widget(
                Gauge::default()
                    .ratio(f64::from(*value) / f64::from(*maximum))
                    .label(label.clone()),
                area,
            ),
            Component::Spacer { .. } => {}
        }
    }
}

fn parse(json: &str) -> Result<Envelope, String> {
    if json.len() > 2 * 1024 * 1024 {
        return Err("UI payload exceeds 2 MiB".into());
    }
    let spec: Envelope = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if spec.version != 1 {
        return Err("unsupported UI version".into());
    }
    let mut ids = BTreeSet::new();
    let mut count = 0;
    let mut stack = vec![(&spec.root, 0)];
    while let Some((node, depth)) = stack.pop() {
        count += 1;
        if depth > 32 || count > 2048 {
            return Err("UI depth or node count exceeds the limit".into());
        }
        if node.id.is_empty() || !ids.insert(node.id.clone()) {
            return Err("UI ID is empty or duplicated".into());
        }
        for length in [&node.width, &node.height] {
            match length {
                Length::Auto(s) if s != "auto" => return Err("unknown UI length type".into()),
                Length::Percent { percent } if *percent > 100 => {
                    return Err("percentage exceeds 100".into());
                }
                Length::Fill { fill } if *fill == 0 => {
                    return Err("fill weight must be positive".into());
                }
                _ => {}
            }
        }
        match &node.content {
            Component::Row { children, .. }
            | Component::Column { children, .. }
            | Component::Grid { children, .. } => {
                if matches!(node.content, Component::Grid { columns: 0, .. }) {
                    return Err("grid must define columns".into());
                }
                stack.extend(children.iter().map(|n| (n, depth + 1)));
            }
            Component::Text { style, .. } if *style > 7 => {
                return Err("unknown semantic style".into());
            }
            Component::Progress { value, maximum, .. } if *maximum == 0 || value > maximum => {
                return Err("progress range is invalid".into());
            }
            Component::Table { headers, rows, .. }
                if rows.iter().any(|r| r.len() != headers.len()) =>
            {
                return Err("table row width differs from its header".into());
            }
            Component::Tree { items, .. } => {
                let mut nodes = items.iter().map(|n| (n, depth + 1)).collect::<Vec<_>>();
                while let Some((n, d)) = nodes.pop() {
                    count += 1;
                    if d > 32 || count > 2048 || n.id.is_empty() || !ids.insert(n.id.clone()) {
                        return Err("tree entry ID or boundary is invalid".into());
                    }
                    nodes.extend(n.children.iter().map(|c| (c, d + 1)));
                }
            }
            _ => {}
        }
    }
    Ok(spec)
}

fn ids(root: &Node) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        ids.insert(n.id.clone());
        match &n.content {
            Component::Row { children, .. }
            | Component::Column { children, .. }
            | Component::Grid { children, .. } => stack.extend(children),
            Component::Tree { items, .. } => {
                let mut pending = items.iter().collect::<Vec<_>>();
                while let Some(item) = pending.pop() {
                    ids.insert(item.id.clone());
                    pending.extend(&item.children);
                }
            }
            _ => {}
        }
    }
    ids
}

fn constraint(length: &Length, node: &Node, horizontal: bool) -> Constraint {
    match length {
        Length::Cells { cells } => Constraint::Length(*cells),
        Length::Percent { percent } => Constraint::Percentage(*percent),
        Length::Fill { fill } => Constraint::Fill(*fill),
        Length::Auto(_) => {
            if horizontal {
                Constraint::Fill(1)
            } else {
                Constraint::Length(natural_height(node))
            }
        }
    }
}

fn natural_height(node: &Node) -> u16 {
    match &node.content {
        Component::Column { children, gap } => children
            .iter()
            .map(natural_height)
            .fold(0, u16::saturating_add)
            .saturating_add(gap.saturating_mul(
                u16::try_from(children.len().saturating_sub(1)).unwrap_or(u16::MAX),
            )),
        Component::Row { children, .. } => children.iter().map(natural_height).max().unwrap_or(1),
        Component::List { items, .. } => u16::try_from(items.len().min(20)).unwrap_or(20),
        Component::Table { rows, .. } => {
            u16::try_from(rows.len().saturating_add(1).min(20)).unwrap_or(20)
        }
        Component::Tree { .. } | Component::Grid { .. } => 8,
        Component::Input { .. } => 2,
        Component::Text { text, .. } => {
            u16::try_from(text.lines().count().max(1)).unwrap_or(u16::MAX)
        }
        Component::Spacer { cells } => *cells,
        _ => 1,
    }
}

fn role(role: u8) -> Style {
    Style::default().fg(match role {
        1 => Color::DarkGray,
        2 => Color::Cyan,
        3 => Color::Green,
        4 => Color::Yellow,
        5 => Color::Red,
        6 => Color::Magenta,
        7 => Color::Blue,
        _ => Color::White,
    })
}

fn render_rows(frame: &mut Frame<'_>, area: Rect, items: &[String], selected: usize, style: Style) {
    let start = selected.saturating_sub(usize::from(area.height).saturating_sub(1));
    frame.render_widget(
        List::new(items.iter().enumerate().skip(start).map(|(i, text)| {
            ListItem::new(format!("{} {text}", if i == selected { ">" } else { " " })).style(
                if i == selected {
                    style.bg(Color::DarkGray)
                } else {
                    style
                },
            )
        })),
        area,
    );
}

fn tree_rows(items: &[TreeItem], state: &BTreeMap<String, bool>) -> Vec<(usize, String, String)> {
    let mut rows = Vec::new();
    let mut stack = items.iter().rev().map(|n| (n, 0)).collect::<Vec<_>>();
    while let Some((n, d)) = stack.pop() {
        let expanded = state.get(&n.id).copied().unwrap_or(n.expanded);
        rows.push((
            d,
            n.id.clone(),
            format!(
                "{} {}",
                if n.children.is_empty() {
                    " "
                } else if expanded {
                    "▾"
                } else {
                    "▸"
                },
                n.label
            ),
        ));
        if expanded {
            stack.extend(n.children.iter().rev().map(|n| (n, d + 1)));
        }
    }
    rows
}

fn envelope(kind: &str, id: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("kind".into(), kind.into()), ("id".into(), id.into())])
}

fn key_event(id: &str, event: InputEvent) -> BTreeMap<String, String> {
    let mut e = envelope("key", id);
    e.insert("key".into(), format!("{:?}", event.key));
    e.insert("control".into(), event.modifiers.control.to_string());
    e.insert("alt".into(), event.modifiers.alt.to_string());
    e.insert("shift".into(), event.modifiers.shift.to_string());
    e
}
#[cfg(test)]
mod tests;
