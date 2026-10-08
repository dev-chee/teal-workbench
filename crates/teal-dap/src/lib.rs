//! Teal 的源码级 Debug Adapter Protocol 协议库。
#![deny(missing_docs)]
//! Teal 的源码级 Debug Adapter Protocol 支持。
//!
//! `teal-dap` 将面向编辑器的 adapter 与终端拥有的 VM 进程分离。两者的 TCP 通道在接受命令前通过版本化挑战应答双向验证共享 token，不发送秘密正文。
//! 远程 attach 有意只提供认证、不提供传输加密；请使用可信网络或 SSH 隧道。

mod adapter;
mod authentication;
mod debuggee;
mod wire;

/// 调试器纯表达式分析；工作台与 DAP 共用同一份语义。
pub mod expression;

pub use adapter::run_stdio;
pub use debuggee::{DebuggeeOptions, run as run_debuggee};
