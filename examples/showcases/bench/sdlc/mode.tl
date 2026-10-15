@edition("2026");

// Cross-tier Showcase：工作模式把工作台动作路由到 Teal 阶段状态机。
import "bench/extension.tl" as ext;
import "flow.tl" as flow;
fn application() -> none | Error {
    let action = ext::event("action") ?;
    if ext::event("kind") ? == "command" && action == "" {
        action = "command";
    }
    flow::dispatch(action, ext::event("input") ?) ?;
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
