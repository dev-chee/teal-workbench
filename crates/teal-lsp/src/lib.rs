//! Teal 的 Language Server Protocol 实现。
#![deny(missing_docs)]
//! Teal Language Server Protocol 实现。
//!
//! 服务器保留 UTF-8 源码快照，并按 LSP 要求在所有公开协议位置上转换 UTF-16 坐标。语法诊断使用当前文本；
//! 完整编译成功时导航使用跨模块 resolved 分析；编辑器暂时包含不完整语法时回退到最近一次成功快照。

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use serde_json::{Value, json};
use teal_compiler::{
    BuiltinOwner, CompilationAnalysis, CompileOptions, Item, LexicalTokenKind, LoadedSource,
    SemanticAnalysis, SemanticSymbol, SemanticSymbolKind, Source, SourceLoadError, SourceLoader,
    Span, analyze_source, compile_with_analysis, parse, tokenize,
};
use teal_fmt::format_named;
use teal_packages::{BuiltinInterface, BuiltinInterfaceEntry};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

mod document;
mod diagnostics;
mod queries;
mod protocol;
use document::{Document, ServerState};
use diagnostics::{DiagnosticReport, DiagnosticTracker};
pub use protocol::{run, run_stdio};

/// 映射 UTF-8 字节抵消零基LSP UTF-16 阵地和背部的零基 LSP UTF-16 位置。
#[derive(Clone, Debug)]
pub struct LineIndex {
    inner: teal_text::LineIndex,
}

impl LineIndex {
    /// 构建一个不可变源快照的索引。
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            inner: teal_text::LineIndex::new(text),
        }
    }

    fn from_shared(text: Arc<str>) -> Self {
        Self {
            inner: teal_text::LineIndex::from_shared(text),
        }
    }

    /// 将 LSP 位置转换为 UTF-8 字节偏移。
    #[must_use]
    pub fn offset(&self, line: u64, utf16_character: u64) -> Option<usize> {
        self.inner.offset(line, utf16_character)
    }

    /// 将 UTF-8 字节偏移转换成 LSP 位置对象。
    #[must_use]
    pub fn position(&self, offset: usize) -> Option<Value> {
        let position = self.inner.position(offset)?;
        Some(json!({ "line": position.line, "character": position.character }))
    }

    fn range(&self, span: Span) -> Value {
        json!({
            "start": self.position(span.start as usize).unwrap_or_else(|| json!({"line": 0, "character": 0})),
            "end": self.position(span.end() as usize).unwrap_or_else(|| json!({"line": 0, "character": 0}))
        })
    }
}

fn package_catalog() -> Result<&'static teal_packages::PackageCatalog, SourceLoadError> {
    static CATALOG: OnceLock<Result<teal_packages::PackageCatalog, String>> = OnceLock::new();
    CATALOG
        .get_or_init(|| {
            teal_packages::PackageCatalog::discover().map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|message| SourceLoadError {
            message: message.clone(),
        })
}

fn position_offset(index: &LineIndex, position: &Value) -> Option<usize> {
    index.offset(
        position.get("line")?.as_u64()?,
        position.get("character")?.as_u64()?,
    )
}

fn location(document: &Document, span: Span) -> Value {
    json!({ "uri": document.uri, "range": document.index.range(span) })
}

fn imported_location(loaded: &LoadedSource, span: Span) -> Option<Value> {
    let uri = match &loaded.source.provenance {
        teal_compiler::SourceProvenance::User => path_to_uri(&loaded.identity),
        _ => materialize_package_module(&loaded.source)?,
    };
    Some(json!({ "uri": uri, "range": LineIndex::new(loaded.source.contents()).range(span) }))
}

fn analysis_location(analysis: &SemanticAnalysis, span: Span) -> Option<Value> {
    let uri = match analysis.source_provenance() {
        teal_compiler::SourceProvenance::User => path_to_uri(Path::new(analysis.source_name())),
        provenance => materialize_package_module(&Source::with_provenance(
            analysis.source_name(),
            analysis.source_contents(),
            provenance.clone(),
        ))?,
    };
    Some(json!({
        "uri": uri,
        "range": LineIndex::new(analysis.source_contents()).range(span)
    }))
}

/// 只解析直接导入别名；嵌套限定路径不能误命中另一个同名根别名。
fn qualified_namespace(head: &str) -> Option<&str> {
    let head = head.trim_end().strip_suffix("::")?.trim_end();
    let name = trailing_identifier(head);
    if name.is_empty() || head[..head.len() - name.len()].trim_end().ends_with("::") {
        None
    } else {
        Some(name)
    }
}

fn member_documentation(source: &Source, span: Span) -> String {
    let before = source
        .contents()
        .get(..span.start as usize)
        .unwrap_or_default();
    let mut lines = before.lines().rev();
    // 第一行是 pub/task/fn 等声明前缀，文档位于其前方。
    lines.next();
    let mut documentation = lines
        .map(str::trim)
        .take_while(|line| line.starts_with("///"))
        .map(|line| line.trim_start_matches("///").trim())
        .collect::<Vec<_>>();
    documentation.reverse();
    documentation.join("\n")
}

/// 内嵌包没有可导航的工作区文件；按已验证内容摘要缓存只读源码。
fn materialize_package_module(source: &Source) -> Option<String> {
    let teal_compiler::SourceProvenance::Package {
        module,
        module_digest,
        ..
    } = &source.provenance
    else {
        return None;
    };
    let root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
        .join("Teal")
        .join("lsp")
        .join("modules")
        .join(module_digest.as_str());
    std::fs::create_dir_all(&root).ok()?;
    let path = root.join(Path::new(module.as_str()).file_name()?);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(source.contents()) {
        if path.exists() {
            std::fs::remove_file(&path).ok()?;
        }
        std::fs::write(&path, source.contents()).ok()?;
    }
    let mut permissions = std::fs::metadata(&path).ok()?.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions).ok()?;
    Some(path_to_uri(&path))
}

fn materialize_builtin_interface() -> Option<String> {
    let source = interface_source()?.to_owned();
    let digest = package_catalog().ok()?.builtin_interface()?.source_digest;
    let cache_root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
        .join("Teal")
        .join("lsp")
        .join("builtins")
        .join(&digest);
    std::fs::create_dir_all(&cache_root).ok()?;
    let path = cache_root.join("builtins.tli");
    let current = std::fs::read_to_string(&path).ok();
    if current.as_deref() != Some(source.as_str()) {
        if current.is_some() {
            std::fs::remove_file(&path).ok()?;
        }
        std::fs::write(&path, source).ok()?;
    }
    let mut permissions = std::fs::metadata(&path).ok()?.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions).ok()?;
    Some(path_to_uri(&path))
}

/// 返回当前进程使用的 Builtin 接口制品。
///
/// 编辑器只用它提供补全、悬停与定义位置；指针由包快照一次性校验并缓存。
fn builtin_interface() -> Option<&'static BuiltinInterface> {
    static INTERFACE: OnceLock<Option<BuiltinInterface>> = OnceLock::new();
    INTERFACE
        .get_or_init(|| {
            package_catalog()
                .ok()?
                .builtin_interface()
                .map(|resolved| resolved.interface)
        })
        .as_ref()
}

/// 返回制品文本。
fn interface_source() -> Option<&'static str> {
    builtin_interface().map(BuiltinInterface::source)
}

fn builtin_completion_candidates(
    document: &Document,
    offset: u32,
) -> Vec<(String, String, String)> {
    let Some(interface) = builtin_interface() else {
        return Vec::new();
    };
    let prefix = document
        .text
        .get(..offset as usize)
        .unwrap_or_default()
        .trim_end();
    let mut entries: Vec<&BuiltinInterfaceEntry> = Vec::new();
    if let Some(head) = prefix.strip_suffix("::") {
        let namespace = trailing_identifier(head);
        entries.extend(interface.entries().iter().filter(|entry| {
            matches!(&entry.entry.owner, BuiltinOwner::Namespace { namespace: owner, .. } if owner == namespace)
        }));
    } else if let Some(head) = prefix.strip_suffix('.') {
        let families = infer_receiver_families(document, head.len());
        entries.extend(method_entries(&families));
    } else {
        entries.extend(interface.entries().iter().filter(|entry| {
            matches!(&entry.entry.owner, BuiltinOwner::Namespace { namespace, .. } if namespace.is_empty())
        }));
    }
    let mut output = entries
        .into_iter()
        .map(|entry| {
            (
                entry.entry.name.clone(),
                entry
                    .signatures
                    .iter()
                    .map(|signature| signature.display.as_str())
                    .collect::<Vec<_>>()
                    .join(" | "),
                entry
                    .signatures
                    .iter()
                    .find(|signature| !signature.documentation.is_empty())
                    .map(|signature| signature.documentation.clone())
                    .unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    if !prefix.ends_with('.') && !prefix.ends_with("::") {
        output.extend(["Task", "reflect"].into_iter().map(|namespace| {
            (
                namespace.to_owned(),
                "builtin namespace".to_owned(),
                String::new(),
            )
        }));
    }
    output
}

fn builtin_at(document: &Document, offset: u32) -> Option<(&'static BuiltinInterfaceEntry, Span)> {
    let (name, span) = identifier_at(&document.text, offset)?;
    let start = span.start as usize;
    let head = document.text.get(..span.end() as usize)?;
    builtin_for_call_head(document, head, &name)
        .map(|entry| (entry, Span::new(span.start, span.size)))
        .or_else(|| {
            let before = document.text.get(..start)?.trim_end();
            if before.ends_with('.') || before.ends_with("::") {
                None
            } else {
                builtin_interface()?
                    .find_free("", &name)
                    .map(|entry| (entry, span))
            }
        })
}

fn builtin_for_call_head(
    document: &Document,
    call_head: &str,
    name: &str,
) -> Option<&'static BuiltinInterfaceEntry> {
    let interface = builtin_interface()?;
    let name_start = call_head.rfind(name)?;
    let before = call_head[..name_start].trim_end();
    if let Some(namespace_head) = before.strip_suffix("::") {
        return interface.find_free(trailing_identifier(namespace_head), name);
    }
    if let Some(receiver_head) = before.strip_suffix('.') {
        let families = infer_receiver_families(document, receiver_head.len());
        return method_entry(&families, name).or_else(|| interface.find_method_named(name));
    }
    interface.find_free("", name)
}

fn method_entries(families: &[&str]) -> Vec<&'static BuiltinInterfaceEntry> {
    let Some(interface) = builtin_interface() else {
        return Vec::new();
    };
    let all = interface
        .entries()
        .iter()
        .filter(|entry| matches!(entry.entry.owner, BuiltinOwner::Receiver { .. }));
    if families.is_empty() {
        return all.collect();
    }
    all.filter(|entry| {
        matches!(&entry.entry.owner, BuiltinOwner::Receiver { family, .. } if families.iter().any(|candidate| candidate == family))
    })
    .collect()
}

fn method_entry(families: &[&str], name: &str) -> Option<&'static BuiltinInterfaceEntry> {
    let interface = builtin_interface()?;
    families
        .iter()
        .find_map(|family| interface.find_method(family, name))
}

fn infer_receiver_families(document: &Document, end: usize) -> Vec<&'static str> {
    let expression = document.text.get(..end).unwrap_or_default().trim_end();
    if expression.ends_with('"') {
        return vec!["string"];
    }
    if expression.ends_with(']') {
        let start = expression.rfind('[').unwrap_or(0);
        let contents = &expression[start + 1..expression.len().saturating_sub(1)];
        return if !contents.trim().is_empty()
            && contents
                .split(',')
                .all(|item| item.trim().starts_with('"') && item.trim().ends_with('"'))
        {
            vec!["string_array", "array"]
        } else {
            vec!["array"]
        };
    }
    if expression.ends_with('}') {
        return vec!["map"];
    }
    let token = trailing_identifier(expression);
    if !token.is_empty()
        && let Some(detail) = document
            .analysis()
            .symbols()
            .iter()
            .rev()
            .find(|symbol| symbol.name == token)
            .and_then(|symbol| symbol.detail.as_deref())
    {
        return families_for_type_detail(&document.text, detail);
    }
    let tail = expression
        .split(|character: char| character.is_whitespace() || "(){}[];,".contains(character))
        .next_back()
        .unwrap_or_default();
    if tail.parse::<i32>().is_ok() {
        vec!["number"]
    } else if tail.parse::<f64>().is_ok() {
        vec!["float", "number"]
    } else {
        Vec::new()
    }
}

fn families_for_type_detail(source: &str, detail: &str) -> Vec<&'static str> {
    let detail = detail.trim();
    match detail {
        "string" => vec!["string"],
        "int" => vec!["number"],
        "float" => vec!["float", "number"],
        "string[]" => vec!["string_array", "array"],
        "int[]" => vec!["int_array", "array"],
        value if value.ends_with("[]") => vec!["array"],
        value if value.starts_with('{') => vec!["map"],
        value if value.starts_with("Task<") || value.starts_with("Task(") => vec!["task"],
        value if source.contains(&format!("agent {value}")) => vec!["agent"],
        _ => Vec::new(),
    }
}

fn trailing_identifier(source: &str) -> &str {
    let end = source.trim_end().len();
    let source = &source[..end];
    let start = source
        .char_indices()
        .rev()
        .find(|(_, character)| !(*character == '_' || character.is_alphanumeric()))
        .map_or(0, |(index, character)| index + character.len_utf8());
    &source[start..]
}

fn identifier_at(source: &str, offset: u32) -> Option<(String, Span)> {
    let mut offset = usize::try_from(offset).ok()?.min(source.len());
    if offset == source.len()
        || !source
            .as_bytes()
            .get(offset)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        offset = offset.checked_sub(1)?;
    }
    let bytes = source.as_bytes();
    if !bytes
        .get(offset)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        return None;
    }
    let mut start = offset;
    while start > 0
        && bytes[start - 1].is_ascii()
        && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_')
    {
        start -= 1;
    }
    let mut end = offset + 1;
    while end < bytes.len()
        && bytes[end].is_ascii()
        && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
    {
        end += 1;
    }
    Some((
        source[start..end].to_owned(),
        Span::from_bounds(start as u32, end as u32),
    ))
}

fn builtin_markdown(entry: &BuiltinInterfaceEntry) -> String {
    let signatures = entry
        .signatures
        .iter()
        .map(|signature| signature.display.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let documentation = entry
        .signatures
        .iter()
        .find(|signature| !signature.documentation.is_empty())
        .map(|signature| signature.documentation.as_str())
        .unwrap_or_default();
    if documentation.is_empty() {
        format!("```teal\n{signatures}\n```")
    } else {
        format!("```teal\n{signatures}\n```\n\n{documentation}")
    }
}

fn builtin_location(uri: &str, entry: &BuiltinInterfaceEntry) -> Value {
    let signature = &entry.signatures[0];
    let line = signature.line.saturating_sub(1);
    let source_line = interface_source()
        .unwrap_or_default()
        .lines()
        .nth(line as usize)
        .unwrap_or_default();
    let start = source_line.find(&entry.entry.name).unwrap_or(0);
    let end = start + entry.entry.name.len();
    json!({
        "uri": uri,
        "range": {
            "start": { "line": line, "character": start },
            "end": { "line": line, "character": end }
        }
    })
}

fn completion_kind(kind: SemanticSymbolKind) -> u32 {
    match kind {
        SemanticSymbolKind::Constant => 21,
        SemanticSymbolKind::Function
        | SemanticSymbolKind::Method
        | SemanticSymbolKind::AssociatedFunction
        | SemanticSymbolKind::NativeFunction => 3,
        SemanticSymbolKind::Class | SemanticSymbolKind::Trait => 7,
        SemanticSymbolKind::Enum => 13,
        SemanticSymbolKind::Field | SemanticSymbolKind::EnumVariant => 5,
        SemanticSymbolKind::Namespace => 9,
        SemanticSymbolKind::Alias => 18,
        SemanticSymbolKind::Parameter
        | SemanticSymbolKind::Variable
        | SemanticSymbolKind::Static => 6,
    }
}

fn symbol_kind(kind: SemanticSymbolKind) -> u32 {
    match kind {
        SemanticSymbolKind::Constant => 14,
        SemanticSymbolKind::Function
        | SemanticSymbolKind::Method
        | SemanticSymbolKind::AssociatedFunction
        | SemanticSymbolKind::NativeFunction => 12,
        SemanticSymbolKind::Class | SemanticSymbolKind::Trait => 5,
        SemanticSymbolKind::Enum => 10,
        SemanticSymbolKind::Field => 8,
        SemanticSymbolKind::EnumVariant => 22,
        SemanticSymbolKind::Namespace => 2,
        SemanticSymbolKind::Alias => 13,
        SemanticSymbolKind::Parameter
        | SemanticSymbolKind::Variable
        | SemanticSymbolKind::Static => 13,
    }
}

fn lexical_token_kind(kind: LexicalTokenKind) -> u32 {
    match kind {
        LexicalTokenKind::Keyword => 9,
        LexicalTokenKind::Identifier | LexicalTokenKind::Label => 5,
        LexicalTokenKind::Number => 11,
        LexicalTokenKind::String => 10,
        LexicalTokenKind::Comment => 12,
        LexicalTokenKind::Operator | LexicalTokenKind::Punctuation | LexicalTokenKind::Error => 13,
    }
}

fn valid_identifier(name: &str) -> bool {
    let source = Source::new("<identifier>", name);
    matches!(
        tokenize(&source).as_slice(),
        [token]
            if token.kind == LexicalTokenKind::Identifier
                && token.text == name
                && token.span == Span::new(0, name.len() as u32)
    )
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let url = url::Url::parse(uri).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    if let Ok(path) = url.to_file_path() {
        return Some(normalize_path(path));
    }
    #[cfg(windows)]
    {
        // 测试和内存客户端可能使用没有盘符的 `file:///memory/...`；它不是 Windows
        // 本地绝对路径，但仍沿用既有的相对逻辑身份。
        let decoded = percent_decode(url.path());
        let decoded = decoded
            .strip_prefix('/')
            .unwrap_or(&decoded)
            .replace('/', "\\");
        Some(normalize_path(PathBuf::from(decoded)))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// 将客户端等价的文件 URI 与规范路径收敛为一个文档身份。
fn document_uri_and_path(uri: &str) -> Option<(String, PathBuf)> {
    let path = uri_to_path(uri)?;
    let canonical_uri = if path.is_absolute() {
        path_to_uri(&path)
    } else {
        uri.to_owned()
    };
    Some((canonical_uri, path))
}

fn path_to_uri(path: &Path) -> String {
    let mut path = path_without_verbatim_prefix(path);
    if path.is_relative()
        && let Ok(current) = std::env::current_dir()
    {
        path = current.join(path);
    }
    url::Url::from_file_path(&path)
        .map(|url| url.to_string())
        .unwrap_or_else(|()| "file:///".to_owned())
}

/// Windows 规范化会添加 `\\?\`，文件 URI 不得把该设备前缀暴露给 LSP 客户端。
#[cfg(windows)]
fn path_without_verbatim_prefix(path: &Path) -> PathBuf {
    let display = path.to_string_lossy();
    if let Some(rest) = display.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = display.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_owned()
    }
}

/// 非 Windows 路径没有扩展设备前缀。
#[cfg(not(windows))]
fn path_without_verbatim_prefix(path: &Path) -> PathBuf {
    path.to_owned()
}

/// 解码 Windows 虚拟文件 URI 回退中的 UTF-8 百分号字节。
#[cfg(windows)]
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2]))
        {
            output.push(high * 16 + low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

/// 返回一个十六进制 ASCII 数字的值。
#[cfg(windows)]
const fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn normalize_path(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

#[cfg(test)]
#[path = "tests/regression_tests.rs"]
mod regression_tests;

#[cfg(test)]
mod tests {
    use super::{LineIndex, ServerState, run, uri_to_path};
    use serde_json::{Value, json};
    use teal_compiler::Span;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn line_index_uses_utf16_code_units() {
        let index = LineIndex::new("a😀b\n\u{4e2d}");
        assert_eq!(index.offset(0, 3), Some(5));
        assert_eq!(
            index.position(5),
            Some(json!({ "line": 0, "character": 3 }))
        );
        assert_eq!(
            index.offset(0, 2),
            None,
            "positions cannot split a surrogate pair"
        );
    }

    #[test]
    fn line_index_converts_nonzero_start_plus_size_to_lsp_boundaries() {
        let index = LineIndex::new("head\n😀tail");

        assert_eq!(
            index.range(Span::new(5, 4)),
            json!({
                "start": { "line": 1, "character": 0 },
                "end": { "line": 1, "character": 2 }
            })
        );
    }

    #[test]
    fn public_modifier_is_encoded_as_an_lsp_keyword_and_rejected_as_a_rename() {
        let uri = "file:///memory/public.tl";
        let mut state = ServerState::default();
        state.open(&json!({ "textDocument": {
            "uri": uri,
            "text": "pub fn helper() {}"
        }}));
        let tokens = state.semantic_tokens(&json!({ "textDocument": { "uri": uri } }));
        assert_eq!(
            tokens["data"][3], 9,
            "the first token should use the keyword legend index"
        );
        assert!(!super::valid_identifier("pub"));
        assert!(!super::valid_identifier("impl"));
        assert!(!super::valid_identifier("trait"));
        assert!(super::valid_identifier("pub_value"));
    }

    #[test]
    fn re_export_aliases_participate_in_completion_and_rename() {
        let uri = "file:///memory/re-export.tl";
        let text =
            "import \"leaf.tl\" as leaf; pub leaf::value as renamed; fn main() { renamed(); }";
        let mut state = ServerState::default();
        state.open(&json!({ "textDocument": { "uri": uri, "text": text } }));

        let completions = state.completion(&json!({
            "textDocument": { "uri": uri },
            "position": { "line": 0, "character": text.len() }
        }));
        assert!(
            completions
                .as_array()
                .expect("completion result should be an array")
                .iter()
                .any(|item| item["label"] == "renamed" && item["kind"] == 18)
        );

        let use_offset = text
            .rfind("renamed")
            .expect("fixture contains an alias use")
            + 1;
        let rename = state.rename(&json!({
            "textDocument": { "uri": uri },
            "position": { "line": 0, "character": use_offset },
            "newName": "public_value"
        }));
        let edits = rename["changes"][uri]
            .as_array()
            .expect("rename should return declaration and use edits");
        assert_eq!(edits.len(), 2);
        assert!(edits.iter().all(|edit| edit["newText"] == "public_value"));
    }

    #[test]
    fn configured_entry_file_uses_main() {
        let uri = "file:///memory/entries.tl";
        let mut state = ServerState::default();
        let path = uri_to_path(uri).expect("file URI should resolve");
        state.entry_files.push(path);
        state.open(&json!({ "textDocument": {
            "uri": uri,
            "text": "fn main() {}"
        }}));
        assert!(state.diagnostics(uri).is_empty());
    }

    #[test]
    fn invalid_entry_target_produces_a_compiler_diagnostic() {
        let uri = "file:///memory/invalid-entry.tl";
        let mut state = ServerState::default();
        state.open(&json!({ "textDocument": {
            "uri": uri,
            "text": "pub fn main() {}"
        }}));
        let diagnostics = state.diagnostics(uri);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0]["code"], "teal::entry");
        assert!(
            diagnostics[0]["message"]
                .as_str()
                .is_some_and(|message| message.contains("private, parameterless"))
        );
    }

    #[test]
    fn imported_private_names_produce_visibility_diagnostics() {
        let dependency_uri = "file:///memory/dep.tl";
        let root_uri = "file:///memory/private-use.tl";
        let mut state = ServerState::default();
        state.open(&json!({ "textDocument": {
            "uri": dependency_uri,
            "text": "fn hidden() {}"
        }}));
        state.open(&json!({ "textDocument": {
            "uri": root_uri,
            "text": "import \"dep.tl\" as dep;  fn main() { dep::hidden(); }"
        }}));

        let diagnostics = state.diagnostics(root_uri);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0]["message"].as_str().is_some_and(|message| {
            message.contains("Private module declaration") && message.contains("dep.tl")
        }));
    }

    #[test]
    fn parser_diagnostics_publish_stable_codes() {
        let uri = "file:///memory/parse-error.tl";
        let mut state = ServerState::default();
        state.open(&json!({ "textDocument": {
            "uri": uri,
            "text": "fn main( {"
        }}));

        let diagnostics = state.diagnostics(uri);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0]["code"], "teal::parse");
    }

    #[tokio::test]
    async fn stdio_protocol_advertises_and_serves_core_extensions() {
        let messages = [
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
            json!({ "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": { "textDocument": {
                "uri": "file:///memory/main.tl", "languageId": "teal", "version": 1,
                "text": "fn add(value: int) -> int { return value; }\nfn main() { let answer = add(1); }"
            } } }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "textDocument/hover", "params": {
                "textDocument": { "uri": "file:///memory/main.tl" }, "position": { "line": 1, "character": 25 }
            } }),
            json!({ "jsonrpc": "2.0", "id": 3, "method": "textDocument/formatting", "params": {
                "textDocument": { "uri": "file:///memory/main.tl" },
                "options": { "tabSize": 4, "insertSpaces": true }
            } }),
            json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": null }),
            json!({ "jsonrpc": "2.0", "method": "exit" }),
        ];
        let (mut client, mut server_input) = tokio::io::duplex(32 * 1024);
        let (mut server_output, mut client_output) = tokio::io::duplex(32 * 1024);
        for message in messages {
            let body = serde_json::to_vec(&message).unwrap();
            client
                .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
                .await
                .unwrap();
            client.write_all(&body).await.unwrap();
        }
        client.shutdown().await.unwrap();
        run(&mut server_input, &mut server_output).await.unwrap();
        drop(server_output);
        let mut output = Vec::new();
        client_output.read_to_end(&mut output).await.unwrap();
        let text = String::from_utf8(output).unwrap();
        let bodies = text
            .split("Content-Length: ")
            .skip(1)
            .filter_map(|message| message.split_once("\r\n\r\n").map(|(_, body)| body))
            .filter_map(|body| serde_json::from_str::<Value>(body).ok())
            .collect::<Vec<_>>();
        assert!(
            bodies
                .iter()
                .any(|message| message.get("id") == Some(&json!(1))
                    && message["result"]["capabilities"]["renameProvider"].is_object())
        );
        assert!(
            bodies
                .iter()
                .any(|message| message.get("id") == Some(&json!(2))
                    && message["result"]["contents"].is_object())
        );
        assert!(bodies.iter().any(|message| {
            message.get("id") == Some(&json!(1))
                && message["result"]["capabilities"]["documentFormattingProvider"] == true
        }));
        assert!(bodies.iter().any(|message| {
            message.get("id") == Some(&json!(3))
                && message["result"].as_array().is_some_and(|edits| {
                    edits.len() == 1
                        && edits[0]["newText"]
                            .as_str()
                            .is_some_and(|text| text.contains("    return value;"))
                })
        }));
    }
}

#[cfg(test)]
mod refactor_bench;
