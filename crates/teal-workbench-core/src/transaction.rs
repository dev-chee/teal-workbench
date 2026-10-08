//! 同一旧快照上的文本事务与分支撤销历史。
//!
//! 替换范围使用半开字符区间，插入内容使用 UTF-8 字符串。事务先校验边界再逆序应用，
//! 逆事务改用编辑后的坐标；撤销树记录事务关系，不自行访问文档或磁盘。

use std::ops::Range;

use ropey::Rope;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 以 Rope 字符偏移表示的一次替换。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    /// 从原始快照删除的半开字符范围。
    pub range: Range<usize>,

    /// 在 `range.start` 插入的 UTF-8 文本。
    pub insert: String,
}

/// 原子应用到一个文档修订的有序非重叠编辑。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    edits: Vec<TextEdit>,
}

/// 无效的事务形状或文档边界。
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum TransactionError {
    /// 缓冲区暂时由串行文件操作保留。
    #[error("Buffer is busy with a file-system operation")]
    BufferBusy,

    /// 两个编辑引用了重叠的原始范围。
    #[error("Transaction edit overlaps at character {0}")]
    Overlap(usize),

    /// 编辑范围反向或超出当前文档。
    #[error("Transaction range {start}..{end} exceeds document length {len}")]
    OutOfBounds {
        /// 无效范围起点。
        start: usize,

        /// 无效范围终点。
        end: usize,

        /// 当前文档字符长度。
        len: usize,
    },
}

impl Transaction {
    pub(crate) fn normalized(self) -> Result<Self, TransactionError> {
        Self::new(
            self.edits
                .into_iter()
                .map(|mut edit| {
                    edit.insert = edit.insert.replace("\r\n", "\n").replace('\r', "\n");
                    edit
                })
                .collect(),
        )
    }

    /// 按旧快照的起点和终点排序并拒绝重叠编辑。
    ///
    /// 所有范围必须使用同一个旧快照的字符坐标；此时没有文档长度，反向或越界范围在应用时检查。
    /// 同位置插入的相对次序由稳定排序保留。直接反序列化事务时，调用方仍须维护这些结构约束。
    pub fn new(mut edits: Vec<TextEdit>) -> Result<Self, TransactionError> {
        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
        for pair in edits.windows(2) {
            if pair[1].range.start < pair[0].range.end {
                return Err(TransactionError::Overlap(pair[1].range.start));
            }
        }
        Ok(Self { edits })
    }

    /// 创建一个插入事务。
    #[must_use]
    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Self {
            edits: vec![TextEdit {
                range: at..at,
                insert: text.into(),
            }],
        }
    }

    /// 创建一个删除事务。
    #[must_use]
    pub fn delete(range: Range<usize>) -> Self {
        Self {
            edits: vec![TextEdit {
                range,
                insert: String::new(),
            }],
        }
    }

    /// 返回有序编辑。
    #[must_use]
    pub fn edits(&self) -> &[TextEdit] {
        &self.edits
    }

    /// 返回此事务是否不产生任何变化。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
            || self
                .edits
                .iter()
                .all(|edit| edit.range.is_empty() && edit.insert.is_empty())
    }

    /// 应用此事务并返回使用编辑后坐标的逆事务。
    ///
    /// 要求事务已经满足有序、互不重叠的结构约定。先检查所有范围并收集删除内容，再从后往前修改 Rope，
    /// 因此有效事务的越界错误不会先写入一部分文本。本函数不更新文档修订、选择区或撤销树。
    ///
    /// ```rust
    /// use teal_workbench_core::{TextEdit, Transaction};
    /// use ropey::Rope;
    ///
    /// let mut text = Rope::from_str("a界🙂");
    /// let edit = Transaction::new(vec![TextEdit { range: 1..2, insert: "文".into() }])?;
    /// let inverse = edit.apply(&mut text)?;
    /// assert_eq!(text.to_string(), "a文🙂");
    /// inverse.apply(&mut text)?;
    /// assert_eq!(text.to_string(), "a界🙂");
    /// Ok::<(), teal_workbench_core::TransactionError>(())
    /// ```
    pub fn apply(&self, rope: &mut Rope) -> Result<Self, TransactionError> {
        let len = rope.len_chars();
        let mut inverse = Vec::with_capacity(self.edits.len());
        let mut delta: isize = 0;

        // 修改任何偏移前，先对旧 Rope 验证并收集删除的文本。
        for edit in &self.edits {
            if edit.range.start > edit.range.end || edit.range.end > len {
                return Err(TransactionError::OutOfBounds {
                    start: edit.range.start,
                    end: edit.range.end,
                    len,
                });
            }
            let removed = rope.slice(edit.range.clone()).to_string();
            let start = edit.range.start.saturating_add_signed(delta);
            let inserted_len = edit.insert.chars().count();
            inverse.push(TextEdit {
                range: start..start + inserted_len,
                insert: removed,
            });
            delta += inserted_len.cast_signed() - (edit.range.end - edit.range.start).cast_signed();
        }

        // 逆序应用使剩余编辑范围保持原始坐标。
        for edit in self.edits.iter().rev() {
            rope.remove(edit.range.clone());
            rope.insert(edit.range.start, &edit.insert);
        }

        Self::new(inverse)
    }
}

#[derive(Clone, Debug)]
struct HistoryNode {
    parent: Option<usize>,
    children: Vec<usize>,
    forward: Transaction,
    inverse: Transaction,
}

/// 保留分支的撤销树。
#[derive(Clone, Debug)]
pub struct UndoTree {
    nodes: Vec<HistoryNode>,
    current: usize,
}

impl Default for UndoTree {
    fn default() -> Self {
        Self {
            nodes: vec![HistoryNode {
                parent: None,
                children: Vec::new(),
                forward: Transaction::default(),
                inverse: Transaction::default(),
            }],
            current: 0,
        }
    }
}

impl UndoTree {
    /// 将一个已应用事务记录为当前节点的新分支。
    ///
    /// 调用方提供相互对应的正向和逆向事务，并负责实际修改文本。撤销后再记录编辑会增加兄弟分支，
    /// 不会清空原来的重做分支；分支索引因此是局部的子节点序号。
    pub fn record(&mut self, forward: Transaction, inverse: Transaction) {
        let id = self.nodes.len();
        self.nodes.push(HistoryNode {
            parent: Some(self.current),
            children: Vec::new(),
            forward,
            inverse,
        });
        self.nodes[self.current].children.push(id);
        self.current = id;
    }

    /// 返回逆事务并移动到父节点。
    pub fn undo(&mut self) -> Option<Transaction> {
        let node = &self.nodes[self.current];
        let parent = node.parent?;
        let inverse = node.inverse.clone();
        self.current = parent;
        Some(inverse)
    }

    /// 返回一个子事务并进入该分支。
    pub fn redo(&mut self, branch: usize) -> Option<Transaction> {
        let child = *self.nodes[self.current].children.get(branch)?;
        self.current = child;
        Some(self.nodes[child].forward.clone())
    }

    /// 返回当前稳定历史节点 ID。
    #[must_use]
    pub const fn current(&self) -> usize {
        self.current
    }

    /// 返回当前节点的重做分支数。
    #[must_use]
    pub fn redo_branches(&self) -> usize {
        self.nodes[self.current].children.len()
    }
}

#[cfg(test)]
mod tests {
    use ropey::Rope;

    use super::{TextEdit, Transaction, UndoTree};

    #[test]
    fn applies_unicode_edits_and_round_trips_the_inverse() {
        let mut rope = Rope::from_str("a\u{754c}cdef");
        let change = Transaction::new(vec![
            TextEdit {
                range: 1..3,
                insert: "🙂".to_owned(),
            },
            TextEdit {
                range: 6..6,
                insert: "!".to_owned(),
            },
        ])
        .expect("Non-overlapping editing");
        let inverse = change.apply(&mut rope).expect("valid transaction");
        assert_eq!(rope.to_string(), "a🙂def!");
        inverse
            .apply(&mut rope)
            .expect("Should be able to apply reverse transactions");
        assert_eq!(rope.to_string(), "a\u{754c}cdef");
    }

    #[test]
    fn undo_after_undo_creates_a_preserved_branch() {
        let mut history = UndoTree::default();
        history.record(Transaction::insert(0, "a"), Transaction::delete(0..1));
        assert!(history.undo().is_some());
        history.record(Transaction::insert(0, "b"), Transaction::delete(0..1));
        assert!(history.undo().is_some());
        assert_eq!(history.redo_branches(), 2);
    }
}
