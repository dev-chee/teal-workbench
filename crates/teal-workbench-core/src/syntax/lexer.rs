//! 最长匹配词法分析及旧词元前缀复用。
//!
//! 词元范围使用源码 UTF-8 字节偏移；正则匹配必须从当前扫描位置开始，
//! 无法匹配的输入仍须推进并产生错误词元，避免停留在同一位置。

use std::ops::Range;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::{GrammarError, GrammarSpec, TokenSpec};

/// 增量词法分析和树标识映射使用的面向字节编辑。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputEdit {
    /// 旧 UTF-8 源码中被替换的字节范围。
    pub old_range: Range<usize>,

    /// 插入的 UTF-8 文本。
    pub inserted: String,
}

/// 带源码字节和词元后模式栈的一个非跳过词元。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LexedToken {
    /// 终结符或合成的 `ERROR` 类型。
    pub kind: String,

    /// 半开 UTF-8 字节范围。
    pub range: Range<usize>,

    /// 此词元及中间跳过词元之后的词法模式。
    pub mode_after: Vec<String>,
}

struct CompiledToken {
    spec: TokenSpec,
    regex: Regex,
}

/// 支持命名模式的最长匹配正则词法分析器。
pub struct Lexer {
    tokens: Vec<CompiledToken>,
}

impl Lexer {
    /// 编译一个已验证文法的词元正则。
    pub fn new(grammar: &GrammarSpec) -> Result<Self, GrammarError> {
        grammar.validate()?;
        let tokens = grammar
            .tokens
            .iter()
            .cloned()
            .map(|spec| {
                let regex = Regex::new(&format!(r"\A(?:{})", spec.pattern))
                    .map_err(|error| GrammarError::Regex(spec.name.clone(), error.to_string()))?;
                Ok(CompiledToken { spec, regex })
            })
            .collect::<Result<Vec<_>, GrammarError>>()?;
        Ok(Self { tokens })
    }

    /// 从主模式对完整源码进行词法分析。
    #[must_use]
    pub fn lex(&self, source: &str) -> Vec<LexedToken> {
        self.lex_from(source, 0, vec!["main".to_owned()]).0
    }

    fn lex_from(
        &self,
        source: &str,
        offset: usize,
        mut modes: Vec<String>,
    ) -> (Vec<LexedToken>, Vec<String>) {
        let mut position = 0;
        let mut output = Vec::new();
        while position < source.len() {
            let active = modes.last().map_or("main", String::as_str);
            let rest = &source[position..];
            let matched = self
                .tokens
                .iter()
                .filter(|token| token.spec.mode == active)
                .filter_map(|token| {
                    token
                        .regex
                        .find(rest)
                        .filter(|found| found.start() == 0 && found.end() > 0)
                        .map(|found| (found.end(), token.spec.priority, token))
                })
                .max_by_key(|(length, priority, _)| (*length, *priority));
            let Some((length, _, token)) = matched else {
                let length = rest.chars().next().map_or(1, char::len_utf8);
                output.push(LexedToken {
                    kind: "ERROR".to_owned(),
                    range: offset + position..offset + position + length,
                    mode_after: modes.clone(),
                });
                position += length;
                continue;
            };
            if token.spec.pop_mode && modes.len() > 1 {
                modes.pop();
            }
            if let Some(mode) = &token.spec.push_mode {
                modes.push(mode.clone());
            }
            if !token.spec.skip {
                output.push(LexedToken {
                    kind: token.spec.name.clone(),
                    range: offset + position..offset + position + length,
                    mode_after: modes.clone(),
                });
            }
            position += length;
        }
        (output, modes)
    }
}

/// 支持仅后缀重新词法分析的源码快照与词元。
#[derive(Clone, Debug)]
pub struct TokenCache {
    source: String,
    tokens: Vec<LexedToken>,
}

impl TokenCache {
    /// 对初始源码快照进行词法分析。
    #[must_use]
    pub fn new(source: impl Into<String>, lexer: &Lexer) -> Self {
        let source = source.into();
        let tokens = lexer.lex(&source);
        Self { source, tokens }
    }

    /// 返回当前源码。
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// 返回当前非跳过词元。
    #[must_use]
    pub fn tokens(&self) -> &[LexedToken] {
        &self.tokens
    }

    /// 应用一次编辑，并从前一个最近的词元边界重新词法分析。前缀词元保留其
    /// 分配与范围；由于编辑可能改变任意长的词法模式，后缀会被重建。
    pub fn apply(&mut self, edit: &InputEdit, lexer: &Lexer) -> Range<usize> {
        assert!(
            edit.old_range.start <= edit.old_range.end && edit.old_range.end <= self.source.len()
        );
        assert!(
            self.source.is_char_boundary(edit.old_range.start)
                && self.source.is_char_boundary(edit.old_range.end)
        );
        let start_token = self
            .tokens
            .partition_point(|token| token.range.end <= edit.old_range.start);
        let relex_start = self
            .tokens
            .get(start_token)
            .map_or(edit.old_range.start, |token| {
                token.range.start.min(edit.old_range.start)
            });
        let modes = start_token
            .checked_sub(1)
            .and_then(|index| self.tokens.get(index))
            .map_or_else(|| vec!["main".to_owned()], |token| token.mode_after.clone());
        self.source
            .replace_range(edit.old_range.clone(), &edit.inserted);
        self.tokens.truncate(start_token);
        let (mut suffix, _) = lexer.lex_from(&self.source[relex_start..], relex_start, modes);
        self.tokens.append(&mut suffix);
        relex_start..self.source.len()
    }
}

#[cfg(test)]
mod tests {
    use super::super::GrammarBuilder;
    use super::{InputEdit, Lexer, TokenCache};

    fn lexer() -> Lexer {
        let grammar = GrammarBuilder::new("test")
            .token("word", "[a-z]+", 0)
            .skip("space", "\\s+", 0)
            .production("root", ["word"], "root")
            .start("root")
            .finish()
            .expect("grammar should be valid");
        Lexer::new(&grammar).expect("lexer should be valid")
    }

    #[test]
    fn longest_match_errors_and_incremental_prefix_reuse_are_deterministic() {
        let lexer = lexer();
        let mut cache = TokenCache::new("one two", &lexer);
        let first = cache.tokens()[0].clone();
        let dirty = cache.apply(
            &InputEdit {
                old_range: 4..7,
                inserted: "three".to_owned(),
            },
            &lexer,
        );
        assert_eq!(cache.tokens()[0], first);
        assert_eq!(&cache.source()[cache.tokens()[1].range.clone()], "three");
        assert!(dirty.start >= 3);
    }
}
