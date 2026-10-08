//! 从具体语法节点生成按范围和优先级排序的高亮捕获。
//!
//! 捕获保留字节范围，不直接绘制终端单元格；语义词元、诊断和嵌入语言可由上层叠加。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{GrammarSpec, SyntaxTree};

/// 覆盖一个 UTF-8 字节范围的样式捕获。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capture {
    /// `keyword`、`string` 或 `function` 等捕获名称。
    pub name: String,

    /// 覆盖的源码字节。
    pub range: std::ops::Range<usize>,

    /// 较大的值覆盖较早的基础捕获。
    pub priority: u16,
}

/// 从具体类型到高亮的确定性映射。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighlightQuery {
    captures: BTreeMap<String, (String, u16)>,
}

impl HighlightQuery {
    /// 从文法的基础捕获创建优先级为零的查询。
    #[must_use]
    pub fn from_grammar(grammar: &GrammarSpec) -> Self {
        Self {
            captures: grammar
                .captures
                .iter()
                .map(|(kind, capture)| (kind.clone(), (capture.clone(), 0)))
                .collect(),
        }
    }

    /// 插入或替换类型映射。
    pub fn insert(&mut self, kind: impl Into<String>, capture: impl Into<String>, priority: u16) {
        self.captures
            .insert(kind.into(), (capture.into(), priority));
    }

    /// 按源码顺序和稳定优先级顺序求值捕获。
    #[must_use]
    pub fn evaluate(&self, tree: &SyntaxTree) -> Vec<Capture> {
        let mut captures = tree
            .root()
            .walk()
            .filter_map(|node| {
                self.captures
                    .get(node.kind())
                    .map(|(name, priority)| Capture {
                        name: name.clone(),
                        range: node.range(),
                        priority: *priority,
                    })
            })
            .collect::<Vec<_>>();
        captures.sort_by_key(|capture| (capture.range.start, capture.range.end, capture.priority));
        captures
    }
}
