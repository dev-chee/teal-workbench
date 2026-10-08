@edition("2026");

// Cross-tier Showcase：面板从扩展状态读取数据并交给工作台 Host 渲染。
import "bench/extension.tl" as ext;
import "bench/ui.tl" as ui;
fn main() {
    ui::render(ui::node("greeting", ui::Component.text {
        text: "Hello from a Teal extension: " + ext::get("greeting"), style: ui::Style.heading, wrap: true
    }));
}
