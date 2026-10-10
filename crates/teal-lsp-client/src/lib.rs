//! Teal 的通用语言服务器协议客户端。
//!
//! 本库提供协议传输、文档同步、坐标转换与编辑解析。
//! 嵌入应用负责请求触发、过期结果检查、工作区授权，以及把编辑应用到共享缓冲区。

mod client;
mod diagnostics;
mod manager;
pub use diagnostics::{Diagnostic, DiagnosticStore};
pub use manager::{LspManager, ManagedServer, TextDocumentChange};
mod framing;
mod text;
mod workspace_edit;

pub use client::{JsonRpcId, LspClient, LspError, LspEvent, ServerConfig};
pub use framing::{LspReader, write_message};
pub use text::{LineIndex, SemanticToken, SemanticTokenCache};
pub use workspace_edit::{
    DocumentEdit, TextEdit, WorkspaceEditError, WorkspaceOperation, parse_workspace_edit,
};
