//! 缓冲区正则搜索、忽略规则遍历与候选项评分。
//!
//! 匹配位置使用行内 UTF-8 字节范围；结果进入编辑器前仍需转换为字符坐标。
//! 工作区搜索是同步的目录与文件读取，应由调用方放到阻塞工作线程。

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use thiserror::Error;

/// 缓冲区或工作区文件中的一次正则匹配。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    /// 包含匹配的文件；未命名缓冲区没有此项。
    pub path: Option<PathBuf>,

    /// 从一开始的行号。
    pub line: usize,

    /// 从零开始的 UTF-8 字节列。
    pub column: usize,

    /// 移除终止符后的匹配行。
    pub line_text: String,

    /// `line_text` 内的字节范围。
    pub range: std::ops::Range<usize>,
}

/// 无效正则表达式或文件系统读取失败。
#[derive(Debug, Error)]
pub enum SearchError {
    /// 正则表达式无效。
    #[error("Invalid regular expression: {0}")]
    Regex(#[from] regex::Error),

    /// 无法读取匹配候选项。
    #[error("Workspace search failed: {0}")]
    Io(#[from] std::io::Error),

    /// 请求被其所有者取消。
    #[error("Search canceled")]
    Cancelled,
}

/// Unicode 正则、忽略文件遍历与内置模糊评分。
#[derive(Clone, Debug, Default)]
pub struct SearchService;

impl SearchService {
    /// 搜索一个 UTF-8 缓冲区。
    pub fn buffer(
        text: &str,
        pattern: &str,
        case_sensitive: bool,
    ) -> Result<Vec<SearchMatch>, SearchError> {
        let regex = RegexBuilder::new(pattern)
            .unicode(true)
            .case_insensitive(!case_sensitive)
            .build()?;
        Ok(search_text(None, text, &regex, usize::MAX))
    }

    /// 遵循忽略规则同步遍历目录并搜索 UTF-8 文件。
    ///
    /// 即使根目录不在 Git 仓库内也应用忽略规则；不可读条目和非 UTF-8 文件会跳过。
    /// 调用方应传入正数结果上限，并将此同步工作移到阻塞线程；取消时返回错误而非部分结果。
    pub fn workspace(
        root: &Path,
        pattern: &str,
        case_sensitive: bool,
        limit: usize,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<Vec<SearchMatch>, SearchError> {
        let regex = RegexBuilder::new(pattern)
            .unicode(true)
            .case_insensitive(!case_sensitive)
            .build()?;
        let mut matches = Vec::new();
        for entry in WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(true)
            .require_git(false)
            .build()
        {
            if cancelled.load(Ordering::Relaxed) {
                return Err(SearchError::Cancelled);
            }
            let Ok(entry) = entry else {
                continue;
            };
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            let remaining = limit.saturating_sub(matches.len());
            matches.extend(search_text(Some(path), text, &regex, remaining));
            if matches.len() >= limit {
                break;
            }
        }
        Ok(matches)
    }

    /// 使用不区分大小写的子序列分数对候选项排序。
    #[must_use]
    pub fn fuzzy<'a>(
        query: &str,
        candidates: impl IntoIterator<Item = &'a str>,
        limit: usize,
    ) -> Vec<&'a str> {
        let mut scored = candidates
            .into_iter()
            .filter_map(|candidate| fuzzy_score(query, candidate).map(|score| (score, candidate)))
            .collect::<Vec<_>>();
        scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(right.1)));
        scored
            .into_iter()
            .take(limit)
            .map(|(_, candidate)| candidate)
            .collect()
    }
}

fn search_text(path: Option<&Path>, text: &str, regex: &Regex, limit: usize) -> Vec<SearchMatch> {
    let mut output = Vec::new();
    for (line_index, line) in text.lines().enumerate() {
        for found in regex.find_iter(line) {
            output.push(SearchMatch {
                path: path.map(Path::to_path_buf),
                line: line_index + 1,
                column: found.start(),
                line_text: line.to_owned(),
                range: found.range(),
            });
            if output.len() >= limit {
                return output;
            }
        }
    }
    output
}

fn fuzzy_score(query: &str, candidate: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let query = query.to_lowercase();
    let candidate_lower = candidate.to_lowercase();
    if let Some(position) = candidate_lower.find(&query) {
        let boundary = position == 0
            || candidate[..position]
                .chars()
                .next_back()
                .is_some_and(|character| !character.is_alphanumeric());
        return Some(
            100 + i64::from(boundary) * 25
                - saturating_i64(position)
                - saturating_i64(candidate.len()) / 8,
        );
    }
    let mut positions = Vec::new();
    let mut cursor = 0;
    for needle in query.chars() {
        let relative = candidate_lower[cursor..].find(needle)?;
        let position = cursor + relative;
        positions.push(position);
        cursor = position + needle.len_utf8();
    }
    let consecutive = positions
        .windows(2)
        .filter(|pair| pair[1] == pair[0] + 1)
        .count();
    let boundary = positions
        .iter()
        .filter(|position| {
            **position == 0
                || candidate[..**position]
                    .chars()
                    .next_back()
                    .is_some_and(|character| !character.is_alphanumeric())
        })
        .count();
    Some(
        saturating_i64(consecutive) * 20 + saturating_i64(boundary) * 15
            - saturating_i64(positions[0])
            - saturating_i64(candidate.len()) / 8,
    )
}

fn saturating_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, atomic::AtomicBool};

    use tempfile::tempdir;

    use super::SearchService;

    #[test]
    fn searches_unicode_and_ranks_fuzzy_boundaries() {
        let matches =
            SearchService::buffer("alpha\n\u{4e16}\u{754c} alpha", "\u{4e16}\u{754c}", true)
                .expect("Valid regular expression");
        assert_eq!(matches[0].line, 2);
        assert_eq!(matches[0].column, 0);
        let ranked = SearchService::fuzzy("rs", ["restore", "src/main.rs", "browser"], 3);
        assert_eq!(ranked[0], "src/main.rs");
    }

    #[test]
    fn workspace_search_honors_ignore_files() {
        let directory = tempdir().expect("temporary directory");
        std::fs::write(directory.path().join("visible.txt"), "needle").expect("visible test data");
        std::fs::write(directory.path().join("ignored.txt"), "needle").expect("ignored test data");
        std::fs::write(directory.path().join(".gitignore"), "ignored.txt\n")
            .expect("ignore-rule test data");
        let found = SearchService::workspace(
            directory.path(),
            "needle",
            true,
            20,
            &Arc::new(AtomicBool::new(false)),
        )
        .expect("workspace search");
        assert_eq!(found.len(), 1);
        assert!(
            found[0]
                .path
                .as_ref()
                .is_some_and(|path| path.ends_with("visible.txt"))
        );
    }
}
