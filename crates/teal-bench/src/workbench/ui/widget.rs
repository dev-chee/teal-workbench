//! Widget 契约与组合容器。
//!
//! Panel 是布局结点，只负责几何、焦点与边框；真正的 UI 功能由 Widget 承担。
//! 容器 Widget 按 `Length` 约束切分区域并转发输入、光标与绘制，因此复杂界面可以
//! 由小组件组合而成，而不必为每种布局新增面板类别。

use ratatui::{Frame, layout::Rect as TuiRect, widgets::Paragraph};
use teal_workbench_core::{InputEvent, MouseEvent, PanelSize, Rect, SplitAxis, split_rectangles};

use super::panel::{
    ClipboardAction, ClipboardOutcome, ContextMenu, PanelKind, PanelStatus, WidgetAction,
};

/// 面板宿主绘制的边框与状态信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WidgetChrome {
    /// 会话与布局使用的稳定类别。
    pub kind: PanelKind,

    /// 边框标题。
    pub title: String,

    /// 可选状态行字段。
    pub status: Option<PanelStatus>,
}

impl WidgetChrome {
    /// 使用类别与标题创建边框信息。
    #[must_use]
    pub fn new(kind: PanelKind, title: impl Into<String>) -> Self {
        Self {
            kind,
            title: title.into(),
            status: None,
        }
    }

    /// 附加状态行字段。
    #[must_use]
    pub fn with_status(mut self, status: PanelStatus) -> Self {
        self.status = Some(status);
        self
    }
}

/// 存储在布局叶节点中、可渲染且可聚焦的 UI 单元。
///
/// 实现者只描述自己的内容与输入行为；边框、状态行与几何由 Widget 宿主提供。
pub trait Widget {
    /// 返回稳定组件标识，用于宿主查询或后续组合；默认没有标识。
    fn id(&self) -> Option<&'static str> {
        None
    }

    /// 非阻塞地收取局部任务结果，返回延迟的全局操作。
    fn poll_action(&mut self) -> WidgetAction {
        WidgetAction::Ignored
    }

    /// 返回此 Widget 的类别。
    fn kind(&self) -> PanelKind;

    /// 返回边框上显示的标题。
    fn title(&self) -> String;

    /// 当 Widget 有输入或元数据时返回状态字段。
    fn status(&self) -> Option<PanelStatus>;

    /// 返回宿主绘制边框与状态行所需的信息；默认由上面三项组合。
    fn chrome(&self) -> WidgetChrome {
        WidgetChrome {
            kind: self.kind(),
            title: self.title(),
            status: self.status(),
        }
    }

    /// 路由一个规范化输入事件。
    fn handle_input(&mut self, event: InputEvent) -> WidgetAction;

    /// 路由一个规范化绝对坐标鼠标事件。
    fn handle_mouse(&mut self, _event: MouseEvent, _area: TuiRect) -> WidgetAction {
        WidgetAction::Ignored
    }

    /// 应用读取所有粘贴负载后执行剪贴板操作。
    fn handle_clipboard(
        &mut self,
        _action: ClipboardAction,
        _paste: Option<&str>,
    ) -> ClipboardOutcome {
        ClipboardOutcome::default()
    }

    /// 应用由此 Widget 拥有的上下文菜单项。
    fn handle_menu(&mut self, _id: &str) -> WidgetAction {
        WidgetAction::Ignored
    }

    /// 渲染 Widget 正文，不包括宿主绘制的状态行。
    fn render(&mut self, frame: &mut Frame<'_>, area: TuiRect, focused: bool);

    /// 返回相对于 `area` 的可选光标单元格。
    fn cursor(&self, _area: TuiRect) -> Option<(u16, u16)> {
        None
    }

    /// 在组件内部推进焦点；返回是否消费该 Tab。
    fn focus_next(&mut self, _backwards: bool) -> bool {
        false
    }
}

/// 按约束切分区域的子节点。
struct Child {
    size: PanelSize,
    widget: Box<dyn Widget>,
}

/// 按主轴顺序排列子 Widget 的容器。
pub struct Split {
    axis: SplitAxis,
    children: Vec<Child>,
    focus: usize,
}

impl Split {
    /// 创建纵向（上下）排列的容器。
    #[must_use]
    pub fn rows(children: Vec<(PanelSize, Box<dyn Widget>)>) -> Self {
        Self::new(SplitAxis::Rows, children)
    }

    /// 创建横向（左右）排列的容器。
    #[must_use]
    pub fn columns(children: Vec<(PanelSize, Box<dyn Widget>)>) -> Self {
        Self::new(SplitAxis::Columns, children)
    }

    fn new(axis: SplitAxis, children: Vec<(PanelSize, Box<dyn Widget>)>) -> Self {
        Self {
            axis,
            children: children
                .into_iter()
                .map(|(size, widget)| Child { size, widget })
                .collect(),
            focus: 0,
        }
    }

    /// 返回主轴与约束是否可用于切分。
    fn layout(&self, area: TuiRect) -> Vec<TuiRect> {
        let sizes = self
            .children
            .iter()
            .map(|child| child.size)
            .collect::<Vec<_>>();
        let core_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: area.height,
        };
        split_rectangles(self.axis, &sizes, core_area)
            .into_iter()
            .map(|rect| TuiRect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            })
            .collect()
    }
}

impl Widget for Split {
    fn kind(&self) -> PanelKind {
        self.children
            .get(self.focus)
            .map_or(PanelKind::Custom, |child| child.widget.kind())
    }

    fn title(&self) -> String {
        self.children
            .get(self.focus)
            .map_or_else(String::new, |child| child.widget.title())
    }

    fn status(&self) -> Option<PanelStatus> {
        self.children
            .get(self.focus)
            .and_then(|child| child.widget.status())
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: TuiRect, focused: bool) {
        let areas = self.layout(area);
        for (child, child_area) in self.children.iter_mut().zip(areas) {
            child.widget.render(frame, child_area, focused);
        }
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        match self.children.get_mut(self.focus) {
            Some(child) => child.widget.handle_input(event),
            None => WidgetAction::Ignored,
        }
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: TuiRect) -> WidgetAction {
        let areas = self.layout(area);
        for (index, (child, child_area)) in self.children.iter_mut().zip(areas).enumerate() {
            if child_area.contains(ratatui::layout::Position::new(event.column, event.row)) {
                self.focus = index;
                return child.widget.handle_mouse(event, child_area);
            }
        }
        WidgetAction::Ignored
    }

    fn cursor(&self, area: TuiRect) -> Option<(u16, u16)> {
        let child = self.children.get(self.focus)?;
        let child_area = self.layout(area).get(self.focus).copied()?;
        let (x, y) = child.widget.cursor(child_area)?;
        Some((
            x.saturating_sub(child_area.x.saturating_sub(area.x)),
            y.saturating_sub(child_area.y.saturating_sub(area.y)),
        ))
    }

    fn focus_next(&mut self, backwards: bool) -> bool {
        if self.children.is_empty() {
            return false;
        }
        if self.children[self.focus].widget.focus_next(backwards) {
            return true;
        }
        let len = self.children.len();
        self.focus = if backwards {
            (self.focus + len - 1) % len
        } else {
            (self.focus + 1) % len
        };
        true
    }
}

/// 在给定区域内水平排布固定文本的容器。
pub struct StatusLine {
    text: String,
}

impl StatusLine {
    /// 使用初始文本创建状态行。
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// 改写状态行文本。
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }

    /// 返回当前文本。
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl Widget for StatusLine {
    fn kind(&self) -> PanelKind {
        PanelKind::Output
    }

    fn title(&self) -> String {
        String::new()
    }

    fn status(&self) -> Option<PanelStatus> {
        None
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: TuiRect, _focused: bool) {
        frame.render_widget(Paragraph::new(self.text.clone()), area);
    }

    fn handle_input(&mut self, _event: InputEvent) -> WidgetAction {
        WidgetAction::Ignored
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use ratatui::{Terminal, backend::TestBackend, layout::Position, widgets::Paragraph};
    use ratatui::layout::Rect as TuiRect;
    use teal_workbench_core::{
        InputEvent, Key, KeyModifiers, MouseButton, MouseEvent, MouseEventKind, PanelSize,
    };

    use super::{Split, StatusLine, Widget, WidgetChrome};
    use crate::workbench::ui::{PanelKind, PanelStatus, WidgetAction};

    struct Probe {
        label: &'static str,
        inputs: Rc<Cell<usize>>,
    }

    impl Widget for Probe {
        fn kind(&self) -> PanelKind {
            PanelKind::Output
        }

        fn title(&self) -> String {
            self.label.to_owned()
        }

        fn status(&self) -> Option<PanelStatus> {
            Some(PanelStatus {
                mode: self.label.to_owned(),
                resource: String::new(),
                position: String::new(),
            })
        }

        fn render(&mut self, frame: &mut ratatui::Frame<'_>, area: TuiRect, _focused: bool) {
            frame.render_widget(Paragraph::new(self.label), area);
        }

        fn handle_input(&mut self, _event: InputEvent) -> WidgetAction {
            self.inputs.set(self.inputs.get() + 1);
            WidgetAction::Consumed
        }

        fn handle_mouse(&mut self, _event: MouseEvent, _area: TuiRect) -> WidgetAction {
            self.inputs.set(self.inputs.get() + 1);
            WidgetAction::Consumed
        }
    }

    fn key() -> InputEvent {
        InputEvent {
            key: Key::Char('x'),
            modifiers: KeyModifiers::default(),
        }
    }

    #[test]
    fn split_places_children_and_routes_input_to_the_focused_one() {
        let top_inputs = Rc::new(Cell::new(0));
        let bottom_inputs = Rc::new(Cell::new(0));
        let mut split = Split::rows(vec![
            (
                PanelSize::fill(1),
                Box::new(Probe {
                    label: "top",
                    inputs: Rc::clone(&top_inputs),
                }) as Box<dyn Widget>,
            ),
            (
                PanelSize::cells(3),
                Box::new(Probe {
                    label: "bottom",
                    inputs: Rc::clone(&bottom_inputs),
                }),
            ),
        ]);
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| split.render(frame, frame.area(), true))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), "t");
        assert_eq!(buffer[(0, 6)].symbol(), " ");
        assert_eq!(buffer[(0, 7)].symbol(), "b");
        assert_eq!(buffer[(0, 9)].symbol(), " ");

        // 默认聚焦第一个子节点。
        split.handle_input(key());
        assert_eq!(top_inputs.get(), 1);
        assert_eq!(bottom_inputs.get(), 0);

        // 点击第二个子节点把焦点移过去。
        split.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 2,
                row: 8,
                modifiers: KeyModifiers::default(),
                click_count: 1,
            },
            TuiRect::new(0, 0, 40, 10),
        );
        split.handle_input(key());
        assert_eq!(
            bottom_inputs.get(),
            2,
            "the click is routed to the second child and the following key stays there"
        );

        // Tab 在子节点之间轮流。
        assert!(split.focus_next(false));
        split.handle_input(key());
        assert_eq!(top_inputs.get(), 2);
        assert!(
            TuiRect::new(0, 0, 40, 10).contains(Position::new(0, 0)),
            "the probe area must stay inside the split area"
        );
    }

    #[test]
    fn chrome_defaults_and_builders_stay_consistent() {
        let status_line = StatusLine::new("ready");
        let chrome = status_line.chrome();
        assert_eq!(chrome.kind, PanelKind::Output);
        assert!(chrome.title.is_empty());
        assert!(chrome.status.is_none());
        assert_eq!(status_line.id(), None);

        let chrome = WidgetChrome::new(PanelKind::Agent, "Agent").with_status(PanelStatus {
            mode: "chat".to_owned(),
            resource: "flow".to_owned(),
            position: "1/1".to_owned(),
        });
        assert_eq!(chrome.title, "Agent");
        assert_eq!(
            chrome.status.map(|status| status.mode),
            Some("chat".to_owned())
        );
    }
}
