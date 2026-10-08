use std::path::PathBuf;
use tokio::sync::oneshot;
use super::MutationKind;

/// 返回界面/编辑器事务边界的智能体修改。
pub struct MutationRequest {
    /// 跨修改队列保留的发起工具上下文，永不序列化。
    pub observation: crate::tracing_context::Context,
    /// 已解析的工作区路径。
    pub path: PathBuf,
    /// 请求的内容修改。
    pub kind: MutationKind,
    /// 工具回调建立时打开缓冲区的内容版本；执行前必须仍然匹配。
    pub expected_buffer_version: Option<String>,
    /// 工具回调建立时磁盘或打开缓冲区的规范化文本摘要；审查等待期间必须仍然匹配。
    pub expected_content_sha256: Option<String>,
    /// 编辑器/磁盘修改完成后发送的完成通知。
    pub response: oneshot::Sender<Result<(), String>>,
}

/// 送往应用事件循环进行分级审查的多文件 ChangeSet。
pub struct ChangeSetRequest {
    /// 跨审查和修改队列保留的发起工具上下文。
    pub observation: crate::tracing_context::Context,
    /// 已绑定路径、摘要和 hunk 身份的候选变更。
    pub change_set: teal_workbench_core::ChangeSet,
    /// 审查并提交后的结果；成功文本说明实际采用的选择范围。
    pub response: oneshot::Sender<Result<String, String>>,
}
