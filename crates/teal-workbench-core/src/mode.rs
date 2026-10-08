//! 编辑模式与键位预设的稳定数据类型。
//!
//! 模式决定输入如何解释，键位预设决定冲突命令的优先级；两者共用同一套选择和事务模型。

use serde::{Deserialize, Serialize};

/// 两个默认键位预设共享的当前模态输入状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EditorMode {
    /// 移动、操作符和选择命令。
    #[default]
    Normal,

    /// 直接插入文本。
    Insert,

    /// Vim 风格字符选择。
    Visual,

    /// Helix 风格选择优先操作。
    Select,

    /// 等待移动或文本对象的待处理 Vim 操作符。
    OperatorPending,

    /// 不引起插入位移的字符替换。
    Replace,

    /// 全局命令行输入。
    Command,

    /// 向前搜索缓冲区。
    SearchForward,

    /// 向后搜索缓冲区。
    SearchBackward,

    /// 地址或确认等面板专用提示。
    Prompt,
}

/// 基于同一选择与事务模型提供的键位风格。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeymapPreset {
    /// Vim 移动与操作符优先于选择优先绑定。
    #[default]
    VimFirst,

    /// Helix 选择优先绑定在冲突时优先于 Vim。
    HelixFirst,
}
