//! 组合容器 Widget。
//!
//! 容器只负责把父区域切分给子 Widget 并转发绘制、输入、鼠标命中、光标与 Tab 焦点，
//! 不承担业务语义。`Split` 负责并排/上下排列，这里补齐边框、内边距与叠放三种组合。

use ratatui::{
    Frame,
    layout::Rect as TuiRect,
    style::{Color, Style},
    widgets::{Block, Borders, Clear},
};
use teal_workbench_core::{InputEvent, MouseEvent};

use super::panel::{ClipboardAction, ClipboardOutcome, PanelKind, PanelStatus, WidgetAction};
use super::widget::{Widget, WidgetChrome};

/// 为子 Widget 绘制边框并把它放入边框内侧。
pub struct Border {
    title: String,
    focused_color: Color,
    child: Box<dyn Widget>,
}

impl Border {
    /// 用标题和子 Widget 创建边框容器。
    #[must_use]
    pub fn new(title: impl Into<String>, child: Box<dyn Widget>) -> Self {
        Self {
            title: title.into(),
            focused_color: Color::Cyan,
            child,
        }
    }

    /// 改写聚焦边框颜色。
    #[must_use]
    pub fn with_focused_color(mut self, color: Color) -> Self {
        self.focused_color = color;
        self
    }

    /// 返回子区域：边框内侧。
    fn inner(area: TuiRect) -> TuiRect {
        Block::default().borders(Borders::ALL).inner(area)
    }
}

impl Widget for Border {
    fn kind(&self) -> PanelKind {
        self.child.kind()
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn status(&self) -> Option<PanelStatus> {
        self.child.status()
    }

    fn chrome(&self) -> WidgetChrome {
        WidgetChrome::new(self.kind(), self.title())
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: TuiRect, focused: bool) {
        let block = Block::default()
            .title(self.title.as_str())
            .borders(Borders::ALL)
            .border_style(if focused {
                Style::default().fg(self.focused_color)
            } else {
                Style::default().fg(Color::DarkGray)
            });
        frame.render_widget(block, area);
        let inner = Self::inner(area);
        if inner.width > 0 && inner.height > 0 {
            self.child.render(frame, inner, focused);
        }
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        self.child.handle_input(event)
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: TuiRect) -> WidgetAction {
        let inner = Self::inner(area);
        if inner.width == 0 || inner.height == 0 {
            return WidgetAction::Ignored;
        }
        self.child.handle_mouse(event, inner)
    }

    fn handle_clipboard(
        &mut self,
        action: ClipboardAction,
        paste: Option<&str>,
    ) -> ClipboardOutcome {
        self.child.handle_clipboard(action, paste)
    }

    fn handle_menu(&mut self, id: &str) -> WidgetAction {
        self.child.handle_menu(id)
    }

    fn cursor(&self, area: TuiRect) -> Option<(u16, u16)> {
        let inner = Self::inner(area);
        let (x, y) = self.child.cursor(inner)?;
        Some((
            x.saturating_add(inner.x.saturating_sub(area.x)),
            y.saturating_add(inner.y.saturating_sub(area.y)),
        ))
    }

    fn focus_next(&mut self, backwards: bool) -> bool {
        self.child.focus_next(backwards)
    }
}

/// 按行列裁剪子 Widget 并转发事件。
pub struct Padding {
    horizontal: u16,
    vertical: u16,
    child: Box<dyn Widget>,
}

impl Padding {
    /// 用均匀内边距创建容器。
    #[must_use]
    pub fn new(horizontal: u16, vertical: u16, child: Box<dyn Widget>) -> Self {
        Self {
            horizontal,
            vertical,
            child,
        }
    }

    /// 返回去除内边距后的子区域。
    fn inner(&self, area: TuiRect) -> TuiRect {
        TuiRect {
            x: area.x.saturating_add(self.horizontal),
            y: area.y.saturating_add(self.vertical),
            width: area.width.saturating_sub(self.horizontal.saturating_mul(2)),
            height: area.height.saturating_sub(self.vertical.saturating_mul(2)),
        }
    }
}

impl Widget for Padding {
    fn kind(&self) -> PanelKind {
        self.child.kind()
    }

    fn title(&self) -> String {
        self.child.title()
    }

    fn status(&self) -> Option<PanelStatus> {
        self.child.status()
    }

    fn chrome(&self) -> WidgetChrome {
        self.child.chrome()
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: TuiRect, focused: bool) {
        let inner = self.inner(area);
        if inner.width > 0 && inner.height > 0 {
            self.child.render(frame, inner, focused);
        }
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        self.child.handle_input(event)
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: TuiRect) -> WidgetAction {
        let inner = self.inner(area);
        if inner.width == 0 || inner.height == 0 {
            return WidgetAction::Ignored;
        }
        self.child.handle_mouse(event, inner)
    }

    fn cursor(&self, area: TuiRect) -> Option<(u16, u16)> {
        let inner = self.inner(area);
        let (x, y) = self.child.cursor(inner)?;
        Some((
            x.saturating_add(inner.x.saturating_sub(area.x)),
            y.saturating_add(inner.y.saturating_sub(area.y)),
        ))
    }

    fn focus_next(&mut self, backwards: bool) -> bool {
        self.child.focus_next(backwards)
    }
}

/// 在当前区域内叠放子 Widget 的容器。
///
/// 子级按顺序绘制在同一个区域上，最后一个可见子级拥有焦点；用于把被动内容
/// （提示、状态叠加）放在基础内容之上而不改变布局。
pub struct Stack {
    layers: Vec<Box<dyn Widget>>,
    focus: usize,
}

impl Stack {
    /// 用叠放顺序创建容器。
    #[must_use]
    pub fn new(layers: Vec<Box<dyn Widget>>) -> Self {
        Self { layers, focus: 0 }
    }

    /// 聚焦指定的层，使其获得输入。
    pub fn focus_layer(&mut self, index: usize) -> bool {
        if index < self.layers.len() {
            self.focus = index;
            true
        } else {
            false
        }
    }

    /// 返回当前聚焦层下标。
    #[must_use]
    pub const fn focused_layer(&self) -> usize {
        self.focus
    }
}

impl Widget for Stack {
    fn kind(&self) -> PanelKind {
        self.layers
            .get(self.focus)
            .map_or(PanelKind::Custom, |layer| layer.kind())
    }

    fn title(&self) -> String {
        self.layers
            .get(self.focus)
            .map_or_else(String::new, |layer| layer.title())
    }

    fn status(&self) -> Option<PanelStatus> {
        self.layers.get(self.focus).and_then(|layer| layer.status())
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: TuiRect, focused: bool) {
        for layer in &mut self.layers {
            layer.render(frame, area, focused);
        }
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        match self.layers.get_mut(self.focus) {
            Some(layer) => layer.handle_input(event),
            None => WidgetAction::Ignored,
        }
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: TuiRect) -> WidgetAction {
        match self.layers.get_mut(self.focus) {
            Some(layer) => layer.handle_mouse(event, area),
            None => WidgetAction::Ignored,
        }
    }

    fn cursor(&self, area: TuiRect) -> Option<(u16, u16)> {
        self.layers
            .get(self.focus)
            .and_then(|layer| layer.cursor(area))
    }

    fn focus_next(&mut self, backwards: bool) -> bool {
        if self.layers.len() < 2 {
            return self
                .layers
                .get_mut(self.focus)
                .is_some_and(|layer| layer.focus_next(backwards));
        }
        if self
            .layers
            .get_mut(self.focus)
            .is_some_and(|layer| layer.focus_next(backwards))
        {
            return true;
        }
        let len = self.layers.len();
        self.focus = if backwards {
            (self.focus + len - 1) % len
        } else {
            (self.focus + 1) % len
        };
        true
    }
}

/// 清除叠放区域后再绘制的透明度辅助；供 `Stack` 上层使用。
pub fn clear_area(frame: &mut Frame<'_>, area: TuiRect) {
    frame.render_widget(Clear, area);
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use ratatui::{Terminal, backend::TestBackend, layout::Rect as TuiRect, widgets::Paragraph};
    use teal_workbench_core::{InputEvent, Key, KeyModifiers};

    use super::{Border, Padding, Stack};
    use crate::workbench::ui::{PanelKind, PanelStatus, Widget, WidgetAction};

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
            None
        }

        fn render(&mut self, frame: &mut ratatui::Frame<'_>, area: TuiRect, _focused: bool) {
            frame.render_widget(Paragraph::new(self.label), area);
        }

        fn handle_input(&mut self, _event: InputEvent) -> WidgetAction {
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
    fn border_draws_frame_and_delegates_input_to_the_inner_area() {
        let inputs = Rc::new(Cell::new(0));
        let mut border = Border::new(
            "Panel",
            Box::new(Probe {
                label: "body",
                inputs: Rc::clone(&inputs),
            }),
        );
        let backend = TestBackend::new(20, 6);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| border.render(frame, frame.area(), true))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        // 边框占据第一行，正文写入内侧。
        assert_eq!(buffer[(0, 0)].symbol(), "\u{250c}");
        assert_eq!(buffer[(1, 1)].symbol(), "b");

        border.handle_input(key());
        assert_eq!(inputs.get(), 1);
        assert_eq!(border.title(), "Panel");
        assert_eq!(border.kind(), PanelKind::Output);
    }

    #[test]
    fn padding_clips_the_child_area_and_stack_forwards_to_the_focused_layer() {
        let inputs = Rc::new(Cell::new(0));
        let mut padding = Padding::new(
            2,
            1,
            Box::new(Probe {
                label: "body",
                inputs: Rc::clone(&inputs),
            }),
        );
        let area = TuiRect::new(0, 0, 20, 6);
        let inner = TuiRect {
            x: 2,
            y: 1,
            width: 16,
            height: 4,
        };
        assert_eq!(padding.cursor(area), None);
        let backend = TestBackend::new(20, 6);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| padding.render(frame, area, false))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(2, 1)].symbol(), "b");
        assert_eq!(buffer[(0, 0)].symbol(), " ");

        // 叠放容器把输入交给聚焦层，切层后转发目标改变。
        let first = Rc::new(Cell::new(0));
        let second = Rc::new(Cell::new(0));
        let mut stack = Stack::new(vec![
            Box::new(Probe {
                label: "base",
                inputs: Rc::clone(&first),
            }),
            Box::new(Probe {
                label: "overlay",
                inputs: Rc::clone(&second),
            }),
        ]);
        stack.handle_input(key());
        assert_eq!(first.get(), 1);
        assert_eq!(second.get(), 0);
        assert!(stack.focus_layer(1));
        assert_eq!(stack.focused_layer(), 1);
        stack.handle_input(key());
        assert_eq!(first.get(), 1);
        assert_eq!(second.get(), 1);
        assert!(!stack.focus_layer(3));
        assert!(inner.width == 16);
    }
}
