//! 与 `teal-knowledge` 构建脚本共享的知识生成器源码。
//!
//! 按 `teal-packages` 的既有范式用 `#[path]` 引入 `src/` 下的同一批文件，因此构建期生成
//! 与维护命令不会各自实现一遍知识快照与索引逻辑；这些名字必须在 crate 根可见，共享模块
//! 内部的 `crate::` 引用才能解析。

#[path = "../../../crates/teal-knowledge/src/error.rs"]
pub mod error;
#[path = "../../../crates/teal-knowledge/src/index.rs"]
pub mod index;
#[path = "../../../crates/teal-knowledge/src/snapshot.rs"]
pub mod snapshot;
#[path = "../../../crates/teal-knowledge/src/document.rs"]
pub mod document;
#[path = "../../../crates/teal-knowledge/src/generator.rs"]
pub mod generator;

pub use error::KnowledgeError;
pub use index::{
    KNOWLEDGE_INDEX_FORMAT_VERSION, KnowledgeEntry, KnowledgeEntryIndex, KnowledgeHit,
    KnowledgeIndex,
};
pub use snapshot::{
    KNOWLEDGE_FORMAT_VERSION, KnowledgeArtifact, KnowledgeKind, KnowledgeRecord, KnowledgeScope,
    KnowledgeSnapshot, KnowledgeSourceFile, compute_content_digest,
};
