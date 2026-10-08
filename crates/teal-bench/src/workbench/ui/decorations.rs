//! 编辑器与会话面板共用的 gutter 与文本装饰模型。
//!
//! gutter 表示行首的窄列标记（行号、诊断、断点、VCS、工具状态），装饰表示行内的
//! 高亮区间（诊断、搜索命中、Agent 改动、选区）。两者都以零基行号索引，调用方
//! 负责把它们与文本坐标对齐。

use std::collections::BTreeMap;

/// 行首 gutter 标记类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GutterType {
    /// 诊断错误。
    Error,

    /// 诊断警告。
    Warning,

    /// 调试断点。
    Breakpoint,

    /// 版本控制改动。
    Vcs,

    /// 工具或运行状态。
    Tool,
}

/// 行内高亮类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DecorationKind {
    /// 语言诊断。
    Diagnostic,

    /// 搜索命中。
    Search,

    /// Agent 改动。
    Change,

    /// 用户选区。
    Selection,
}

/// 按行索引的装饰集合。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decorations {
    gutter: BTreeMap<usize, Vec<GutterType>>,
    ranges: BTreeMap<usize, Vec<(usize, usize, DecorationKind)>>,
}

impl Decorations {
    /// 在指定行的 gutter 上追加一个标记。
    pub fn push_gutter(&mut self, line: usize, marker: GutterType) {
        let entry = self.gutter.entry(line).or_default();
        if !entry.contains(&marker) {
            entry.push(marker);
            entry.sort_unstable();
        }
    }

    /// 在指定行上追加一段装饰区间；区间使用零基字符列，半开区间。
    pub fn push_range(&mut self, line: usize, start: usize, end: usize, kind: DecorationKind) {
        if end <= start {
            return;
        }
        self.ranges
            .entry(line)
            .or_default()
            .push((start, end, kind));
    }

    /// 返回指定行的 gutter 标记。
    #[must_use]
    pub fn gutter(&self, line: usize) -> &[GutterType] {
        self.gutter.get(&line).map_or(&[], Vec::as_slice)
    }

    /// 返回指定行的装饰区间。
    #[must_use]
    pub fn ranges(&self, line: usize) -> &[(usize, usize, DecorationKind)] {
        self.ranges.get(&line).map_or(&[], Vec::as_slice)
    }

    /// 清空全部装饰。
    pub fn clear(&mut self) {
        self.gutter.clear();
        self.ranges.clear();
    }

    /// 返回是否没有任何 gutter 标记与装饰区间。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.gutter.is_empty() && self.ranges.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{DecorationKind, Decorations, GutterType};

    #[test]
    fn gutter_markers_are_sorted_and_deduplicated_per_line() {
        let mut decorations = Decorations::default();
        decorations.push_gutter(4, GutterType::Warning);
        decorations.push_gutter(4, GutterType::Error);
        decorations.push_gutter(4, GutterType::Error);
        assert_eq!(
            decorations.gutter(4),
            &[GutterType::Error, GutterType::Warning]
        );
        assert!(decorations.gutter(0).is_empty());
    }

    #[test]
    fn ranges_reject_empty_spans_and_track_kinds() {
        let mut decorations = Decorations::default();
        decorations.push_range(2, 4, 4, DecorationKind::Search);
        decorations.push_range(2, 4, 9, DecorationKind::Search);
        decorations.push_range(2, 0, 3, DecorationKind::Change);
        assert_eq!(
            decorations.ranges(2),
            &[
                (4, 9, DecorationKind::Search),
                (0, 3, DecorationKind::Change),
            ]
        );
        assert!(!decorations.is_empty());
        decorations.clear();
        assert!(decorations.is_empty());
    }
}
