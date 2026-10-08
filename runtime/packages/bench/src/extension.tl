@edition("2026");

// 工作台专用扩展 SDK；该命名空间不属于 Teal 标准库。
host "std.bench.extension" as host_api {
    fn event(key: string) -> string;
    fn config(key: string) -> string;
    fn get(key: string) -> string;
    fn set(key: string, value: string) -> string;
    fn start(name: string, input: string) -> string;
    fn start_bound(name: string, input: string, approvals_json: string) -> string;
    fn quote(value: string) -> string;
    fn task_status(key: string) -> string;
    fn cancel(key: string) -> string;
    fn resume(key: string) -> string;
    fn complete(value: string) -> string;
    fn message(value: string) -> string;
    fn request_approval(prompt: string, paths_json: string, operation: string, environment: string) -> string;
    fn approval(id: string) -> string;
}

/// 读取已捕获的事件字段；不存在的字段返回空字符串。
pub fn event(key: string) -> string { return host_api::event(key); }
/// 读取无秘密扩展配置。
pub fn config(key: string) -> string { return host_api::config(key); }
/// 读取本实例业务状态，不包含宿主收据。
pub fn get(key: string) -> string { return host_api::get(key); }
/// 暂存业务值；事件正常完成后才提交。
pub fn set(key: string, value: string) { host_api::set(key, value); }
/// 请求启动清单中声明的后台任务，返回稳定请求身份。
pub fn start(name: string, input: string) -> string { return host_api::start(name, input); }
/// 启动必须绑定指定审批的阶段；宿主在实际执行前重新核对文件摘要。
pub fn start_bound(name: string, input: string, approvals_json: string) -> string { return host_api::start_bound(name, input, approvals_json); }
/// 安全编码 JSON 字符串，包括引号、换行和控制字符。
pub fn quote(value: string) -> string { return host_api::quote(value); }
/// 查询本实例任务的捕获状态；界面文本不能代替运行收据。
pub fn task_status(key: string) -> string { return host_api::task_status(key); }
/// 在本次事件提交后请求取消指定任务。
pub fn cancel(key: string) { host_api::cancel(key); }
/// 在本次事件提交后从原身份显式继续任务。
pub fn resume(key: string) { host_api::resume(key); }
/// 显式提交阶段结果；运行正常终止后才可被扩展接受。
pub fn complete(value: string) { host_api::complete(value); }
/// 显示消息；消息不能作为任务完成或授权凭据。
pub fn message(value: string) { host_api::message(value); }
/// 请求宿主审批，paths_json 为工作区相对路径数组；不自行授予权限。
pub fn request_approval(prompt: string, paths_json: string, operation: string, environment: string) -> string {
    return host_api::request_approval(prompt, paths_json, operation, environment);
}
/// 读取宿主审批状态：missing、pending、approved 或 rejected。
pub fn approval(id: string) -> string { return host_api::approval(id); }
