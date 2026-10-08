//! 以 Unicode 字符偏移表示的主选择与多选择集合。
//!
//! 锚点与活动端保留方向，读取范围时才取最小和最大端点；字节、UTF-16 和屏幕列不能直接填入。

use std::ops::Range;

use serde::{Deserialize, Serialize};

/// UTF-8 Rope 中的一个有方向字符选择区。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    /// 固定选择端点。
    pub anchor: usize,

    /// 移动选择端点和插入光标。
    pub head: usize,
}

impl Selection {
    /// 创建折叠光标。
    #[must_use]
    pub const fn cursor(position: usize) -> Self {
        Self {
            anchor: position,
            head: position,
        }
    }

    /// 返回有序的半开字符范围。
    #[must_use]
    pub fn range(self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    /// 返回此选择区是否为光标而非范围。
    #[must_use]
    pub const fn is_cursor(self) -> bool {
        self.anchor == self.head
    }
}

/// 包含一个主成员的规范化非重叠选择区。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionSet {
    selections: Vec<Selection>,
    primary: usize,
}

impl Default for SelectionSet {
    fn default() -> Self {
        Self::single(Selection::cursor(0))
    }
}

impl SelectionSet {
    /// 创建包含一个主选择区的集合。
    #[must_use]
    pub fn single(selection: Selection) -> Self {
        Self {
            selections: vec![selection],
            primary: 0,
        }
    }

    /// 创建并规范化可能重叠的集合。
    #[must_use]
    pub fn new(selections: Vec<Selection>, primary: usize) -> Self {
        if selections.is_empty() {
            return Self::default();
        }
        let primary_head = selections[primary.min(selections.len() - 1)].head;
        let mut ranges = selections
            .into_iter()
            .map(Selection::range)
            .collect::<Vec<_>>();
        ranges.sort_by_key(|range| (range.start, range.end));
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
        for range in ranges {
            if let Some(previous) = merged.last_mut()
                && range.start <= previous.end
            {
                previous.end = previous.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        let selections = merged
            .into_iter()
            .map(|range| Selection {
                anchor: range.start,
                head: range.end,
            })
            .collect::<Vec<_>>();
        let primary = selections
            .iter()
            .position(|selection| {
                selection.range().contains(&primary_head) || selection.head == primary_head
            })
            .unwrap_or(0);
        Self {
            selections,
            primary,
        }
    }

    /// 按源码顺序返回所有规范化选择区。
    #[must_use]
    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }

    /// 返回主选择区。
    #[must_use]
    pub fn primary(&self) -> Selection {
        self.selections[self.primary]
    }

    /// 替换所有选择区并规范化重叠部分。
    pub fn replace(&mut self, selections: Vec<Selection>, primary: usize) {
        *self = Self::new(selections, primary);
    }

    /// 将每个端点限制在包含文档长度的范围内。
    pub fn clamp(&mut self, len_chars: usize) {
        for selection in &mut self.selections {
            selection.anchor = selection.anchor.min(len_chars);
            selection.head = selection.head.min(len_chars);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Selection, SelectionSet};

    #[test]
    fn merges_overlaps_and_keeps_the_primary_region() {
        let set = SelectionSet::new(
            vec![
                Selection { anchor: 8, head: 3 },
                Selection {
                    anchor: 6,
                    head: 12,
                },
                Selection::cursor(20),
            ],
            1,
        );
        assert_eq!(set.selections().len(), 2);
        assert_eq!(set.selections()[0].range(), 3..12);
        assert_eq!(set.primary().range(), 3..12);
    }
}
