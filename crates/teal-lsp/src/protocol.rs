//! LSP 消息边界、请求分发与诊断发布。

use super::*;

/// 运行 Teal 语言服务器, 超过标准输入和输出。
///
/// # Errors
/// 返回错误构造或封闭输出流的 I/O 错误。
pub async fn run_stdio() -> io::Result<()> {
    let mut input = tokio::io::stdin();
    let mut output = tokio::io::stdout();
    run(&mut input, &mut output).await
}

/// 在任意的非同步字节流上运行一个 LSP 连接。
///
/// 这个切入点有助于嵌入器和协议一级的测试。
///
/// # Errors
/// 返回无效设置或运输失败的 I/O 错误。
#[tracing::instrument(name = "teal.lsp.connection", skip_all)]
pub async fn run<R, W>(input: &mut R, output: &mut W) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut state = ServerState::default();
    let mut diagnostic_tracker = DiagnosticTracker::default();
    let mut request_sequence = 0i64;
    while let Some(message) = read_message(input).await? {
        request_sequence = request_sequence.saturating_add(1);
        tracing::debug!(event.name = "started", request.id = request_sequence);
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        if let Some(id) = message.get("id").cloned() {
            let result = match method {
                "initialize" => {
                    if params
                        .get("initializationOptions")
                        .and_then(|options| options.get("entryPoints"))
                        .is_some()
                    {
                        write_message(output, &json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32602,"message":"entryPoints was removed; use entryFiles and fn main() or task main()"}})).await?;
                        continue;
                    }
                    state.initialize(&params);
                    initialize_result()
                }
                "shutdown" => {
                    state.shutdown = true;
                    Value::Null
                }
                "textDocument/completion" => state.completion(&params),
                "textDocument/hover" => state.hover(&params),
                "textDocument/signatureHelp" => state.signature_help(&params),
                "textDocument/definition" => state.definition(&params),
                "textDocument/references" => state.references(
                    &params,
                    params
                        .get("context")
                        .and_then(|context| context.get("includeDeclaration"))
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                ),
                "textDocument/prepareRename" => {
                    let Some((document, offset)) = state.document_and_offset(&params) else {
                        write_message(output, &response(id, Value::Null)).await?;
                        continue;
                    };
                    document
                        .edit_analysis()
                        .and_then(|analysis| analysis.symbol_at(offset))
                        .map(|symbol| document.index.range(symbol.selection_span))
                        .unwrap_or(Value::Null)
                }
                "textDocument/rename" => state.rename(&params),
                "textDocument/documentSymbol" => state.document_symbols(&params),
                "workspace/symbol" => state.workspace_symbols(&params),
                "textDocument/semanticTokens/full" => state.semantic_tokens(&params),
                "textDocument/formatting" => state.formatting(&params),
                _ => Value::Null,
            };
            write_message(output, &response(id, result)).await?;
        } else {
            match method {
                "textDocument/didOpen" => {
                    if let Some(uri) = state.open(&params) {
                        let report = state.refresh_diagnostics(&uri, &mut diagnostic_tracker);
                        publish_diagnostic_report(output, report).await?;
                    }
                }
                "textDocument/didChange" => {
                    if let Some(uri) = state.change(&params) {
                        let report = state.refresh_diagnostics(&uri, &mut diagnostic_tracker);
                        publish_diagnostic_report(output, report).await?;
                    }
                }
                "textDocument/didClose" => {
                    if let Some(uri) = params
                        .get("textDocument")
                        .and_then(|document| document.get("uri"))
                        .and_then(Value::as_str)
                        && let Some((uri, _)) = state.close(uri)
                    {
                        let report = state.refresh_diagnostics(&uri, &mut diagnostic_tracker);
                        publish_diagnostic_report(output, report).await?;
                    }
                }
                "exit" => break,
                _ => {}
            }
        }
        tracing::debug!(event.name = "completed", request.id = request_sequence);
    }
    Ok(())
}

pub(super) fn initialize_result() -> Value {
    json!({
        "capabilities": {
            "positionEncoding": "utf-16",
            "textDocumentSync": { "openClose": true, "change": 2 },
            "completionProvider": { "resolveProvider": false, "triggerCharacters": [".", ":"] },
            "hoverProvider": true,
            "signatureHelpProvider": { "triggerCharacters": ["(", ","] },
            "definitionProvider": true,
            "referencesProvider": true,
            "renameProvider": { "prepareProvider": true },
            "documentSymbolProvider": true,
            "workspaceSymbolProvider": true,
            "documentFormattingProvider": true,
            "semanticTokensProvider": {
                "full": true,
                "legend": {
                    "tokenTypes": ["namespace", "type", "class", "enum", "function", "variable", "parameter", "property", "enumMember", "keyword", "string", "number", "comment", "operator"],
                    "tokenModifiers": []
                }
            }
        },
        "serverInfo": { "name": "teal-lsp", "version": env!("CARGO_PKG_VERSION") }
    })
}

pub(super) async fn publish_diagnostics<W: AsyncWrite + Unpin>(
    output: &mut W,
    uri: &str,
    diagnostics: Vec<Value>,
) -> io::Result<()> {
    write_message(
        output,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": { "uri": uri, "diagnostics": diagnostics }
        }),
    )
    .await
}

/// 发布诊断跟踪器已经聚合并补齐清空项的全部 URI。
pub(super) async fn publish_diagnostic_report<W: AsyncWrite + Unpin>(
    output: &mut W,
    report: DiagnosticReport,
) -> io::Result<()> {
    for (uri, diagnostics) in report.publications {
        publish_diagnostics(output, &uri, diagnostics).await?;
    }
    Ok(())
}

pub(super) fn response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub(super) async fn read_message<R: AsyncRead + Unpin>(input: &mut R) -> io::Result<Option<Value>> {
    let mut header = Vec::new();
    let mut byte = [0_u8; 1];
    while !header.ends_with(b"\r\n\r\n") {
        let count = input.read(&mut byte).await?;
        if count == 0 {
            return if header.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete LSP header",
                ))
            };
        }
        header.push(byte[0]);
        if header.len() > 16 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "LSP header is too large",
            ));
        }
    }
    let header_text = std::str::from_utf8(&header)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "LSP header is not UTF-8"))?;
    let length = header_text
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("Content-Length"))
        })
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    if length > 16 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "LSP message is too large",
        ));
    }
    let mut body = vec![0; length];
    input.read_exact(&mut body).await?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub(super) async fn write_message<W: AsyncWrite + Unpin>(
    output: &mut W,
    value: &Value,
) -> io::Result<()> {
    let body = serde_json::to_vec(value).map_err(io::Error::other)?;
    output
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await?;
    output.write_all(&body).await?;
    output.flush().await
}
