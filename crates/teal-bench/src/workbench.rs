//! 工作台私有的终端呈现层。
//!
//! 此模块直接绑定 Ratatui 与 Crossterm，因此不作为未来 GUI 的复用边界；可复用的编辑、
//! 自动化、服务和 Agent runtime 能力保留在各自的 `teal-workbench-*` crate 中。

// 合入后保留原终端 UI 组件的完整内部契约；部分组件尚未由当前产品路由启用。
#[allow(dead_code)]
pub(crate) mod ui;
