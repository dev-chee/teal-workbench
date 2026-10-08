@edition("2026");

// Cross-tier Showcase：工作模式把工作台动作路由到 Teal 阶段状态机。
import "bench/extension.tl" as ext;
import "flow.tl" as flow;
fn main() {
    let action = ext::event("action");
    if ext::event("kind") == "command" && action == "" { action = "command"; }
    flow::dispatch(action, ext::event("input"));
}
