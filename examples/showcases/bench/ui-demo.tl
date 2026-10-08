@edition("2026");

// Cross-tier Showcase：工作台注册与渲染桥接；先批准面板脚本，再加载本文件。
// 执行 `:panel demo` 时 Host 会独立校验 ui-demo-panel.tl 的授权。
host "std.bench.config" as config { fn register_panel(arg0: string, arg1: string); }
fn main() { config::register_panel("demo", "ui-demo-panel.tl"); }
