//! Markdown 到终端文本行的结构化渲染。
//!
//! 保留标题、列表、代码和链接等可读结构，不执行嵌入脚本；语法高亮与交互导航由面板另行处理。

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// 一行已渲染 Markdown 的语义样式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarkdownStyle {
    /// 普通段落文本。
    #[default]
    Paragraph,

    /// 带从一开始级别的标题。
    Heading(u8),

    /// 列表项。
    ListItem,

    /// 块引用。
    Quote,

    /// 围栏或缩进代码。
    Code,

    /// 水平或表格结构。
    Table,
}

/// 终端 Markdown 渲染器生成的一行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkdownLine {
    /// 纯显示文本。
    pub text: String,

    /// 语义终端样式。
    pub style: MarkdownStyle,

    /// 可选围栏代码语言标识。
    pub language: Option<String>,
}

/// 已解析 Markdown 行和链接目标。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MarkdownDocument {
    /// 按源码顺序排列的渲染行。
    pub lines: Vec<MarkdownLine>,

    /// 按源码顺序排列的链接标签与目标。
    pub links: Vec<(String, String)>,
}

impl MarkdownDocument {
    /// 解析包含表格、任务列表、删除线和围栏代码的 Markdown。
    #[must_use]
    pub fn parse(source: &str) -> Self {
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_FOOTNOTES;
        let mut document = Self::default();
        let mut current = String::new();
        let mut style = MarkdownStyle::Paragraph;
        let mut code_language = None;
        let mut link = None::<(String, String)>;
        for event in Parser::new_ext(source, options) {
            match event {
                Event::Start(Tag::Heading { level, .. }) => {
                    flush(&mut document, &mut current, style, code_language.as_deref());
                    style = MarkdownStyle::Heading(heading_level(level));
                }
                Event::End(
                    TagEnd::Heading(_) | TagEnd::Item | TagEnd::BlockQuote(_) | TagEnd::Table,
                ) => {
                    flush(&mut document, &mut current, style, code_language.as_deref());
                    style = MarkdownStyle::Paragraph;
                }
                Event::Start(Tag::Item) => {
                    flush(&mut document, &mut current, style, code_language.as_deref());
                    style = MarkdownStyle::ListItem;
                    current.push_str("• ");
                }
                Event::Start(Tag::BlockQuote(_)) => style = MarkdownStyle::Quote,
                Event::Start(Tag::CodeBlock(kind)) => {
                    flush(&mut document, &mut current, style, code_language.as_deref());
                    style = MarkdownStyle::Code;
                    code_language = match kind {
                        pulldown_cmark::CodeBlockKind::Fenced(language) if !language.is_empty() => {
                            Some(language.into_string())
                        }
                        _ => None,
                    };
                }
                Event::End(TagEnd::CodeBlock) => {
                    flush(&mut document, &mut current, style, code_language.as_deref());
                    style = MarkdownStyle::Paragraph;
                    code_language = None;
                }
                Event::Start(Tag::Table(_) | Tag::TableRow) => {
                    style = MarkdownStyle::Table;
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    link = Some((String::new(), dest_url.into_string()));
                }
                Event::End(TagEnd::Link) => {
                    if let Some(link) = link.take() {
                        document.links.push(link);
                    }
                }
                Event::Text(text) | Event::Code(text) => {
                    current.push_str(&text);
                    if let Some((label, _)) = &mut link {
                        label.push_str(&text);
                    }
                }
                Event::SoftBreak => current.push(' '),
                Event::HardBreak | Event::End(TagEnd::Paragraph) => {
                    flush(&mut document, &mut current, style, code_language.as_deref());
                }
                Event::Rule => document.lines.push(MarkdownLine {
                    text: "─".repeat(20),
                    style: MarkdownStyle::Table,
                    language: None,
                }),
                _ => {}
            }
        }
        flush(&mut document, &mut current, style, code_language.as_deref());
        document
    }
}

fn flush(
    document: &mut MarkdownDocument,
    current: &mut String,
    style: MarkdownStyle,
    language: Option<&str>,
) {
    if current.is_empty() {
        return;
    }
    for line in current.lines() {
        document.lines.push(MarkdownLine {
            text: line.to_owned(),
            style,
            language: language.map(ToOwned::to_owned),
        });
    }
    current.clear();
}

const fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::{MarkdownDocument, MarkdownStyle};

    #[test]
    fn renders_headings_lists_links_tables_and_code_languages() {
        let document = MarkdownDocument::parse(
            "# Title\n\n- [link](https://example.test)\n\n```rust\nfn main() {}\n```\n",
        );
        assert!(
            document
                .lines
                .iter()
                .any(|line| line.style == MarkdownStyle::Heading(1))
        );
        assert!(
            document
                .lines
                .iter()
                .any(|line| line.language.as_deref() == Some("rust"))
        );
        assert_eq!(
            document.links,
            [("link".to_owned(), "https://example.test".to_owned())]
        );
    }
}
