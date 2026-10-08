//! 被动 hover 浮窗。
//!
//! hover 是"弹出即窗口"的被动形态：它登记为一个瞬态浮动窗口用于身份与生命周期，
//! 但不接收焦点、不参与标签条，绘制在内容区之上并跟随光标位置。

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use teal_workbench_core::{WindowId, WindowSet};

/// 当前显示的 hover 内容与它的窗口身份。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HoverView {
    window: Option<WindowId>,
    text: String,
}

impl HoverView {
    /// 显示新的 hover 文本，并登记一个瞬态被动窗口。
    ///
    /// 已有窗口就地复用，避免每次请求都累积窗口。
    pub fn show(&mut self, windows: &mut WindowSet, text: impl Into<String>) {
        self.text = text.into();
        if let Some(window) = self.window
            && windows.get(window).is_some()
        {
            return;
        }
        let descriptor = popup_window("Hover", self.text.clone());
        self.window = Some(windows.insert_passive(descriptor));
    }

    /// 关闭 hover 浮窗并清除内容。
    pub fn hide(&mut self, windows: &mut WindowSet) {
        if let Some(window) = self.window.take() {
            let _ = windows.close_passive(window);
        }
        self.text.clear();
    }

    /// 返回当前 hover 窗口标识。
    #[must_use]
    pub const fn window(&self) -> Option<WindowId> {
        self.window
    }

    /// 返回当前 hover 文本。
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 返回是否正在显示 hover。
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.window.is_some() && !self.text.is_empty()
    }

    /// 在内容区之上绘制 hover 浮窗。
    ///
    /// 宽度取内容区的 60% 且不超过文本自然宽度，位置靠近 `anchor` 并钳制回内容区。
    pub fn render(&self, frame: &mut Frame<'_>, content: Rect, anchor: Option<(u16, u16)>) {
        if !self.is_visible() || content.width == 0 || content.height == 0 {
            return;
        }
        let natural = u16::try_from(self.text.chars().count()).unwrap_or(u16::MAX);
        let width = natural
            .min(content.width.saturating_mul(60) / 100)
            .max(20)
            .min(content.width);
        let wrapped = u16::try_from(self.text.chars().count() / usize::from(width.max(1)) + 1)
            .unwrap_or(u16::MAX)
            .saturating_add(2)
            .min(content.height);
        let height = wrapped.max(3);
        let (x, y) = anchor.unwrap_or((content.x, content.y));
        let x = x
            .min(
                content
                    .x
                    .saturating_add(content.width)
                    .saturating_sub(width),
            )
            .max(content.x);
        let area = Rect {
            x,
            y: y.saturating_add(1).min(
                content
                    .y
                    .saturating_add(content.height)
                    .saturating_sub(height),
            ),
            width,
            height,
        };
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(Line::from(self.text.clone()))
                .wrap(Wrap { trim: false })
                .block(
                    Block::default()
                        .title("Hover")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::DarkGray)),
                ),
            area,
        );
    }
}

/// 构造一个被动 hover 窗口骨架。
fn popup_window(title: &str, _text: String) -> teal_workbench_core::Window {
    use teal_workbench_core::{
        LayoutTree, PanelId, Window, WindowChrome, WindowInteraction, WindowKind,
    };

    let mut window = Window::new(
        WindowKind::Editor,
        title.to_owned(),
        LayoutTree::new(PanelId::default()),
    );
    window.set_interaction(WindowInteraction::Passive);
    window.set_chrome(WindowChrome::None);
    window
}

#[cfg(test)]
mod tests {
    use super::HoverView;
    use teal_workbench_core::{LayoutTree, PanelId, Window, WindowKind, WindowSet};

    fn windows() -> WindowSet {
        WindowSet::new(Window::new(
            WindowKind::Editor,
            "primary",
            LayoutTree::new(PanelId::default()),
        ))
    }

    #[test]
    fn hover_registers_a_passive_window_and_reuses_it() {
        let mut set = windows();
        let mut hover = HoverView::default();
        assert!(!hover.is_visible());

        hover.show(&mut set, "fn main()");
        let first = hover.window().expect("hover window");
        assert!(hover.is_visible());
        assert!(set.get(first).is_some_and(|window| window.is_passive()));
        // 被动窗口不进入标签顺序，也不参与轮转。
        assert_eq!(set.order(), set.order());
        assert!(!set.select(first));

        // 再次显示复用同一窗口，不累积。
        hover.show(&mut set, "fn main() -> int");
        assert_eq!(hover.window(), Some(first));
        assert_eq!(hover.text(), "fn main() -> int");

        hover.hide(&mut set);
        assert!(!hover.is_visible());
        assert!(set.get(first).is_none());
    }
}
