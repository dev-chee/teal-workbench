//! 终端文本行的显示坐标、命中与选择提取。
//!
//! Tab、宽字符和组合字素按终端显示宽度处理，不能把屏幕列直接当作字符串字节下标。

use std::ops::Range;

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// 显示行集合中的逻辑字符位置。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPoint {
    /// 从零开始的逻辑行。
    pub line: usize,

    /// 行内从零开始的 Unicode 字符偏移。
    pub column: usize,
}

/// 逻辑显示行上的有方向选择区。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSelection {
    /// 固定选择端点。
    pub anchor: TextPoint,

    /// 移动端点。
    pub head: TextPoint,
}

impl TextSelection {
    /// 返回有序的选择端点。
    #[must_use]
    pub fn ordered(self) -> (TextPoint, TextPoint) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// 返回两个端点是否标识同一字符边界。
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.anchor.line == self.head.line && self.anchor.column == self.head.column
    }
}

/// 返回使用四列制表位时文本的终端单元格宽度。
#[must_use]
pub fn display_width(text: &str) -> usize {
    display_width_from(text, 0)
}

/// 返回一个字符边界占据的终端单元格。
#[must_use]
pub fn char_to_cell(text: &str, character: usize) -> usize {
    let mut cell = 0;
    let mut chars = 0;
    for grapheme in text.graphemes(true) {
        let grapheme_chars = grapheme.chars().count();
        if chars >= character || chars + grapheme_chars > character {
            break;
        }
        cell += grapheme_width(grapheme, cell);
        chars += grapheme_chars;
    }
    cell
}

/// 将终端单元格列映射到最近的 Unicode 字符边界。
#[must_use]
pub fn cell_to_char(text: &str, target: usize) -> usize {
    let mut cell = 0;
    let mut chars = 0;
    for grapheme in text.graphemes(true) {
        let width = grapheme_width(grapheme, cell).max(1);
        if target < cell + width {
            return if target.saturating_sub(cell) * 2 >= width {
                chars + grapheme.chars().count()
            } else {
                chars
            };
        }
        cell += width;
        chars += grapheme.chars().count();
    }
    chars
}

/// 将制表符展开为空格，同时保留请求的终端起始列。
#[must_use]
pub fn expand_tabs(text: &str, starting_cell: usize) -> String {
    let mut cell = starting_cell;
    let mut output = String::new();
    for grapheme in text.graphemes(true) {
        if grapheme == "\t" {
            let width = 4 - (cell % 4);
            output.extend(std::iter::repeat_n(' ', width));
            cell += width;
        } else {
            output.push_str(grapheme);
            cell += UnicodeWidthStr::width(grapheme);
        }
    }
    output
}

/// 返回 Unicode 单词选择区或连续空白/标点范围。
#[must_use]
pub fn word_range(text: &str, character: usize) -> Range<usize> {
    let character = character.min(text.chars().count());
    for (byte, word) in text.unicode_word_indices() {
        let start = text[..byte].chars().count();
        let end = start + word.chars().count();
        if character >= start && character < end {
            return start..end;
        }
    }
    let chars = text.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return 0..0;
    }
    let index = character.min(chars.len() - 1);
    let class = |character: char| {
        if character.is_whitespace() {
            0
        } else if character.is_alphanumeric() || character == '_' {
            1
        } else {
            2
        }
    };
    let target_class = class(chars[index]);
    let mut start = index;
    while start > 0 && class(chars[start - 1]) == target_class {
        start -= 1;
    }
    let mut end = index + 1;
    while end < chars.len() && class(chars[end]) == target_class {
        end += 1;
    }
    start..end
}

/// 从纯逻辑行提取选择内容，跨行之间使用 `\n`。
#[must_use]
pub fn selected_text(lines: &[String], selection: TextSelection) -> Option<String> {
    if selection.is_empty() || lines.is_empty() {
        return None;
    }
    let (start, end) = selection.ordered();
    if start.line >= lines.len() {
        return None;
    }
    let end_line = end.line.min(lines.len() - 1);
    let mut output = String::new();
    for (line_index, line) in lines.iter().enumerate().take(end_line + 1).skip(start.line) {
        if line_index > start.line {
            output.push('\n');
        }
        let line_len = line.chars().count();
        let from = if line_index == start.line {
            start.column.min(line_len)
        } else {
            0
        };
        let to = if line_index == end_line {
            end.column.min(line_len)
        } else {
            line_len
        };
        output.extend(line.chars().skip(from).take(to.saturating_sub(from)));
    }
    Some(output)
}

/// 为带样式行应用选择背景，并在不丢失跨度样式的情况下展开制表符。
#[must_use]
pub fn render_lines_with_selection(
    lines: &[Line<'static>],
    selection: Option<TextSelection>,
) -> Vec<Line<'static>> {
    expand_line_tabs(&style_lines_with_selection(lines, selection, 0))
}

/// 从 `line_offset` 解释行索引并应用一个选择背景。
#[must_use]
pub fn style_lines_with_selection(
    lines: &[Line<'static>],
    selection: Option<TextSelection>,
    line_offset: usize,
) -> Vec<Line<'static>> {
    lines
        .iter()
        .enumerate()
        .map(|(line_index, line)| {
            let range = selection.and_then(|selection| {
                selection_range_for_line(selection, line_offset.saturating_add(line_index))
            });
            let mut spans = Vec::new();
            let mut character_offset = 0;
            for span in &line.spans {
                let content = span.content.as_ref();
                let span_len = content.chars().count();
                let mut boundaries = vec![0, span_len];
                if let Some(selected) = &range {
                    boundaries.push(
                        selected
                            .start
                            .saturating_sub(character_offset)
                            .min(span_len),
                    );
                    boundaries.push(selected.end.saturating_sub(character_offset).min(span_len));
                }
                boundaries.sort_unstable();
                boundaries.dedup();
                for pair in boundaries.windows(2) {
                    if pair[0] == pair[1] {
                        continue;
                    }
                    let piece = content
                        .chars()
                        .skip(pair[0])
                        .take(pair[1] - pair[0])
                        .collect::<String>();
                    let absolute = character_offset + pair[0]..character_offset + pair[1];
                    let style = if range.as_ref().is_some_and(|selected| {
                        selected.start < absolute.end && selected.end > absolute.start
                    }) {
                        span.style
                            .patch(Style::default().fg(Color::White).bg(Color::Blue))
                    } else {
                        span.style
                    };
                    spans.push(Span::styled(piece, style));
                }
                character_offset += span_len;
            }
            Line::from(spans).style(line.style)
        })
        .collect()
}

/// 在保留每个跨度样式的情况下展开带样式行中的制表符。
#[must_use]
pub fn expand_line_tabs(lines: &[Line<'static>]) -> Vec<Line<'static>> {
    lines
        .iter()
        .map(|line| {
            let mut cell = 0;
            let spans = line
                .spans
                .iter()
                .map(|span| {
                    let text = span.content.as_ref();
                    let expanded = expand_tabs(text, cell);
                    cell += display_width_from(text, cell);
                    Span::styled(expanded, span.style)
                })
                .collect::<Vec<_>>();
            Line::from(spans).style(line.style)
        })
        .collect()
}

fn selection_range_for_line(selection: TextSelection, line: usize) -> Option<Range<usize>> {
    let (start, end) = selection.ordered();
    if line < start.line || line > end.line {
        return None;
    }
    let from = if line == start.line { start.column } else { 0 };
    let to = if line == end.line {
        end.column
    } else {
        usize::MAX
    };
    (from < to).then_some(from..to)
}

fn display_width_from(text: &str, starting_cell: usize) -> usize {
    let mut cell = starting_cell;
    for grapheme in text.graphemes(true) {
        cell += grapheme_width(grapheme, cell);
    }
    cell.saturating_sub(starting_cell)
}

fn grapheme_width(grapheme: &str, cell: usize) -> usize {
    if grapheme == "\t" {
        4 - (cell % 4)
    } else {
        UnicodeWidthStr::width(grapheme)
    }
}

#[cfg(test)]
mod tests {
    use super::{TextPoint, TextSelection, cell_to_char, char_to_cell, selected_text, word_range};

    #[test]
    fn maps_tabs_wide_graphemes_and_extracts_multiline_text() {
        let line = "a\t\u{754c}🙂z";
        assert_eq!(char_to_cell(line, 2), 4);
        assert_eq!(cell_to_char(line, 5), 3);
        assert_eq!(word_range("one two", 5), 4..7);
        let lines = vec!["alpha".to_owned(), "\u{4e16}\u{754c}".to_owned()];
        assert_eq!(
            selected_text(
                &lines,
                TextSelection {
                    anchor: TextPoint { line: 0, column: 2 },
                    head: TextPoint { line: 1, column: 1 },
                }
            ),
            Some("pha\n\u{4e16}".to_owned())
        );
    }
}
