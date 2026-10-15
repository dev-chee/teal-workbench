//! Teal 工作台中受能力约束的 Teal 自动化。
//!
//! 本库将 Teal 编译与类型化宿主函数接入配置、文法和自动化。
//! 脚本分类由嵌入应用提供；信任检查、完整导入闭包和能力集合必须使用同一次准备结果。

pub mod extension;
mod grammar_host;
mod runtime;
mod trust;

pub use grammar_host::{GrammarHost, builtin_grammar_scripts};
pub use runtime::{
    AgentSettings, AutomationCommand, McpSettings, PreparedScript, RuntimeError, RuntimeSnapshot,
    ScriptKind, WorkbenchAutomationRuntime,
};
pub use trust::{TrustDecision, TrustError, TrustStore};
mod knowledge_review;
pub use knowledge_review::{
    KnowledgePublicationError, KnowledgePublicationWriter, KnowledgeReviewError,
    WorkspacePublication, build_knowledge_review_packet, process_knowledge_proposal,
    publish_approved_knowledge, review_knowledge_proposal,
};
