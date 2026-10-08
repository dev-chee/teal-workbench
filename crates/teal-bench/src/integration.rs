//! 工作台与外部执行能力的终端适配层。
//!
//! 该模块承载调试器和工具目录的终端集成。可复用的工作区、Agent runtime、
//! 自动化和服务实现位于 `teal-workbench-*` crate；这里仅保留终端事件桥接、
//! 审批交互和平台适配。

pub(crate) mod debug;
