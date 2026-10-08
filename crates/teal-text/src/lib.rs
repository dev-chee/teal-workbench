//! 协议工具共用的不可变文本快照与 UTF-8/UTF-16 坐标转换。
#![deny(missing_docs)]

use std::sync::Arc;

/// 从零开始、以 UTF-16 码元计数列宽的文本位置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Utf16Position {
    /// 从零开始的行号。
    pub line: u32,
    /// 从零开始的 UTF-16 码元列。
    pub character: u32,
}

/// 绑定一个不可变 UTF-8 快照的行坐标索引。
///
/// LF 与 CRLF 都是行分隔符，分隔符不计入任一行的 UTF-16 列宽。索引允许文本末尾
/// 位置；坐标落在代理项对内部、字节偏移落在 UTF-8 字符内部或数值溢出时返回 `None`。
#[derive(Clone, Debug)]
pub struct LineIndex {
    inner: Arc<IndexData>,
}

/// 文本与行界限一起共享，克隆索引不会复制整个文档。
#[derive(Debug)]
struct IndexData {
    text: Arc<str>,
    starts: Vec<usize>,
    content_ends: Vec<usize>,
}

impl LineIndex {
    /// 为不可变文本快照建立索引。
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self::from_shared(Arc::from(text.into()))
    }

    /// 为共享的不可变 UTF-8 文本建立索引；克隆索引同时共享文本与行表。
    ///
    /// 此构造函数不执行 I/O。所有合法 UTF-8 文本均可索引；查询越界时返回 `None`。
    ///
    /// ```
    /// let text: std::sync::Arc<str> = "one\ntwo".into();
    /// let index = teal_text::LineIndex::from_shared(text);
    /// assert_eq!(index.offset(1, 0), Some(4));
    /// ```
    #[must_use]
    pub fn from_shared(text: Arc<str>) -> Self {
        let mut starts = vec![0];
        let mut content_ends = Vec::new();
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                let end = if index > 0 && text.as_bytes()[index - 1] == b'\r' {
                    index - 1
                } else {
                    index
                };
                content_ends.push(end);
                starts.push(index + 1);
            }
        }
        content_ends.push(text.len());
        Self {
            inner: Arc::new(IndexData {
                text,
                starts,
                content_ends,
            }),
        }
    }

    /// 将 UTF-16 行列坐标转换为 UTF-8 字节偏移。
    #[must_use]
    pub fn offset(&self, line: u64, utf16_character: u64) -> Option<usize> {
        let line = usize::try_from(line).ok()?;
        let start = *self.inner.starts.get(line)?;
        let end = *self.inner.content_ends.get(line)?;
        let mut units = 0_u64;
        for (relative, character) in self.inner.text.get(start..end)?.char_indices() {
            if units == utf16_character {
                return Some(start + relative);
            }
            units += character.len_utf16() as u64;
            if units > utf16_character {
                return None;
            }
        }
        (units == utf16_character).then_some(end)
    }

    /// 将 UTF-8 字节偏移转换为 UTF-16 行列坐标。
    ///
    /// 换行序列中的偏移归到前一行末尾；下一行起始字节归到下一行第零列。
    #[must_use]
    pub fn position(&self, offset: usize) -> Option<Utf16Position> {
        if offset > self.inner.text.len() || !self.inner.text.is_char_boundary(offset) {
            return None;
        }
        let line = self
            .inner
            .starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1);
        let end = offset.min(self.inner.content_ends[line]);
        let units = self
            .inner
            .text
            .get(self.inner.starts[line]..end)?
            .encode_utf16()
            .count();
        Some(Utf16Position {
            line: u32::try_from(line).ok()?,
            character: u32::try_from(units).ok()?,
        })
    }

    /// 将 UTF-8 字节偏移转换为 Rope 风格的 Unicode 标量值偏移。
    #[must_use]
    pub fn char_offset(&self, offset: usize) -> Option<usize> {
        self.inner
            .text
            .get(..offset)
            .map(str::chars)
            .map(Iterator::count)
    }
}
