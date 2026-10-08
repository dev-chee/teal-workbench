@edition("2026");

// 结构化证据由工作台宿主产生，未知副作用遵循 RuntimeStore 恢复规则。
host "std.bench.evidence" as backend {
    fn capture(paths_json: string) -> string;
    task command(name: string, approval: string) -> string;
    task git(action: string, paths_json: string, message: string, approval: string) -> string;
}
/// 取得工作区普通文件的路径与内容摘要。
pub fn capture(paths_json: string) -> string { return backend::capture(paths_json); }
/// 执行配置中定义的命令；仍需宿主批准，返回结构化收据与输出。
pub task command(name: string, approval: string) -> string { return backend::command(name, approval).await; }
/// 开始分支、读取状态或提交精确批准的文件集合。
pub task git(action: string, paths_json: string, message: string, approval: string) -> string {
    return backend::git(action, paths_json, message, approval).await;
}
