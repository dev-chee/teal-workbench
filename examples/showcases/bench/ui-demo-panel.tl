@edition("2026");

// Cross-tier Showcase：工作台提供当前事件，Teal 计算下一帧类型化组件树。
import "bench/ui.tl" as ui;
fn main() {
    let title = "Teal Bench UI demo";
    match ui::event() {
        ui::UiEvent.activate { id } => { title = "Activated: " + id; }
        ui::UiEvent.mount => {}
        _ => { return; }
    }
    ui::render(ui::node("root", ui::Component.column { children: [
        ui::node("title", ui::Component.text { text: title, style: ui::Style.heading, wrap: true }),
        ui::node("name", ui::Component.input { value: "", placeholder: "Name" }),
        ui::node("go", ui::Component.button { label: "Run", enabled: true })
    ], gap: 1 }));
}
