@edition("2026");

import "std/ui.tl" as ui;
pub ui::Node;
pub ui::Component;
pub ui::TreeItem;
pub ui::Style;
pub ui::Length;
pub ui::UiEvent;
pub ui::node;
host "std.bench.panel" as backend {
    fn render(arg0: string);
    fn event() -> {string: string};
}
pub fn render(root: ui::Node) { backend::render(ui::encode(root)); }
pub fn event() -> ui::UiEvent { return ui::event(backend::event()); }
