//! 基于同一编译快照的语言查询；协议位置统一使用 UTF-16。

use super::*;

impl ServerState {
    pub(super) fn document_and_offset(&self, params: &Value) -> Option<(&Document, u32)> {
        let supplied_uri = params.get("textDocument")?.get("uri")?.as_str()?;
        let (uri, _) = document_uri_and_path(supplied_uri)?;
        let document = self.documents.get(&uri)?;
        let offset = position_offset(&document.index, params.get("position")?)?;
        Some((document, u32::try_from(offset).ok()?))
    }

    pub(super) fn completion(&self, params: &Value) -> Value {
        let Some((document, offset)) = self.document_and_offset(params) else {
            return json!([]);
        };
        let mut names = HashSet::new();
        let mut items = Vec::new();
        let prefix = document
            .text
            .get(..offset as usize)
            .unwrap_or_default()
            .trim_end();
        let partial = trailing_identifier(prefix);
        let head = if prefix.ends_with("::") {
            prefix
        } else {
            &prefix[..prefix.len().saturating_sub(partial.len())]
        };
        if let Some(namespace) = qualified_namespace(head)
            && let Some((loaded, members)) = self.imported_module(document, namespace)
        {
            return json!(members.into_iter().map(|symbol| json!({
                "label": symbol.name,
                "kind": completion_kind(symbol.kind),
                "detail": symbol.detail,
                "documentation": { "kind": "markdown", "value": member_documentation(&loaded.source, symbol.selection_span) }
            })).collect::<Vec<_>>());
        }
        for keyword in [
            "fn", "let", "const", "static", "class", "enum", "if", "else", "for", "match",
            "return", "catch", "throw", "import", "true", "false", "none",
        ] {
            names.insert(keyword.to_owned());
            items.push(json!({ "label": keyword, "kind": 14 }));
        }
        for symbol in document.analysis().visible_symbols(offset) {
            if names.insert(symbol.name.clone()) {
                items.push(json!({
                    "label": symbol.name,
                    "kind": completion_kind(symbol.kind),
                    "detail": symbol.detail
                }));
            }
        }
        if self.builtin_catalog_ready {
            for (label, detail, documentation) in builtin_completion_candidates(document, offset) {
                if names.insert(label.clone()) {
                    items.push(json!({
                        "label": label,
                        "kind": 3,
                        "detail": detail,
                        "documentation": { "kind": "markdown", "value": documentation }
                    }));
                }
            }
        }
        json!(items)
    }

    pub(super) fn hover(&self, params: &Value) -> Value {
        let Some((document, offset)) = self.document_and_offset(params) else {
            return Value::Null;
        };
        if let Some((loaded, symbol, span)) = self.imported_member_at(document, offset) {
            return json!({
                "contents": { "kind": "markdown", "value": format!("```teal\n{}\n```\n{}",
                    symbol.detail.as_deref().unwrap_or(&symbol.name),
                    member_documentation(&loaded.source, symbol.selection_span)) },
                "range": document.index.range(span)
            });
        }
        if let Some(symbol) = document.analysis().symbol_at(offset) {
            let detail = symbol
                .detail
                .clone()
                .unwrap_or_else(|| format!("{:?} {}", symbol.kind, symbol.name));
            return json!({
                "contents": { "kind": "markdown", "value": format!("```teal\n{detail}\n```") },
                "range": document.index.range(symbol.selection_span)
            });
        }
        let Some((entry, span)) = self
            .builtin_catalog_ready
            .then(|| builtin_at(document, offset))
            .flatten()
        else {
            return Value::Null;
        };
        json!({
            "contents": {
                "kind": "markdown",
                "value": builtin_markdown(entry)
            },
            "range": document.index.range(span)
        })
    }

    pub(super) fn signature_help(&self, params: &Value) -> Value {
        let Some((document, offset)) = self.document_and_offset(params) else {
            return Value::Null;
        };
        let prefix = document.text.get(..offset as usize).unwrap_or_default();
        let Some((open, active_parameter)) = active_call(&document.text, offset as usize) else {
            return Value::Null;
        };
        let call_head = prefix[..open].trim_end();
        let name = call_head
            .split(|character: char| !character.is_alphanumeric() && character != '_')
            .next_back()
            .unwrap_or_default();
        if let Some(namespace) = qualified_namespace(&call_head[..call_head.len() - name.len()])
            && let Some((loaded, members)) = self.imported_module(document, namespace)
        {
            return members.into_iter().find(|symbol| symbol.name == name).map(|symbol| json!({
                "signatures": [{
                    "label": symbol.detail.as_deref().unwrap_or(&symbol.name),
                    "documentation": { "kind": "markdown", "value": member_documentation(&loaded.source, symbol.selection_span) }
                }],
                "activeSignature": 0,
                "activeParameter": active_parameter
            })).unwrap_or(Value::Null);
        }
        if self.builtin_catalog_ready
            && let Some(entry) = builtin_for_call_head(document, call_head, name)
        {
            let active_signature = entry
                .entry
                .signatures
                .iter()
                .position(|signature| {
                    signature.parameters.len() > active_parameter
                        || signature
                            .parameters
                            .last()
                            .is_some_and(|parameter| parameter.variadic)
                })
                .unwrap_or(0);
            return json!({
                "signatures": entry.signatures.iter().map(|signature| json!({
                    "label": signature.display,
                    "documentation": { "kind": "markdown", "value": signature.documentation }
                })).collect::<Vec<_>>(),
                "activeSignature": active_signature,
                "activeParameter": active_parameter
            });
        }
        let Some(symbol) = document.analysis().symbols().iter().find(|symbol| {
            matches!(
                symbol.kind,
                SemanticSymbolKind::Function
                    | SemanticSymbolKind::Method
                    | SemanticSymbolKind::AssociatedFunction
                    | SemanticSymbolKind::NativeFunction
            ) && symbol.name == name
        }) else {
            return Value::Null;
        };
        json!({
            "signatures": [{ "label": symbol.detail.clone().unwrap_or_else(|| format!("{}(...)" , symbol.name)) }],
            "activeSignature": 0,
            "activeParameter": active_parameter
        })
    }

    pub(super) fn definition(&self, params: &Value) -> Value {
        let Some((document, offset)) = self.document_and_offset(params) else {
            return Value::Null;
        };
        if let Some((source, compilation)) = document.compilation()
            && let Some(reference) = compilation
                .source(source)
                .and_then(|source| source.reference_at(offset))
            && let Some(symbol) = compilation.symbol(reference.symbol)
            && let Some(target) = compilation.source(reference.symbol.source())
        {
            return analysis_location(target, symbol.selection_span).unwrap_or(Value::Null);
        }
        if let Some((loaded, symbol, _)) = self.imported_member_at(document, offset) {
            return imported_location(&loaded, symbol.selection_span).unwrap_or(Value::Null);
        }
        if let Some(symbol) = document.analysis().symbol_at(offset) {
            return location(document, symbol.selection_span);
        }
        let Some((entry, _)) = builtin_at(document, offset) else {
            return Value::Null;
        };
        let Some(uri) = &self.builtin_uri else {
            return Value::Null;
        };
        builtin_location(uri, entry)
    }

    /// 包与工作区模块复用同一加载器；导出范围来自 AST，不把私有 Host 声明暴露为成员。
    pub(super) fn imported_module(
        &self,
        document: &Document,
        namespace: &str,
    ) -> Option<(LoadedSource, Vec<SemanticSymbol>)> {
        let import = document
            .current
            .imports()
            .iter()
            .find(|import| import.alias.as_deref() == Some(namespace))?;
        let loaded = self.loader().load(&document.path, &import.path).ok()?;
        let module = parse(&loaded.source).ok()?;
        let exports: Vec<_> = module
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Function(item) if item.visibility.is_public() => {
                    Some((item.name.as_str(), item.span))
                }
                Item::Static(item) if item.visibility.is_public() => {
                    Some((item.name.as_str(), item.span))
                }
                Item::Class(item) if item.visibility.is_public() => {
                    Some((item.name.as_str(), item.span))
                }
                Item::Enum(item) if item.visibility.is_public() => {
                    Some((item.name.as_str(), item.span))
                }
                Item::Trait(item) if item.visibility.is_public() => {
                    Some((item.name.as_str(), item.span))
                }
                Item::ReExport(item) => {
                    Some((item.exported_name().as_str(), item.exported_name_span()))
                }
                _ => None,
            })
            .collect();
        let analysis = analyze_source(&loaded.source);
        let members = analysis
            .symbols()
            .iter()
            .filter(|symbol| {
                exports.iter().any(|(name, span)| {
                    symbol.name == *name
                        && symbol.selection_span.start >= span.start
                        && symbol.selection_span.end() <= span.end()
                })
            })
            .cloned()
            .collect();
        Some((loaded, members))
    }

    pub(super) fn imported_member_at(
        &self,
        document: &Document,
        offset: u32,
    ) -> Option<(LoadedSource, SemanticSymbol, Span)> {
        let (name, span) = identifier_at(&document.text, offset)?;
        let namespace = qualified_namespace(document.text.get(..span.start as usize)?)?;
        let (loaded, members) = self.imported_module(document, namespace)?;
        Some((
            loaded,
            members.into_iter().find(|symbol| symbol.name == name)?,
            span,
        ))
    }

    pub(super) fn references(&self, params: &Value, include_declaration: bool) -> Value {
        let Some((document, offset)) = self.document_and_offset(params) else {
            return json!([]);
        };
        if let Some((source, compilation)) = document.compilation()
            && let Some(source_analysis) = compilation.source(source)
            && let Some(symbol) = source_analysis
                .reference_at(offset)
                .map(|reference| reference.symbol)
                .or_else(|| source_analysis.symbol_at(offset).map(|symbol| symbol.id))
        {
            let mut locations = compilation
                .references_to(symbol)
                .filter_map(|(location, _)| {
                    analysis_location(compilation.source(location.source)?, location.span)
                })
                .collect::<Vec<_>>();
            if include_declaration
                && let Some(declaration) = compilation.symbol(symbol)
                && let Some(source) = compilation.source(symbol.source())
                && let Some(location) = analysis_location(source, declaration.selection_span)
            {
                locations.push(location);
            }
            return json!(locations);
        }
        if let Some((loaded, symbol, _)) = self.imported_member_at(document, offset) {
            let loader = self.loader();
            let aliases: HashSet<_> = document
                .current
                .imports()
                .iter()
                .filter_map(|import| {
                    let alias = import.alias.as_deref()?;
                    let candidate = loader.load(&document.path, &import.path).ok()?;
                    (candidate.identity == loaded.identity).then_some(alias)
                })
                .collect();
            let mut locations: Vec<_> = document
                .current
                .tokens()
                .iter()
                .filter(|token| {
                    token.kind == LexicalTokenKind::Identifier
                        && token.text.as_str() == symbol.name.as_str()
                        && document
                            .text
                            .get(..token.span.start as usize)
                            .and_then(qualified_namespace)
                            .is_some_and(|alias| aliases.contains(alias))
                })
                .map(|token| location(document, token.span))
                .collect();
            if include_declaration
                && let Some(declaration) = imported_location(&loaded, symbol.selection_span)
            {
                locations.push(declaration);
            }
            return json!(locations);
        }
        let Some(symbol) = document.analysis().symbol_at(offset) else {
            let Some((builtin, _)) = self
                .builtin_catalog_ready
                .then(|| builtin_at(document, offset))
                .flatten()
            else {
                return json!([]);
            };
            return json!(
                tokenize(&Source::new(
                    document.path.to_string_lossy(),
                    document.text.to_string(),
                ))
                .into_iter()
                .filter(|token| token.kind == LexicalTokenKind::Identifier)
                .filter_map(|token| {
                    builtin_at(document, token.span.start)
                        .filter(|(candidate, _)| candidate.entry.id == builtin.entry.id)
                        .map(|(_, span)| location(document, span))
                })
                .collect::<Vec<_>>()
            );
        };
        json!(
            document
                .analysis()
                .occurrences(symbol.id, include_declaration)
                .map(|span| location(document, span))
                .collect::<Vec<_>>()
        )
    }

    pub(super) fn rename(&self, params: &Value) -> Value {
        let Some(new_name) = params.get("newName").and_then(Value::as_str) else {
            return Value::Null;
        };
        if !valid_identifier(new_name) {
            return Value::Null;
        }
        let Some((document, offset)) = self.document_and_offset(params) else {
            return Value::Null;
        };
        // 跨模块重命名需要工作区级事务；这里不能把成员误绑定到当前文档的同名符号。
        if self.imported_member_at(document, offset).is_some() {
            return Value::Null;
        }
        let Some(analysis) = document.edit_analysis() else {
            return Value::Null;
        };
        let Some(symbol) = analysis.symbol_at(offset) else {
            return Value::Null;
        };
        let changes = HashMap::from([(
            document.uri.clone(),
            analysis
                .occurrences(symbol.id, true)
                .map(|span| json!({ "range": document.index.range(span), "newText": new_name }))
                .collect::<Vec<_>>(),
        )]);
        json!({ "changes": changes })
    }

    pub(super) fn document_symbols(&self, params: &Value) -> Value {
        let Some(supplied_uri) = params
            .get("textDocument")
            .and_then(|document| document.get("uri"))
            .and_then(Value::as_str)
        else {
            return json!([]);
        };
        let Some((uri, _)) = document_uri_and_path(supplied_uri) else {
            return json!([]);
        };
        let Some(document) = self.documents.get(&uri) else {
            return json!([]);
        };
        json!(
            document
                .analysis()
                .symbols()
                .iter()
                // 索引保存的是可见作用域而不是声明范围（参数的作用域不含参数名）。
                // 平面 SymbolInformation 与索引模型一致，不伪造 DocumentSymbol 的包围范围。
                .map(|symbol| json!({
                    "name": symbol.name,
                    "kind": symbol_kind(symbol.kind),
                    "location": location(document, symbol.selection_span)
                }))
                .collect::<Vec<_>>()
        )
    }

    pub(super) fn workspace_symbols(&self, params: &Value) -> Value {
        let query = params
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase();
        let mut result = Vec::new();
        for document in self.documents.values() {
            for symbol in document.analysis().symbols() {
                if query.is_empty() || symbol.name.to_lowercase().contains(&query) {
                    result.push(json!({
                        "name": symbol.name,
                        "kind": symbol_kind(symbol.kind),
                        "location": location(document, symbol.selection_span),
                        "containerName": document.path.file_name().and_then(|name| name.to_str())
                    }));
                }
            }
        }
        json!(result)
    }

    pub(super) fn semantic_tokens(&self, params: &Value) -> Value {
        let Some(supplied_uri) = params
            .get("textDocument")
            .and_then(|document| document.get("uri"))
            .and_then(Value::as_str)
        else {
            return json!({ "data": [] });
        };
        let Some((uri, _)) = document_uri_and_path(supplied_uri) else {
            return json!({ "data": [] });
        };
        let Some(document) = self.documents.get(&uri) else {
            return json!({ "data": [] });
        };
        let mut encoded = Vec::<u32>::new();
        let mut previous_line = 0_u32;
        let mut previous_character = 0_u32;
        for token in document.current.tokens() {
            let Some(start) = document.index.position(token.span.start as usize) else {
                continue;
            };
            let line = start.get("line").and_then(Value::as_u64).unwrap_or(0) as u32;
            let character = start.get("character").and_then(Value::as_u64).unwrap_or(0) as u32;
            let length = token.text.encode_utf16().count() as u32;
            let delta_line = line.saturating_sub(previous_line);
            let delta_character = if delta_line == 0 {
                character.saturating_sub(previous_character)
            } else {
                character
            };
            encoded.extend([
                delta_line,
                delta_character,
                length,
                lexical_token_kind(token.kind),
                0,
            ]);
            previous_line = line;
            previous_character = character;
        }
        json!({ "data": encoded })
    }

    /// 为当前打开文档返回一个覆盖完整缓冲区的格式化编辑。
    ///
    /// 格式化器要求完整有效的语法；编辑器仍在输入不完整结构时返回空编辑，避免用旧快照
    /// 或部分输出覆盖当前缓冲区。
    pub(super) fn formatting(&self, params: &Value) -> Value {
        let Some(supplied_uri) = params
            .get("textDocument")
            .and_then(|document| document.get("uri"))
            .and_then(Value::as_str)
        else {
            return json!([]);
        };
        let Some((uri, _)) = document_uri_and_path(supplied_uri) else {
            return json!([]);
        };
        let Some(document) = self.documents.get(&uri) else {
            return json!([]);
        };
        let name = document.path.to_string_lossy();
        let Ok(formatted) = format_named(name.as_ref(), &document.text) else {
            return json!([]);
        };
        if formatted == document.text.as_ref() {
            return json!([]);
        }
        let end = document
            .index
            .position(document.text.len())
            .unwrap_or_else(|| json!({ "line": 0, "character": 0 }));
        json!([{
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": end
            },
            "newText": formatted
        }])
    }
}

fn active_call(text: &str, offset: usize) -> Option<(usize, usize)> {
    use teal_compiler::LexicalTokenKind;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Delimiter {
        Paren,
        Bracket,
        Brace,
    }

    struct Frame {
        delimiter: Delimiter,
        open: usize,
        commas: usize,
        call_like: bool,
    }

    let source = teal_compiler::Source::new("<signature>", text);
    let mut frames = Vec::<Frame>::new();
    let mut previous: Option<(LexicalTokenKind, String)> = None;
    for token in teal_compiler::tokenize(&source) {
        let spelling = token.text.as_str();
        let open = token.span.start as usize;
        if open >= offset {
            break;
        }
        let delimiter = match spelling {
            "(" => Some(Delimiter::Paren),
            "[" => Some(Delimiter::Bracket),
            "{" => Some(Delimiter::Brace),
            _ => None,
        };
        if let Some(delimiter) = delimiter {
            let call_like = delimiter == Delimiter::Paren
                && previous.as_ref().is_some_and(|(kind, text)| {
                    (matches!(
                        kind,
                        LexicalTokenKind::Identifier | LexicalTokenKind::Keyword
                    ) && !matches!(text.as_str(), "if" | "for" | "while" | "match" | "catch"))
                        || text == ")"
                });
            frames.push(Frame {
                delimiter,
                open,
                commas: 0,
                call_like,
            });
        } else if let Some(closing) = match spelling {
            ")" => Some(Delimiter::Paren),
            "]" => Some(Delimiter::Bracket),
            "}" => Some(Delimiter::Brace),
            _ => None,
        } {
            if let Some(position) = frames.iter().rposition(|frame| frame.delimiter == closing) {
                frames.truncate(position);
            }
        } else if spelling == ","
            && let Some(frame) = frames.last_mut()
            && frame.delimiter == Delimiter::Paren
        {
            frame.commas += 1;
        }
        if token.kind != LexicalTokenKind::Comment {
            previous = Some((token.kind, spelling.to_owned()));
        }
    }
    frames
        .iter()
        .rev()
        .find(|frame| frame.call_like)
        .map(|frame| (frame.open, frame.commas))
}
