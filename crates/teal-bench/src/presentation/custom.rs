//! 已注册 Teal 面板与通用声明式组件之间的适配。
//!
//! 面板保留最近有效规格和控件状态，把类型化事件交回脚本入口；无效更新不替换当前树。

use ratatui::{Frame, layout::Rect};
use teal_workbench_core::{InputEvent, MouseEvent};
use crate::workbench::ui::{DeclarativePanel, Widget, WidgetAction, PanelKind, PanelStatus};

pub(crate) struct CustomPanel {
    pub(crate) name: String,
    pub(crate) ui: DeclarativePanel,
}

impl CustomPanel {
    pub(crate) fn new(name: String) -> Self {
        let spec = serde_json::json!({"version":1,"root":{"id":"loading","width":{"fill":1},"height":"auto","content":{"kind":"text","text":"Loading panels…","style":1,"wrap":true}}});
        Self {
            name,
            ui: DeclarativePanel::from_json(&spec.to_string()).expect("Static custom panel"),
        }
    }
}

impl Widget for CustomPanel {
    fn cursor(&self, area: Rect) -> Option<(u16, u16)> {
        self.ui
            .cursor()
            .map(|(x, y)| (x.saturating_sub(area.x), y.saturating_sub(area.y)))
    }

    fn kind(&self) -> PanelKind {
        PanelKind::Custom
    }

    fn title(&self) -> String {
        self.name.clone()
    }

    fn status(&self) -> Option<PanelStatus> {
        Some(PanelStatus {
            mode: "Teal".into(),
            resource: self.name.clone(),
            position: String::new(),
        })
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        self.ui
            .input(event)
            .map_or(WidgetAction::Consumed, WidgetAction::CustomEvent)
    }

    fn handle_mouse(&mut self, event: MouseEvent, _area: Rect) -> WidgetAction {
        self.ui
            .mouse(event)
            .map_or(WidgetAction::Ignored, WidgetAction::CustomEvent)
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, _focused: bool) {
        self.ui.render(frame, area);
    }
}
