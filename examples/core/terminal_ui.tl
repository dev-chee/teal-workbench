@edition("2026");

// Tier Core：编码一个最小 std/ui.tl 组件树，不执行真实终端 I/O。
import "std/ui.tl" as ui;
task application() -> none | Error {
    let title = ui::node("title", ui::Component.text { text: "Hello UI", style: ui::Style.heading, wrap: true });
    let action = ui::node("refresh", ui::Component.button { label: "Refresh", enabled: true });
    let root = ui::node("root", ui::Component.column { children: [title, action], gap: 1 });
    print(ui::encode(root) ?);
}

/// 入口明确处理失败；未知外部效果仍由 VM 终止。
task main() -> int {
    let result = application().await;
    match result {
        Error { .. } => {
            print(result.message);
            return 1;
        } _ => {
            return 0;
        }
    }
}
