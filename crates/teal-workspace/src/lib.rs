//! 版本化工作区文件快照、修改提案和修改记录。
#![deny(missing_docs)]

mod files;
pub use files::{ChangeRecord, ChangeState, TextEncoding, TextSnapshot, Workspace, WorkspaceError};
