//! 有界的结构化用户消息历史。
//!
//! 消息正文可能包含文件内容或工具输出，不能直接作为观测导出的安全字段。
//! 消息操作保存可执行的命令文本，是否执行仍由应用输入与授权路径决定。

use std::{
    collections::VecDeque,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// 结构化消息的重要程度。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    /// 信息状态。
    #[default]
    Info,

    /// 可恢复问题或降级行为。
    Warning,

    /// 失败的用户操作。
    Error,
}

/// 附加到消息、可由用户调用的操作。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageAction {
    /// 界面显示的短标签。
    pub label: String,

    /// 选中时执行的已注册 Teal 工作台命令。
    pub command: String,
}

/// 一条带来源的结构化消息。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// 自 Unix 纪元以来的毫秒数。
    pub timestamp_ms: u64,

    /// 用于颜色和筛选的重要程度。
    pub severity: Severity,

    /// `editor`、`lsp` 或 `agent` 等子系统。
    pub source: String,

    /// 可用时提供的稳定机器可读类别。
    pub code: Option<String>,

    /// 面向用户的短文本。
    pub text: String,

    /// 可选的换行详情文本。
    pub detail: Option<String>,

    /// 可选恢复或导航操作。
    pub actions: Vec<MessageAction>,
}

impl Message {
    /// 创建以当前墙上时钟时间戳记的消息。
    #[must_use]
    pub fn new(severity: Severity, source: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| {
                    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
                }),
            severity,
            source: source.into(),
            code: None,
            text: text.into(),
            detail: None,
            actions: Vec::new(),
        }
    }
}

/// 容量有界、按时间排序的消息历史。
#[derive(Clone, Debug)]
pub struct MessageLog {
    messages: VecDeque<Message>,
    capacity: usize,
}

impl Default for MessageLog {
    fn default() -> Self {
        Self::new(500)
    }
}

impl MessageLog {
    /// 创建最多保留 `capacity` 条消息的日志。
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            messages: VecDeque::with_capacity(capacity.min(4096)),
            capacity: capacity.max(1),
        }
    }

    /// 追加消息，并在已满时淘汰最旧条目。
    pub fn push(&mut self, message: Message) {
        while self.messages.len() >= self.capacity {
            self.messages.pop_front();
        }
        self.messages.push_back(message);
    }

    /// 从最旧到最新迭代消息。
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Message> {
        self.messages.iter()
    }

    /// 返回最新消息。
    #[must_use]
    pub fn latest(&self) -> Option<&Message> {
        self.messages.back()
    }
}
