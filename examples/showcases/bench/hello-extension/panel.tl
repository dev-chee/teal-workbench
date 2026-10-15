@edition("2026");

// Cross-tier Showcase：面板从扩展状态读取数据并交给工作台 Host 渲染。
import "bench/extension.tl" as ext;
import "bench/ui.tl" as ui;
fn application() -> none | Error {
    ui::render(ui::node("greeting", ui::Component.text {
        text: "Hello from a Teal extension: " + ext::get("greeting") ?,
        style: ui::Style.heading,
        wrap: true
    })) ?;
}

/// 入口明确处理失败；未知外部效果仍由 VM 终止。
fn main() -> int {
    let result = application();
    match result {
        Error { .. } => {
            print(result.message);
            return 1;
        } _ => {
            return 0;
        }
    }
}
