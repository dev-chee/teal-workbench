//! 不可变 UTF-8 快照的 LSP 坐标索引与语义词元缓存。
//!
//! 协议列和长度以 UTF-16 码元计数，编辑器以 Unicode 字符计数，语法捕获以 UTF-8 字节计数。
//! 转换必须基于请求所对应的同一文本快照。

use serde::{Deserialize, Serialize};

/// 支持 LSP UTF-16 行坐标转换的 UTF-8 快照。
#[derive(Clone, Debug)]
pub struct LineIndex {
    inner: teal_utils::text::LineIndex,
}

impl LineIndex {
    /// 为一个不可变快照索引各行起点。
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            inner: teal_utils::text::LineIndex::new(text),
        }
    }

    /// 将从零开始的 LSP UTF-16 坐标转换为 UTF-8 字节偏移。
    ///
    /// 索引绑定构造时的不可变文本。行号不存在、列超出索引范围或位置落在代理项对内部时返回 `None`，
    /// 不会截断到邻近字符；返回值进入 Rope 编辑前还须转换为字符偏移。
    #[must_use]
    pub fn offset(&self, line: u64, utf16_character: u64) -> Option<usize> {
        self.inner.offset(line, utf16_character)
    }

    /// 将 UTF-8 字节偏移转换为从零开始的 `(行, UTF-16 码元列)`。
    ///
    /// 允许文本末尾的边界；超出文本、落在多字节字符内部或结果不能由 `u32` 表示时返回 `None`。
    #[must_use]
    pub fn position(&self, offset: usize) -> Option<(u32, u32)> {
        self.inner
            .position(offset)
            .map(|position| (position.line, position.character))
    }

    /// 将一个 UTF-8 字节偏移转换为 Rope 风格字符偏移。
    #[must_use]
    pub fn char_offset(&self, offset: usize) -> Option<usize> {
        self.inner.char_offset(offset)
    }
}

/// 已解码 LSP 语义词元。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticToken {
    /// 从零开始的行。
    pub line: u32,

    /// 从零开始的 UTF-16 起始列。
    pub start: u32,

    /// UTF-16 长度。
    pub length: u32,

    /// 服务器图例词元类型索引。
    pub token_type: u32,

    /// 服务器图例修饰符位集。
    pub modifiers: u32,
}

/// 完整/增量语义词元缓存。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SemanticTokenCache {
    data: Vec<u32>,
    result_id: Option<String>,
}

impl SemanticTokenCache {
    /// 替换完整编码词元流。
    pub fn replace(&mut self, data: Vec<u32>, result_id: Option<String>) {
        self.data = data;
        self.result_id = result_id;
    }

    /// 按源码逆序应用 LSP 语义词元增量编辑。
    pub fn apply_delta(&mut self, edits: &serde_json::Value, result_id: Option<String>) -> bool {
        let Some(edits) = edits.as_array() else {
            return false;
        };
        let mut parsed = Vec::new();
        let mut previous: Option<(usize, usize)> = None;
        for edit in edits {
            let Some(start) = edit
                .get("start")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
            else {
                return false;
            };
            let Some(delete_count) = edit
                .get("deleteCount")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
            else {
                return false;
            };
            let data = match edit.get("data") {
                None => Vec::new(),
                Some(value) => {
                    let Some(values) = value.as_array() else {
                        return false;
                    };
                    let Some(data) = values
                        .iter()
                        .map(|value| value.as_u64().and_then(|value| u32::try_from(value).ok()))
                        .collect::<Option<Vec<_>>>()
                    else {
                        return false;
                    };
                    data
                }
            };
            let Some(end) = start.checked_add(delete_count) else {
                return false;
            };
            if end > self.data.len()
                || previous.is_some_and(|(prior_start, prior_end)| {
                    start <= prior_start || start < prior_end
                })
            {
                return false;
            }
            previous = Some((start, end));
            parsed.push((start, delete_count, data));
        }
        let mut next = self.data.clone();
        for (start, delete_count, data) in parsed.into_iter().rev() {
            next.splice(start..start + delete_count, data);
        }
        if !next.len().is_multiple_of(5) {
            return false;
        }
        self.data = next;
        self.result_id = result_id;
        true
    }

    /// 解码当前相对五整数流。
    #[must_use]
    pub fn tokens(&self) -> Vec<SemanticToken> {
        let mut line = 0_u32;
        let mut start = 0_u32;
        self.data
            .as_chunks::<5>()
            .0
            .iter()
            .map(|chunk| {
                if chunk[0] == 0 {
                    start = start.saturating_add(chunk[1]);
                } else {
                    line = line.saturating_add(chunk[0]);
                    start = chunk[1];
                }
                SemanticToken {
                    line,
                    start,
                    length: chunk[2],
                    token_type: chunk[3],
                    modifiers: chunk[4],
                }
            })
            .collect()
    }

    /// 返回增量请求使用的服务器结果 ID。
    #[must_use]
    pub fn result_id(&self) -> Option<&str> {
        self.result_id.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{LineIndex, SemanticTokenCache};

    #[test]
    fn converts_surrogate_pairs_and_applies_semantic_deltas() {
        let index = LineIndex::new("a🙂b\n\u{4e16}\u{754c}");
        assert_eq!(index.offset(0, 3), Some("a🙂".len()));
        assert_eq!(index.position("a🙂".len()), Some((0, 3)));

        let mut cache = SemanticTokenCache::default();
        cache.replace(vec![0, 0, 1, 0, 0, 0, 2, 1, 1, 0], Some("one".to_owned()));
        assert!(cache.apply_delta(
            &json!([{"start":5,"deleteCount":5,"data":[1,0,2,2,0]}]),
            Some("two".to_owned())
        ));
        let tokens = cache.tokens();
        assert_eq!(tokens[1].line, 1);
        assert_eq!(tokens[1].length, 2);
    }

    #[test]
    fn invalid_semantic_deltas_leave_the_cache_unchanged() {
        let mut original = SemanticTokenCache::default();
        original.replace(vec![0, 0, 1, 0, 0, 0, 2, 1, 1, 0], Some("original".into()));
        for edits in [
            json!([
                {"start":5,"deleteCount":0,"data":[]},
                {"start":0,"deleteCount":0,"data":[]}
            ]),
            json!([
                {"start":0,"deleteCount":5,"data":[]},
                {"start":3,"deleteCount":5,"data":[]}
            ]),
            json!([{"start":0,"deleteCount":0,"data":[4294967296_u64]}]),
            json!([{"start":0,"deleteCount":0,"data":[0,0,1,0]}]),
        ] {
            let mut cache = original.clone();
            assert!(!cache.apply_delta(&edits, Some("changed".into())));
            assert_eq!(cache, original);
        }
    }
}
