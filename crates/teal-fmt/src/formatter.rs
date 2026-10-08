//! Teal 源码的确定性空白格式化。
//!
//! 格式器保留词法单元的原始拼写和顺序，只规范化缩进、换行与词法单元之间的空白。
//! 因此字符串转义、数字拼写、标识符和行注释不会被重新生成。

use miette::Diagnostic;

use teal_compiler::{LexicalToken, LexicalTokenKind, ParseError, Source, parse, tokenize};

/// [`format()`] 与 [`format_named()`] 可能返回的错误。
#[derive(Debug, Diagnostic, thiserror::Error)]
pub enum FormatError {
    /// 输入不是完整、有效的 Teal 语法。
    #[error(transparent)]
    #[diagnostic(transparent)]
    Syntax(Box<ParseError>),
    /// 内部布局规则意外改变了词法单元序列。
    #[error("Formatted result failed to preserve original token sequence")]
    #[diagnostic(code(teal::format::token_stream))]
    TokenStreamChanged,
}

impl From<ParseError> for FormatError {
    fn from(error: ParseError) -> Self {
        Self::Syntax(Box::new(error))
    }
}

/// 格式化一份没有专用逻辑文件名的完整 Teal 源码。
///
/// 格式器使用四个空格缩进，把语句和多行大括号组放在独立行上，并把行尾统一为 `\n`。
/// 单行的字段、变体载荷和聚合字面量保持紧凑；原本包含换行、注释、嵌套大括号或语句的
/// 大括号组使用多行布局；同一作用域内相邻的命名 `fn`/`task` 声明之间保留一个空行。
/// 非空输出总是以一个换行结尾。
///
/// 格式化只改变词法单元之间的空白。返回前会重新词法分析结果，并核对每个词法单元的类别和
/// 原始拼写；该检查可防止相邻运算符或数字在删除空白后意外合并。
///
/// # Errors
/// 输入无法解析为完整 Teal 模块时返回 [`FormatError::Syntax`]。如果内部布局规则违反
/// token 保持不变量，则返回 [`FormatError::TokenStreamChanged`]，不会把结果交给调用方。
///
/// # Examples
/// ```
/// let formatted = teal_fmt::format("fn main(){let answer=42;}")?;
///
/// assert_eq!(formatted, "fn main() {\n    let answer = 42;\n}\n");
/// Ok::<(), teal_fmt::FormatError>(())
/// ```
pub fn format(source: &str) -> Result<String, FormatError> {
    format_named("<source>", source)
}

/// 使用指定的逻辑文件名格式化一份完整 Teal 源码。
///
/// 格式规则、token 保持不变量和错误条件与 [`format()`] 相同。`name` 只用于语法诊断，
/// 不会被当作文件系统路径读取。
///
/// # Errors
/// 输入不是完整有效的 Teal 模块，或内部 token 保持检查失败时返回 [`FormatError`]。
pub fn format_named(name: &str, source: &str) -> Result<String, FormatError> {
    format_parsed_source(&Source::new(name, source))
}

fn format_parsed_source(source: &Source) -> Result<String, FormatError> {
    parse(source)?;

    let tokens = tokenize(source);
    let brace_layouts = brace_layouts(&tokens, source.contents());
    let declaration_blanks = declaration_blanks(&tokens, source.contents());
    let mut formatter = Formatter::new(
        source.contents(),
        &tokens,
        &brace_layouts,
        &declaration_blanks,
    );
    formatter.format();
    let output = formatter.finish();

    let formatted_source = Source::new(source.filename.clone(), output.clone());
    let formatted_tokens = tokenize(&formatted_source);
    if !same_token_stream(&tokens, &formatted_tokens) {
        return Err(FormatError::TokenStreamChanged);
    }

    Ok(output)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BraceLayout {
    Inline,
    Multiline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delimiter {
    Parenthesis { attribute: bool },
    Bracket,
    Brace(BraceLayout),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Separator {
    None,
    Space,
}

struct Formatter<'a> {
    source: &'a str,
    tokens: &'a [LexicalToken],
    brace_layouts: &'a [Option<BraceLayout>],
    declaration_blanks: &'a [bool],
    output: String,
    delimiters: Vec<Delimiter>,
    indent: usize,
    line_start: bool,
    last_token: Option<String>,
    tight_next: bool,
}

impl<'a> Formatter<'a> {
    fn new(
        source: &'a str,
        tokens: &'a [LexicalToken],
        brace_layouts: &'a [Option<BraceLayout>],
        declaration_blanks: &'a [bool],
    ) -> Self {
        Self {
            source,
            tokens,
            brace_layouts,
            declaration_blanks,
            output: String::with_capacity(source.len().saturating_add(source.len() / 8)),
            delimiters: Vec::new(),
            indent: 0,
            line_start: true,
            last_token: None,
            tight_next: false,
        }
    }

    fn format(&mut self) {
        for index in 0..self.tokens.len() {
            if self.declaration_blanks[index] {
                self.blank_line();
            }
            self.preserve_blank_line(index);
            let text = self.tokens[index].text.as_str();

            if self.tokens[index].kind == LexicalTokenKind::Comment {
                self.comment(text);
                continue;
            }

            match text {
                "{" => self.open_brace(index),
                "}" => self.close_brace(index),
                "(" => self.open_parenthesis(index),
                ")" => self.close_parenthesis(index),
                "[" => self.open_bracket(),
                "]" => self.close_bracket(),
                ";" => self.semicolon(index),
                "," => self.comma(index),
                ":" => self.colon(index),
                "." | "::" => self.tight_punctuation(text),
                ".." => {
                    let separator = if self
                        .last_token
                        .as_deref()
                        .is_some_and(prefix_keyword_needs_space)
                    {
                        Separator::Space
                    } else {
                        Separator::None
                    };
                    self.write_token(text, separator);
                    self.tight_next = true;
                }
                "@" => {
                    self.write_token(text, Separator::None);
                    self.tight_next = true;
                }
                _ if self.tokens[index].kind == LexicalTokenKind::Operator => self.operator(index),
                _ => {
                    let separator = if matches!(
                        self.last_token.as_deref(),
                        Some(".") | Some("::") | Some("@")
                    ) {
                        Separator::None
                    } else {
                        Separator::Space
                    };
                    self.write_token(text, separator);
                }
            }
        }
    }

    fn finish(mut self) -> String {
        while matches!(self.output.as_bytes().last(), Some(b' ' | b'\n')) {
            self.output.pop();
        }
        if !self.output.is_empty() {
            self.output.push('\n');
        }
        self.output
    }

    fn open_brace(&mut self, index: usize) {
        let layout = self.brace_layouts[index].expect("every opening brace has a computed layout");
        self.write_token("{", Separator::Space);
        self.delimiters.push(Delimiter::Brace(layout));

        if self.next_text(index) == Some("}") {
            self.tight_next = true;
        } else if layout == BraceLayout::Multiline {
            self.indent += 1;
            if self.next_is_same_line_comment(index) {
                self.space();
            } else {
                self.newline();
            }
        } else {
            self.space();
        }
    }

    fn close_brace(&mut self, index: usize) {
        let layout = match self.delimiters.pop() {
            Some(Delimiter::Brace(layout)) => layout,
            _ => unreachable!("the parser accepted balanced braces"),
        };

        if layout == BraceLayout::Multiline {
            self.indent = self.indent.saturating_sub(1);
            self.newline();
        } else {
            self.trim_space();
            if self.last_token.as_deref() != Some("{") {
                self.space();
            }
        }
        self.write_token("}", Separator::None);

        if self.next_is_same_line_comment(index) {
            self.space();
            return;
        }
        match self.next_text(index) {
            Some("else" | "catch") => self.space(),
            Some(next) if stays_attached_to_closing_brace(next, self.next_kind(index)) => {}
            Some(_) => self.newline(),
            None => {}
        }
    }

    fn open_parenthesis(&mut self, index: usize) {
        let attribute = is_attribute_parenthesis(self.tokens, index);
        let separator = if self
            .last_token
            .as_deref()
            .is_some_and(prefix_keyword_needs_space)
        {
            Separator::Space
        } else {
            Separator::None
        };
        self.write_token("(", separator);
        self.delimiters.push(Delimiter::Parenthesis { attribute });
        self.tight_next = true;
    }

    fn close_parenthesis(&mut self, index: usize) {
        self.trim_space();
        self.write_token(")", Separator::None);
        let attribute = match self.delimiters.pop() {
            Some(Delimiter::Parenthesis { attribute }) => attribute,
            _ => unreachable!("the parser accepted balanced parentheses"),
        };
        if attribute && !matches!(self.next_text(index), None | Some(";" | "," | ")" | "]")) {
            self.newline();
        }
    }

    fn open_bracket(&mut self) {
        let separator = if self
            .last_token
            .as_deref()
            .is_some_and(prefix_keyword_needs_space)
        {
            Separator::Space
        } else {
            Separator::None
        };
        self.write_token("[", separator);
        self.delimiters.push(Delimiter::Bracket);
        self.tight_next = true;
    }

    fn close_bracket(&mut self) {
        self.trim_space();
        self.write_token("]", Separator::None);
        match self.delimiters.pop() {
            Some(Delimiter::Bracket) => {}
            _ => unreachable!("the parser accepted balanced brackets"),
        }
    }

    fn semicolon(&mut self, index: usize) {
        self.trim_space();
        self.write_token(";", Separator::None);
        if self.next_is_same_line_comment(index) {
            self.space();
        } else {
            self.newline();
        }
    }

    fn comma(&mut self, index: usize) {
        self.trim_space();
        self.write_token(",", Separator::None);
        if self.next_is_same_line_comment(index) {
            self.space();
        } else if matches!(
            self.delimiters.last(),
            Some(Delimiter::Brace(BraceLayout::Multiline))
        ) {
            self.newline();
        } else {
            self.space();
        }
    }

    fn colon(&mut self, index: usize) {
        self.trim_space();
        self.write_token(":", Separator::None);
        if !matches!(
            self.next_text(index),
            None | Some("," | ";" | ")" | "]" | "}")
        ) {
            self.space();
        }
    }

    fn tight_punctuation(&mut self, text: &str) {
        self.trim_space();
        self.write_token(text, Separator::None);
        self.tight_next = true;
    }

    fn operator(&mut self, index: usize) {
        let text = self.tokens[index].text.as_str();
        if is_unary_operator(self.tokens, index) {
            let separator = if self
                .last_token
                .as_deref()
                .is_some_and(prefix_keyword_needs_space)
            {
                Separator::Space
            } else {
                Separator::None
            };
            self.write_token(text, separator);
            self.tight_next = true;
        } else {
            self.space();
            self.write_token(text, Separator::None);
            self.space();
        }
    }

    fn comment(&mut self, text: &str) {
        if !self.line_start {
            self.space();
        }
        self.write_token(text, Separator::None);
        self.newline();
    }

    fn preserve_blank_line(&mut self, index: usize) {
        if index == 0 || !self.line_start || self.output.is_empty() {
            return;
        }
        let previous = &self.tokens[index - 1];
        let current = &self.tokens[index];
        let gap = &self.source[previous.span.end() as usize..current.span.start as usize];
        if line_break_count(gap) >= 2 && !self.output.ends_with("\n\n") {
            self.output.push('\n');
        }
    }

    fn next_text(&self, index: usize) -> Option<&str> {
        self.tokens.get(index + 1).map(|token| token.text.as_str())
    }

    fn next_kind(&self, index: usize) -> Option<LexicalTokenKind> {
        self.tokens.get(index + 1).map(|token| token.kind)
    }

    fn next_is_same_line_comment(&self, index: usize) -> bool {
        let Some(next) = self.tokens.get(index + 1) else {
            return false;
        };
        if next.kind != LexicalTokenKind::Comment {
            return false;
        }
        let current = &self.tokens[index];
        !self.source[current.span.end() as usize..next.span.start as usize].contains(['\n', '\r'])
    }

    fn write_token(&mut self, text: &str, separator: Separator) {
        let separator = if std::mem::take(&mut self.tight_next) {
            Separator::None
        } else {
            separator
        };
        match separator {
            Separator::None => {
                if self
                    .last_token
                    .as_deref()
                    .is_some_and(|previous| tokens_merge_without_separator(previous, text))
                    && !self.line_start
                {
                    self.space();
                }
            }
            Separator::Space => self.space(),
        }

        if self.line_start {
            for _ in 0..self.indent {
                self.output.push_str("    ");
            }
            self.line_start = false;
        }
        self.output.push_str(text);
        self.last_token = Some(text.to_owned());
    }

    fn space(&mut self) {
        if !self.line_start && !self.output.ends_with(' ') {
            self.output.push(' ');
        }
    }

    fn trim_space(&mut self) {
        while self.output.ends_with(' ') {
            self.output.pop();
        }
    }

    fn newline(&mut self) {
        self.trim_space();
        if !self.output.ends_with('\n') {
            self.output.push('\n');
        }
        self.line_start = true;
        self.tight_next = false;
    }

    fn blank_line(&mut self) {
        self.trim_space();
        if !self.output.ends_with('\n') {
            self.output.push('\n');
        }
        if !self.output.ends_with("\n\n") {
            self.output.push('\n');
        }
        self.line_start = true;
        self.tight_next = false;
    }
}

fn brace_layouts(tokens: &[LexicalToken], source: &str) -> Vec<Option<BraceLayout>> {
    let mut result = vec![None; tokens.len()];
    let mut openings = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        match token.text.as_str() {
            "{" => openings.push(index),
            "}" => {
                let opening = openings.pop().expect("the parser accepted balanced braces");
                let inner = &source[tokens[opening].span.end() as usize..token.span.start as usize];
                let nested_or_statement = tokens[opening + 1..index].iter().any(|inner_token| {
                    matches!(inner_token.text.as_str(), "{" | ";")
                        || inner_token.kind == LexicalTokenKind::Comment
                });
                let layout = if inner.contains(['\n', '\r']) || nested_or_statement {
                    BraceLayout::Multiline
                } else {
                    BraceLayout::Inline
                };
                result[opening] = Some(layout);
            }
            _ => {}
        }
    }
    debug_assert!(openings.is_empty());
    result
}

#[derive(Clone, Copy, Debug)]
struct NamedDeclaration {
    keyword: usize,
    end: usize,
    scope: Option<usize>,
}

/// 返回需要在命名函数声明前补充空行的 token 位置。
///
/// 声明前的注释、属性和可见性修饰符属于下一条声明，因此空行放在它们之前，
/// 而不是插在文档注释和声明之间。匿名 `fn`/`task` 表达式没有名称，不会进入
/// 这里的声明列表。
fn declaration_blanks(tokens: &[LexicalToken], source: &str) -> Vec<bool> {
    let matching = matching_braces(tokens);
    let mut declarations = Vec::new();
    let mut brace_stack = Vec::new();

    for (index, token) in tokens.iter().enumerate() {
        match token.text.as_str() {
            "fn" | "task"
                if next_non_comment(tokens, index + 1)
                    .is_some_and(|next| tokens[next].kind == LexicalTokenKind::Identifier) =>
            {
                if let Some(end) = declaration_end(tokens, index, &matching) {
                    declarations.push(NamedDeclaration {
                        keyword: index,
                        end,
                        scope: brace_stack.last().copied(),
                    });
                }
            }
            "{" => brace_stack.push(index),
            "}" => {
                brace_stack.pop();
            }
            _ => {}
        }
    }

    let mut blank_before = vec![false; tokens.len()];
    for pair in declarations.windows(2) {
        let [previous, next] = pair else {
            continue;
        };
        if previous.scope != next.scope
            || !declaration_prefix(tokens, previous.end + 1, next.keyword)
        {
            continue;
        }
        let mut start = previous.end + 1;
        while start < next.keyword
            && tokens[start].kind == LexicalTokenKind::Comment
            && !has_line_break(
                &source
                    [tokens[previous.end].span.end() as usize..tokens[start].span.start as usize],
            )
        {
            start += 1;
        }
        if start < next.keyword {
            blank_before[start] = true;
        } else {
            blank_before[next.keyword] = true;
        }
    }
    blank_before
}

fn matching_braces(tokens: &[LexicalToken]) -> Vec<Option<usize>> {
    let mut result = vec![None; tokens.len()];
    let mut openings = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        match token.text.as_str() {
            "{" => openings.push(index),
            "}" => {
                if let Some(opening) = openings.pop() {
                    result[opening] = Some(index);
                }
            }
            _ => {}
        }
    }
    result
}

fn declaration_end(
    tokens: &[LexicalToken],
    keyword: usize,
    matching: &[Option<usize>],
) -> Option<usize> {
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    let mut index = keyword + 1;
    while index < tokens.len() {
        let text = tokens[index].text.as_str();
        if tokens[index].kind == LexicalTokenKind::Comment {
            index += 1;
            continue;
        }
        match text {
            "(" => parentheses += 1,
            ")" => parentheses = parentheses.saturating_sub(1),
            "[" => brackets += 1,
            "]" => brackets = brackets.saturating_sub(1),
            ";" if parentheses == 0 && brackets == 0 => return Some(index),
            "{" if parentheses == 0 && brackets == 0 => {
                let close = matching[index]?;
                let next = next_non_comment(tokens, close + 1);
                // Braces in a return type (for example `{string: int}`) are
                // followed by the function body or a prototype semicolon.
                if matches!(
                    next.map(|next| tokens[next].text.as_str()),
                    Some("{") | Some(";")
                ) {
                    index = close + 1;
                    continue;
                }
                return Some(close);
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn next_non_comment(tokens: &[LexicalToken], mut index: usize) -> Option<usize> {
    while index < tokens.len() && tokens[index].kind == LexicalTokenKind::Comment {
        index += 1;
    }
    (index < tokens.len()).then_some(index)
}

fn declaration_prefix(tokens: &[LexicalToken], start: usize, end: usize) -> bool {
    let mut index = start;
    while index < end {
        match tokens[index].text.as_str() {
            "pub" | "static" => index += 1,
            "@" => {
                let Some(next) = consume_attribute(tokens, index, end) else {
                    return false;
                };
                index = next;
            }
            _ if tokens[index].kind == LexicalTokenKind::Comment => index += 1,
            _ => return false,
        }
    }
    true
}

fn consume_attribute(tokens: &[LexicalToken], at: usize, end: usize) -> Option<usize> {
    let mut index = at + 1;
    let mut saw_name = false;
    while index < end {
        if tokens[index].kind == LexicalTokenKind::Comment {
            index += 1;
            continue;
        }
        if matches!(
            tokens[index].text.as_str(),
            "pub" | "static" | "fn" | "task"
        ) {
            break;
        }
        if matches!(
            tokens[index].kind,
            LexicalTokenKind::Identifier | LexicalTokenKind::Keyword
        ) || tokens[index].text == "::"
        {
            saw_name = true;
            index += 1;
            continue;
        }
        break;
    }
    if !saw_name {
        return None;
    }
    match tokens.get(index).map(|token| token.text.as_str()) {
        Some("(") => consume_balanced(tokens, index, end, "(", ")"),
        Some("{") => consume_balanced(tokens, index, end, "{", "}"),
        Some("[") => consume_balanced(tokens, index, end, "[", "]"),
        _ => Some(index),
    }
}

fn consume_balanced(
    tokens: &[LexicalToken],
    start: usize,
    end: usize,
    opening: &str,
    closing: &str,
) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        match token.text.as_str() {
            text if text == opening => depth += 1,
            text if text == closing => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn has_line_break(text: &str) -> bool {
    text.contains(['\n', '\r'])
}

fn is_attribute_parenthesis(tokens: &[LexicalToken], opening: usize) -> bool {
    let mut cursor = opening;
    while cursor > 0 {
        cursor -= 1;
        match tokens[cursor].text.as_str() {
            "@" => return true,
            "::" => {}
            _ if tokens[cursor].kind == LexicalTokenKind::Identifier
                || tokens[cursor].kind == LexicalTokenKind::Keyword => {}
            _ => return false,
        }
    }
    false
}

fn is_unary_operator(tokens: &[LexicalToken], index: usize) -> bool {
    let text = tokens[index].text.as_str();
    if text == "!" {
        return true;
    }
    if !matches!(text, "+" | "-") {
        return false;
    }
    let Some(previous) = index
        .checked_sub(1)
        .and_then(|previous| tokens.get(previous))
    else {
        return true;
    };
    previous.kind == LexicalTokenKind::Operator
        || matches!(
            previous.text.as_str(),
            "(" | "[" | "{" | "," | ":" | ";" | "return" | "if" | "else" | "in" | "=>"
        )
}

fn stays_attached_to_closing_brace(next: &str, kind: Option<LexicalTokenKind>) -> bool {
    matches!(next, "}" | ")" | "]" | "," | ";" | "." | "::" | "(" | "[")
        || kind == Some(LexicalTokenKind::Operator)
}

fn prefix_keyword_needs_space(token: &str) -> bool {
    matches!(
        token,
        "if" | "for" | "in" | "match" | "return" | "catch" | "else"
    )
}

fn tokens_merge_without_separator(left: &str, right: &str) -> bool {
    let combined = format!("{left}{right}");
    let boundary = left.len();
    let source = Source::new("<format-token-pair>", combined);
    let tokens = tokenize(&source);
    !(tokens.len() == 2
        && tokens[0].kind != LexicalTokenKind::Error
        && tokens[0].kind != LexicalTokenKind::Comment
        && tokens[0].text == left
        && tokens[0].span.start as usize == 0
        && tokens[0].span.end() as usize == boundary
        && tokens[1].kind != LexicalTokenKind::Error
        && tokens[1].kind != LexicalTokenKind::Comment
        && tokens[1].text == right
        && tokens[1].span.start as usize == boundary
        && tokens[1].span.end() as usize == source.len())
}

fn same_token_stream(left: &[LexicalToken], right: &[LexicalToken]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.kind == right.kind && left.text == right.text)
}

fn line_break_count(text: &str) -> usize {
    let mut count = 0;
    let mut previous_was_carriage_return = false;
    for byte in text.bytes() {
        match byte {
            b'\r' => {
                count += 1;
                previous_was_carriage_return = true;
            }
            b'\n' if previous_was_carriage_return => previous_was_carriage_return = false,
            b'\n' => count += 1,
            _ => previous_was_carriage_return = false,
        }
    }
    count
}

#[cfg(test)]
#[path = "formatter/tests.rs"]
mod tests;
