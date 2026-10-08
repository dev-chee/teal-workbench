use std::path::PathBuf;

/// 受支持的串行化工作区修改。
#[derive(Clone)]
pub enum MutationKind {
    /// 替换完整文件内容。
    Write(String),

    /// 仅当目标目录项仍不存在时创建完整文本文件。
    WriteNew(String),

    /// 替换一次精确匹配。
    Replace {
        /// 必须完全匹配的原文。
        old: String,
        /// 替换后的新文本。
        new: String,
    },

    /// 对一个确定内容摘要应用多 hunk unified diff。
    ApplyPatch {
        /// 应用前必须匹配的修改前内容摘要。
        expected_sha256: String,
        /// 以 unified diff 表达的修改内容。
        patch: String,
    },

    /// 创建一个目录及其缺失父目录。
    CreateDirectory,

    /// 把源路径移动到工作区内目标路径。
    Move {
        /// 工作区内的目标路径。
        destination: PathBuf,
    },

    /// 把路径移动到平台回收站。
    Delete,
}
