//! 语言服务请求、结果选择器、编辑与语义高亮的根应用集成。
//!
//! 请求捕获面板、共享缓冲区、修订和序号，响应与选择器确认都要验证原始上下文。
//! 协议支持不等于执行授权；资源编辑及服务器命令仍经过工作区检查或用户确认。

use super::*;
use teal_workbench_core::BufferId;
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub(crate) enum RequestKind {
    Complete,
    Hover,
    Definition,
    References,
    Rename(String),
    Format,
    CodeAction,
    Semantic,
}

impl RequestKind {
    fn label(&self) -> &'static str {
        match self {
            Self::Complete => "Completion",
            Self::Hover => "Hover",
            Self::Definition => "Definition",
            Self::References => "References",
            Self::Rename(_) => "Rename",
            Self::Format => "Format",
            Self::CodeAction => "Code actions",
            Self::Semantic => "Semantic tokens",
        }
    }

    fn key(&self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Hover => "hover",
            Self::Definition => "definition",
            Self::References => "references",
            Self::Rename(_) => "rename",
            Self::Format => "format",
            Self::CodeAction => "code-action",
            Self::Semantic => "semantic",
        }
    }

    fn capability(&self) -> &'static str {
        match self {
            Self::Complete => "completionProvider",
            Self::Hover => "hoverProvider",
            Self::Definition => "definitionProvider",
            Self::References => "referencesProvider",
            Self::Rename(_) => "renameProvider",
            Self::Format => "documentFormattingProvider",
            Self::CodeAction => "codeActionProvider",
            Self::Semantic => "semanticTokensProvider",
        }
    }
}

/// 一次请求的完整原始上下文；不能用响应到达时的焦点或光标替代。
#[derive(Clone)]
pub(crate) struct Request {
    observation: crate::runtime_bridge::Context,
    panel: PanelId,
    buffer: BufferId,
    revision: u64,
    /// 同类请求即使捕获同一修订，也通过序号区分先后。
    sequence: u64,
    action: RequestKind,
    uri: String,
    source: String,
    cursor: usize,
    client: Arc<LspClient>,
}

pub(crate) struct Response {
    request: Request,
    result: Result<Value, String>,
}

/// 延后由用户选择的响应，保留原请求以便确认时再次拒绝过期编辑。
pub(crate) struct Picker {
    request: Request,
    choices: Vec<Value>,
    resolved: bool,
}

#[derive(Default)]
pub(crate) struct State {
    sequence: u64,
    pending: HashMap<(BufferId, &'static str), (u64, oneshot::Sender<()>)>,
    deferred: std::collections::VecDeque<Response>,
    semantics: HashMap<BufferId, teal_lsp_client::SemanticTokenCache>,
}

impl App {
    pub(super) fn invalidate_lsp_buffer(&mut self, buffer: BufferId, resource_changed: bool) {
        if resource_changed {
            self.lsp_ui.semantics.remove(&buffer);
        }
        self.lsp_ui.pending.retain(|(id, _), _| *id != buffer);
        let ids = self
            .panels
            .iter()
            .filter_map(|(id, panel)| {
                panel
                    .as_editor()
                    .filter(|editor| editor.document().id() == buffer)
                    .map(|_| id)
            })
            .collect::<Vec<_>>();
        for id in ids {
            if let Some(editor) = self.panels.get_mut(id).and_then(AppPanel::as_editor_mut) {
                editor.set_semantic_captures(Vec::new());
                editor.set_diagnostic_captures(Vec::new());
            }
        }
    }

    pub(super) fn start_lsp_rename(&mut self) {
        let panel = self.windows.focused();
        if self
            .panels
            .get(panel)
            .and_then(AppPanel::as_editor)
            .is_none()
        {
            return;
        }
        self.pending_prompt = Some(PendingPrompt::LspRename(panel));
        self.cmdline = Some(Prompt::with_text(CmdlineMode::Prompt, String::new()));
    }

    pub(super) fn request_lsp(&mut self, action: RequestKind) {
        self.request_lsp_for(self.windows.focused(), action);
    }

    #[tracing::instrument(name = "teal.bench.task", skip_all, fields(operation.id = teal_workbench_services::observability::operation_id(), operation.result = tracing::field::Empty))]
    pub(super) fn request_lsp_for(&mut self, panel: PanelId, action: RequestKind) {
        let Some(buffer) = self
            .panels
            .get(panel)
            .and_then(AppPanel::as_editor)
            .map(|e| e.document().clone())
        else {
            return;
        };
        let Some(path) = buffer.path() else {
            return;
        };
        let Some(server) = self.lsp.for_path(&path) else {
            if !matches!(action, RequestKind::Semantic) {
                self.message(Severity::Warning, "lsp", "No matching server");
            }
            return;
        };
        if server
            .client
            .capabilities()
            .get(action.capability())
            .is_none_or(|v| v.is_null() || v == false)
        {
            if !matches!(action, RequestKind::Semantic) {
                self.message(
                    Severity::Info,
                    "lsp",
                    "The server does not support this operation",
                );
            }
            return;
        }
        let Ok(uri) = url::Url::from_file_path(path) else {
            return;
        };
        let source = buffer.to_string();
        let cursor = buffer
            .selections()
            .primary()
            .head
            .min(buffer.text().len_chars());
        let index = LineIndex::new(source.clone());
        let Some((line, character)) = index.position(buffer.text().char_to_byte(cursor)) else {
            return;
        };
        let mut params = json!({"textDocument":{"uri":uri.as_str()},"position":{"line":line,"character":character}});
        let method = match &action {
            RequestKind::Complete => {
                params["context"] = json!({"triggerKind":1});
                "textDocument/completion"
            }
            RequestKind::Hover => "textDocument/hover",
            RequestKind::Definition => "textDocument/definition",
            RequestKind::References => {
                params["context"] = json!({"includeDeclaration":true});
                "textDocument/references"
            }
            RequestKind::Rename(name) => {
                params["newName"] = json!(name);
                "textDocument/rename"
            }
            RequestKind::Format => {
                params = json!({"textDocument":{"uri":uri.as_str()},"options":{"tabSize":4,"insertSpaces":true}});
                "textDocument/formatting"
            }
            RequestKind::CodeAction => {
                params["range"] = json!({"start":{"line":line,"character":character},"end":{"line":line,"character":character}});
                let diagnostics = self
                    .diagnostics
                    .snapshot(Some(uri.as_str()))
                    .into_iter()
                    .filter(|d| d.revision.is_none_or(|revision| revision == buffer.revision()))
                    .map(|d| json!({"range":d.range_utf16,"severity":d.severity,"source":d.source,"message":d.message}))
                    .collect::<Vec<_>>();
                let anchor = buffer
                    .selections()
                    .primary()
                    .anchor
                    .min(buffer.text().len_chars());
                let (sl, sc) = index
                    .position(buffer.text().char_to_byte(anchor.min(cursor)))
                    .unwrap_or((line, character));
                let (el, ec) = index
                    .position(buffer.text().char_to_byte(anchor.max(cursor)))
                    .unwrap_or((line, character));
                params["range"] =
                    json!({"start":{"line":sl,"character":sc},"end":{"line":el,"character":ec}});
                params["context"] = json!({"diagnostics":diagnostics});
                "textDocument/codeAction"
            }
            RequestKind::Semantic => {
                params = json!({"textDocument":{"uri":uri.as_str()}});
                if server
                    .client
                    .capabilities()
                    .pointer("/semanticTokensProvider/full/delta")
                    .and_then(Value::as_bool)
                    == Some(true)
                    && let Some(id) = self
                        .lsp_ui
                        .semantics
                        .get(&buffer.id())
                        .and_then(teal_lsp_client::SemanticTokenCache::result_id)
                {
                    params["previousResultId"] = json!(id);
                    "textDocument/semanticTokens/full/delta"
                } else {
                    "textDocument/semanticTokens/full"
                }
            }
        };
        self.lsp_ui.sequence += 1;
        let sequence = self.lsp_ui.sequence;
        let (cancel, receiver) = oneshot::channel();
        // 同一缓冲区的同类请求由最新序号接管；取消旧等待不保证服务器停止处理。
        if let Some((_, previous)) = self
            .lsp_ui
            .pending
            .insert((buffer.id(), action.key()), (sequence, cancel))
        {
            let _ = previous.send(());
        }
        let request = Request {
            observation: crate::runtime_bridge::Context::current(),
            panel,
            buffer: buffer.id(),
            revision: buffer.revision(),
            sequence,
            action,
            uri: uri.to_string(),
            source: source.clone(),
            cursor,
            client: server.client.clone(),
        };
        let output = self.runtime_tx.clone();
        self.tasks.spawn(async move {
            let result = match server
                .synchronize(&request.uri, request.revision, &source)
                .await
            {
                Ok(()) => server
                    .client
                    .request_cancellable(method, params, receiver)
                    .await
                    .map_err(|e| e.to_string()),
                Err(error) => Err(error.to_string()),
            };
            teal_workbench_services::observability::outcome(&result);
            let _ = output
                .send(RuntimeEvent::LspResponse(Response { request, result }))
                .await;
        });
    }

    pub(super) fn pump_lsp_results(&mut self) {
        if self.pending_overlay.is_none()
            && let Some(response) = self.lsp_ui.deferred.pop_front()
        {
            self.handle_lsp_response(response);
        }
    }

    pub(super) fn handle_lsp_response(&mut self, response: Response) {
        let observation = response.request.observation.clone();
        observation.in_scope(|| self.apply_lsp_response(response));
    }

    fn apply_lsp_response(&mut self, response: Response) {
        if self.pending_overlay.is_some()
            && !matches!(response.request.action, RequestKind::Semantic)
        {
            if self.lsp_ui.deferred.len() >= 32 {
                self.lsp_ui.deferred.pop_front();
            }
            self.lsp_ui.deferred.push_back(response);
            return;
        }
        let Response { request, result } = response;
        if self
            .lsp_ui
            .pending
            .get(&(request.buffer, request.action.key()))
            .map(|p| p.0)
            != Some(request.sequence)
        {
            return;
        }
        self.lsp_ui
            .pending
            .remove(&(request.buffer, request.action.key()));
        if !self.request_is_current(&request) {
            return;
        }
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                if !error.contains("cancelled") {
                    self.message(Severity::Error, "lsp", error);
                }
                return;
            }
        };
        match request.action {
            RequestKind::Hover => {
                let content = value.get("contents").map(hover_text).unwrap_or_default();
                // hover 是被动浮窗：显示文档但不抢占面板与焦点。
                if content.trim().is_empty() {
                    self.hover.hide(&mut self.windows);
                } else {
                    self.hover.show(&mut self.windows, content);
                }
            }
            RequestKind::Rename(_) => {
                if !value.is_null()
                    && let Err(error) = self.apply_workspace_edit(&value)
                {
                    self.message(Severity::Error, "lsp", error);
                }
            }
            RequestKind::Format => {
                if value.is_null() {
                    return;
                }
                let edit = json!({"changes":{request.uri.clone():value}});
                if let Err(error) = self.apply_workspace_edit(&edit) {
                    self.message(Severity::Error, "lsp", error);
                }
            }
            RequestKind::Semantic => self.apply_semantic_response(&request, &value),
            _ => {
                let choices = if matches!(request.action, RequestKind::Complete) {
                    value
                        .as_array()
                        .or_else(|| value.get("items").and_then(Value::as_array))
                        .cloned()
                        .unwrap_or_default()
                } else if let Some(array) = value.as_array() {
                    array.clone()
                } else if value.is_null() {
                    Vec::new()
                } else {
                    vec![value]
                };
                if choices.is_empty() {
                    self.message(Severity::Info, "lsp", "no result");
                    return;
                }
                if matches!(request.action, RequestKind::Definition) && choices.len() == 1 {
                    self.open_lsp_location(&choices[0]);
                    return;
                }
                let items = choices
                    .iter()
                    .enumerate()
                    .map(|(i, v)| MenuItem::enabled(i.to_string(), choice_label(v)))
                    .collect();
                self.overlays.push(PickerOverlay::with_items(
                    format!("LSP {}", request.action.label()),
                    items,
                ));
                self.pending_overlay = Some(PendingOverlay::Lsp(Box::new(Picker {
                    request,
                    choices,
                    resolved: false,
                })));
            }
        }
    }

    fn request_is_current(&self, request: &Request) -> bool {
        self.panels
            .get(request.panel)
            .and_then(AppPanel::as_editor)
            .is_some_and(|e| {
                e.document().id() == request.buffer && e.document().revision() == request.revision
            })
    }

    #[tracing::instrument(name = "teal.bench.task", skip_all, fields(operation.id = teal_workbench_services::observability::operation_id(), operation.result = tracing::field::Empty))]
    pub(super) fn accept_lsp_choice(&mut self, picker: &Picker, index: usize) {
        if !self.request_is_current(&picker.request) {
            self.message(Severity::Warning, "lsp", "Results have expired");
            return;
        }
        let Some(value) = picker.choices.get(index) else {
            return;
        };
        let resolve = match picker.request.action {
            RequestKind::Complete
                if picker
                    .request
                    .client
                    .capabilities()
                    .pointer("/completionProvider/resolveProvider")
                    .and_then(Value::as_bool)
                    == Some(true) =>
            {
                Some("completionItem/resolve")
            }
            RequestKind::CodeAction
                if picker
                    .request
                    .client
                    .capabilities()
                    .pointer("/codeActionProvider/resolveProvider")
                    .and_then(Value::as_bool)
                    == Some(true) =>
            {
                Some("codeAction/resolve")
            }
            _ => None,
        };
        if !picker.resolved
            && value.get("data").is_some()
            && let Some(method) = resolve
        {
            let request = picker.request.clone();
            let value = value.clone();
            let output = self.runtime_tx.clone();
            self.tasks.spawn(async move {
                let result = request
                    .client
                    .request(method, value)
                    .await
                    .map_err(|e| e.to_string());
                teal_workbench_services::observability::outcome(&result);
                let _ = output
                    .send(RuntimeEvent::LspResolved { request, result })
                    .await;
            });
            return;
        }
        match picker.request.action {
            RequestKind::Complete => {
                match completion_transaction(&picker.request.source, picker.request.cursor, value) {
                    Ok((transaction, cursor)) => {
                        if let Some(buffer) = self
                            .panels
                            .get(picker.request.panel)
                            .and_then(AppPanel::as_editor)
                            .map(|editor| editor.document().clone())
                        {
                            if let Err(error) = buffer.apply(transaction) {
                                self.message(Severity::Error, "lsp", error.to_string());
                            } else {
                                buffer.set_selections(teal_workbench_core::SelectionSet::single(
                                    teal_workbench_core::Selection::cursor(cursor),
                                ));
                            }
                        }
                    }
                    Err(error) => self.message(Severity::Error, "lsp", error),
                }
                self.process_buffer_changes();
            }
            RequestKind::Definition | RequestKind::References => self.open_lsp_location(value),
            RequestKind::CodeAction => {
                let edit = value.get("edit").cloned();
                let command = if value.get("command").is_some_and(Value::is_object) {
                    value["command"].clone()
                } else {
                    value.clone()
                };
                if command.get("command").is_some_and(Value::is_string) {
                    self.overlays.push(ConfirmOverlay::new(
                        "LSP command",
                        format!("Do you want to execute {}?", command["command"]),
                    ));
                    self.pending_overlay = Some(PendingOverlay::Confirmation(
                        PendingConfirmation::LspCommand {
                            client: picker.request.client.clone(),
                            command,
                            edit,
                        },
                    ));
                } else if let Some(edit) = edit
                    && let Err(error) = self.apply_workspace_edit(&edit)
                {
                    self.message(Severity::Error, "lsp", error);
                }
            }
            _ => {}
        }
    }

    pub(super) fn finish_lsp_resolve(&mut self, request: Request, result: Result<Value, String>) {
        match result {
            Ok(value) => self.accept_lsp_choice(
                &Picker {
                    request,
                    choices: vec![value],
                    resolved: true,
                },
                0,
            ),
            Err(error) => self.message(Severity::Error, "lsp", error),
        }
    }

    fn open_lsp_location(&mut self, value: &Value) {
        let uri = value
            .get("uri")
            .or_else(|| value.get("targetUri"))
            .and_then(Value::as_str);
        let range = value
            .get("range")
            .or_else(|| value.get("targetSelectionRange"));
        let Some((uri, range)) = uri.zip(range) else {
            return;
        };
        let Ok(path) = url::Url::parse(uri)
            .ok()
            .and_then(|u| u.to_file_path().ok())
            .ok_or(())
        else {
            return;
        };
        self.queue_open(path, false, Some(range["start"].clone()));
    }

    fn apply_semantic_response(&mut self, request: &Request, value: &Value) {
        let cache = self.lsp_ui.semantics.entry(request.buffer).or_default();
        let result_id = value
            .get("resultId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(data) = value.get("data").and_then(Value::as_array) {
            let Some(data) = data
                .iter()
                .map(|v| v.as_u64().and_then(|n| u32::try_from(n).ok()))
                .collect::<Option<Vec<_>>>()
            else {
                return;
            };
            if data.len() % 5 != 0 {
                return;
            }
            cache.replace(data, result_id);
        } else if let Some(edits) = value.get("edits")
            && !cache.apply_delta(edits, result_id)
        {
            self.lsp_ui.semantics.remove(&request.buffer);
            // 无效增量可能来自失步的服务器缓存；清除 resultId 后重新请求完整词元。
            self.request_lsp_for(request.panel, RequestKind::Semantic);
            return;
        }
        let legend = request
            .client
            .capabilities()
            .pointer("/semanticTokensProvider/legend/tokenTypes")
            .and_then(Value::as_array);
        let index = LineIndex::new(request.source.clone());
        let captures = cache
            .tokens()
            .into_iter()
            .filter_map(|token| {
                let start = index.offset(u64::from(token.line), u64::from(token.start))?;
                let end = index.offset(
                    u64::from(token.line),
                    u64::from(token.start.checked_add(token.length)?),
                )?;
                Some(teal_workbench_core::syntax::Capture {
                    name: legend?.get(token.token_type as usize)?.as_str()?.to_owned(),
                    range: start..end,
                    priority: 100,
                })
            })
            .collect();
        if let Some(editor) = self
            .panels
            .get_mut(request.panel)
            .and_then(AppPanel::as_editor_mut)
        {
            editor.set_semantic_captures(captures);
        }
    }
}

fn choice_label(value: &Value) -> String {
    value
        .get("label")
        .or_else(|| value.get("title"))
        .and_then(Value::as_str)
        .map_or_else(
            || {
                let uri = value
                    .get("uri")
                    .or_else(|| value.get("targetUri"))
                    .and_then(Value::as_str)
                    .unwrap_or("location");
                let line = value
                    .pointer("/range/start/line")
                    .or_else(|| value.pointer("/targetSelectionRange/start/line"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    + 1;
                format!("{uri}:{line}")
            },
            str::to_owned,
        )
}

fn hover_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(values) => values
            .iter()
            .map(hover_text)
            .collect::<Vec<_>>()
            .join("\n\n"),
        Value::Object(o) => o.get("value").map(hover_text).unwrap_or_default(),
        _ => String::new(),
    }
}

fn position_offset(index: &LineIndex, value: &Value) -> Option<usize> {
    index.offset(
        value.get("line")?.as_u64()?,
        value.get("character")?.as_u64()?,
    )
}

fn edit_range(index: &LineIndex, value: &Value) -> Result<std::ops::Range<usize>, String> {
    let start = position_offset(index, &value["start"])
        .and_then(|b| index.char_offset(b))
        .ok_or("Invalid edit start position")?;
    let end = position_offset(index, &value["end"])
        .and_then(|b| index.char_offset(b))
        .ok_or("Invalid edit end position")?;
    Ok(start..end)
}

fn completion_transaction(
    source: &str,
    cursor: usize,
    item: &Value,
) -> Result<(Transaction, usize), String> {
    let index = LineIndex::new(source);
    let edit = item.get("textEdit");
    let range = if let Some(edit) = edit {
        edit_range(
            &index,
            edit.get("range")
                .or_else(|| edit.get("replace"))
                .ok_or("Missing completion range")?,
        )?
    } else {
        cursor..cursor
    };
    let text = edit
        .and_then(|e| e.get("newText"))
        .or_else(|| item.get("insertText"))
        .or_else(|| item.get("label"))
        .and_then(Value::as_str)
        .ok_or("Completion text is missing")?;
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let (text, tab) = if item.get("insertTextFormat").and_then(Value::as_u64) == Some(2) {
        snippet(&text)
    } else {
        (text, None)
    };
    let mut final_cursor = range.start + tab.unwrap_or_else(|| text.chars().count());
    let mut edits = vec![TextEdit {
        range: range.clone(),
        insert: text,
    }];
    if let Some(additional) = item.get("additionalTextEdits").and_then(Value::as_array) {
        for edit in additional {
            let edit_range = edit_range(&index, &edit["range"])?;
            let insert = edit["newText"]
                .as_str()
                .ok_or("Missing additional text")?
                .replace("\r\n", "\n")
                .replace('\r', "\n");
            if edit_range.end <= range.start {
                final_cursor = final_cursor
                    .saturating_sub(edit_range.len())
                    .saturating_add(insert.chars().count());
            }
            edits.push(TextEdit {
                range: edit_range,
                insert,
            });
        }
    }
    Ok((
        Transaction::new(edits).map_err(|e| e.to_string())?,
        final_cursor,
    ))
}

fn snippet(input: &str) -> (String, Option<usize>) {
    let chars = input.chars().collect::<Vec<_>>();
    let mut i = 0;
    let mut out = String::new();
    let mut first = None;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            i += 1;
            out.push(chars[i]);
            i += 1;
            continue;
        }
        if chars[i] == '$' && i + 1 < chars.len() {
            let start = i;
            i += 1;
            let braced = chars[i] == '{';
            if braced {
                i += 1;
            }
            let number_start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            if i > number_start {
                let number = chars[number_start..i]
                    .iter()
                    .collect::<String>()
                    .parse::<u32>()
                    .unwrap_or(0);
                if number != 0 && first.is_none() {
                    first = Some(out.chars().count());
                }
                if braced {
                    if i < chars.len() && chars[i] == ':' {
                        i += 1;
                        while i < chars.len() && chars[i] != '}' {
                            out.push(chars[i]);
                            i += 1;
                        }
                    } else if i < chars.len() && chars[i] == '|' {
                        i += 1;
                        while i < chars.len() && chars[i] != ',' && chars[i] != '|' {
                            out.push(chars[i]);
                            i += 1;
                        }
                        while i < chars.len() && chars[i] != '}' {
                            i += 1;
                        }
                    }
                    if i < chars.len() && chars[i] == '}' {
                        i += 1;
                    }
                }
                continue;
            }
            i = start;
        }
        out.push(chars[i]);
        i += 1;
    }
    (out, first)
}

impl App {
    pub(super) fn start_lsp(&mut self, preset: &str) {
        let (language, config) = match preset {
            "rust" | "rust-analyzer" => {
                ("rust", ServerConfig::rust_analyzer(self.workspace.clone()))
            }
            "teal" => {
                let mut config = ServerConfig::teal(self.workspace.clone());
                let executable = if cfg!(windows) {
                    "teal-lsp.exe"
                } else {
                    "teal-lsp"
                };
                let sibling = std::env::current_exe()
                    .ok()
                    .and_then(|path| path.parent().map(|parent| parent.join(executable)));
                let development = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .and_then(Path::parent)
                    .map(|root| root.join("target/debug").join(executable));
                if let Some(local) = sibling
                    .into_iter()
                    .chain(development)
                    .find(|path| path.exists())
                {
                    config.program = local.to_string_lossy().into_owned();
                }
                ("teal", config)
            }
            _ => {
                self.message(Severity::Error, "lsp", "Please use :lsp rust or :lsp teal");
                return;
            }
        };
        let language = language.to_owned();
        let output = self.runtime_tx.clone();
        self.tasks.spawn(async move {
            let result = LspClient::spawn(config).await.map_err(|e| e.to_string());
            let _ = output
                .send(RuntimeEvent::LspStarted { language, result })
                .await;
        });
    }

    pub(super) async fn attach_lsp(&mut self, language: String, result: Result<LspClient, String>) {
        match result {
            Ok(client) => {
                let client = Arc::new(client);
                let mut events = client.subscribe();
                let output = self.lsp_events_tx.clone();
                let event_client = client.clone();
                self.tasks.spawn(async move {
                    while let Ok(event) = events.recv().await {
                        if output.send((event_client.clone(), event)).await.is_err() {
                            break;
                        }
                    }
                });
                self.lsp.insert(language.clone(), client);
                let ids = self.panels.iter().map(|(id, _)| id).collect::<Vec<_>>();
                for id in ids {
                    self.notify_lsp_open(id).await;
                }
                self.message(Severity::Info, "lsp", format!("Started {language}"));
                self.fire_hook("lsp.started", serde_json::json!({"language":language}));
            }
            Err(error) => self.message(Severity::Error, "lsp", error),
        }
    }

    pub(super) async fn notify_lsp_open(&self, panel: PanelId) {
        let Some(buffer) = self
            .panels
            .get(panel)
            .and_then(AppPanel::as_editor)
            .map(crate::workbench::ui::EditorPanel::document)
        else {
            return;
        };
        let Some(path) = buffer.path() else {
            return;
        };
        let Some(server) = self.lsp.for_path(&path) else {
            return;
        };
        let Ok(uri) = url::Url::from_file_path(path) else {
            return;
        };
        let _ = server
            .open(uri.as_str(), buffer.revision(), &buffer.to_string())
            .await;
    }

    pub(super) async fn handle_lsp_event(&mut self, client: Arc<LspClient>, event: LspEvent) {
        match event {
            LspEvent::Stderr(line) => self.message(Severity::Warning, "lsp", line),
            LspEvent::ServerExited => self.message(Severity::Warning, "lsp", "Server has exited"),
            LspEvent::Notification { method, params } => {
                if method == "textDocument/publishDiagnostics" {
                    self.apply_diagnostics(&params);
                } else if (method == "window/showMessage" || method == "window/logMessage")
                    && let Some(message) = params.get("message").and_then(serde_json::Value::as_str)
                {
                    self.message(Severity::Info, "lsp", message);
                }
            }
            LspEvent::Request { id, method, params } => {
                if method == "workspace/applyEdit" {
                    if let Some(edit) = params.get("edit") {
                        self.queue_work(
                            mutation::Work::Lsp(edit.clone()),
                            None,
                            None,
                            Some((client, id)),
                        );
                    } else {
                        let _ = client
                            .respond(
                                id,
                                serde_json::json!({"applied":false,"failureReason":"missing edit"}),
                            )
                            .await;
                    }
                    return;
                }
                let result = match method.as_str() {
                    "workspace/configuration" => {
                        let count = params
                            .get("items")
                            .and_then(serde_json::Value::as_array)
                            .map_or(0, Vec::len);
                        serde_json::Value::Array(vec![serde_json::Value::Null; count])
                    }
                    "client/registerCapability"
                    | "client/unregisterCapability"
                    | "window/workDoneProgress/create" => serde_json::Value::Null,
                    "workspace/workspaceFolders" => {
                        let uri = url::Url::from_directory_path(&self.workspace)
                            .ok()
                            .map(|uri| uri.to_string());
                        serde_json::json!([{"uri":uri,"name":self.workspace.file_name().and_then(|name| name.to_str()).unwrap_or("workspace")}])
                    }
                    "window/showMessageRequest" => params
                        .get("actions")
                        .and_then(serde_json::Value::as_array)
                        .and_then(|actions| actions.first())
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    "workspace/applyEdit" => {
                        let applied = params
                            .get("edit")
                            .cloned()
                            .map(|edit| self.apply_workspace_edit(&edit))
                            .transpose()
                            .is_ok();
                        serde_json::json!({"applied":applied})
                    }
                    _ => {
                        let _ = client
                            .respond_error(id, -32601, "Teal Bench does not support this method")
                            .await;
                        return;
                    }
                };
                let _ = client.respond(id, result).await;
            }
        }
    }

    pub(super) fn apply_diagnostics(&mut self, params: &serde_json::Value) {
        let Some(uri) = params.get("uri").and_then(serde_json::Value::as_str) else {
            return;
        };
        let buffer = url::Url::parse(uri)
            .ok()
            .and_then(|url| url.to_file_path().ok())
            .and_then(|path| self.buffers.find(&path));
        let source = buffer.as_ref().map(ToString::to_string);
        if !self.diagnostics.update(
            params,
            source.as_deref(),
            buffer
                .as_ref()
                .map(teal_workbench_core::BufferHandle::revision),
        ) {
            return;
        }
        let captures = self
            .diagnostics
            .snapshot(Some(uri))
            .into_iter()
            .filter_map(|record| {
                Some(teal_workbench_core::syntax::Capture {
                    name: if record.severity == 1 {
                        "diagnostic.error"
                    } else {
                        "diagnostic.warning"
                    }
                    .to_owned(),
                    range: record.range_utf8?,
                    priority: 200,
                })
            })
            .collect::<Vec<_>>();
        let ids = self
            .panels
            .iter()
            .filter_map(|(id, p)| {
                p.as_editor()
                    .filter(|editor| {
                        buffer
                            .as_ref()
                            .is_some_and(|b| b.id() == editor.document().id())
                    })
                    .map(|_| id)
            })
            .collect::<Vec<_>>();
        for id in ids {
            if let Some(editor) = self.panels.get_mut(id).and_then(AppPanel::as_editor_mut) {
                editor.set_diagnostic_captures(captures.clone());
            }
        }
        self.fire_hook("diagnostics.changed", serde_json::json!({"uri":uri}));
    }

    pub(super) fn apply_workspace_edit(&mut self, edit: &serde_json::Value) -> Result<(), String> {
        self.queue_workspace_edit(edit, None)?;
        Ok(())
    }

    pub(super) fn queue_workspace_edit(
        &mut self,
        edit: &serde_json::Value,
        response: Option<oneshot::Sender<Result<(), String>>>,
    ) -> Result<(), String> {
        if !edit.is_object() {
            return Err("Workspace editing is invalid".into());
        }
        if self.io.is_full() {
            return Err("File system queue is full".into());
        }
        self.queue_work(mutation::Work::Lsp(edit.clone()), None, response, None);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completion_uses_utf16_and_first_snippet_stop() {
        let source = "\u{4e16}😀";
        let item = json!({"label":"call","insertTextFormat":2,"textEdit":{"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":3}},"newText":"call(${1:value})$0"}});
        let (transaction, cursor) = completion_transaction(source, 2, &item).unwrap();
        let mut doc = teal_workbench_core::Document::from_text(source);
        doc.apply(transaction).unwrap();
        assert_eq!(doc.to_string(), "\u{4e16}call(value)");
        assert_eq!(cursor, 6);
    }
}
