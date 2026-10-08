//! Teal 工作台的模态终端编程环境。
//!
//! 根包负责命令行解析并通过 [`StartupOptions`] 启动本库；本库负责进程级观测、Tokio
//! 运行时和终端恢复。`App` 负责应用初始化、事件循环、会话策略及服务清理；这些操作在
//! `Runtime::block_on` 内执行，随后才关闭运行时和观测提供器。
//!
//! 正常退出顺序为应用清理、终端恢复、运行时关闭、观测关闭。错误返回会保留最初的
//! 启动或事件循环错误，同时仍尝试执行适用的清理。

mod app;
pub mod config;
mod lifecycle;
mod terminal;
mod presentation;
mod runtime_bridge;
mod integration;
mod workbench;
#[cfg(test)]
mod observability_protocol;

#[cfg(test)]
use app::AppOptions;

pub use lifecycle::run;

/// 启动终端工作台所需的、与命令行解析器无关的选项。
pub struct StartupOptions {
    /// 可选文件或目录；省略时恢复工作区会话或使用当前目录。
    pub path: Option<std::path::PathBuf>,
    /// 禁用启动脚本、Agent、内建语法和会话恢复。
    pub safe_mode: bool,
    /// 禁用用户及工作区 Teal 配置。
    pub no_config: bool,
    /// 禁用远程模型、网络、遥测导出和扩展副作用。
    pub offline: bool,
    /// 显式指定的会话 JSON 文件。
    pub session: Option<std::path::PathBuf>,
    /// 是否由工作台捕获终端鼠标事件。
    pub capture_mouse: bool,
    /// 进程日志与可选遥测导出的启动参数。
    pub observation: teal_workbench_services::observability::Arguments,
    /// 显式启用的模型协议抓包根目录。
    pub model_protocol_capture: Option<std::path::PathBuf>,
}
