//! 行内补全、进程输出和 Markdown 的后台任务。
//!
//! 任务输入使用捕获的文档快照，完成结果携带原缓冲区身份和修订；界面线程只安装仍适用的结果。

use super::{
    App, AppPanel, SplitAxis, MarkdownDocument, PanelId, PanelKind, PathBuf, RuntimeEvent,
    Severity, TextPanel, Transaction, run_command,
};

pub(crate) struct InlineResult {
    pub(crate) panel: PanelId,
    pub(crate) buffer: teal_workbench_core::BufferId,
    pub(crate) revision: u64,
    pub(crate) head: usize,
    pub(crate) result: Result<String, String>,
}

/// 认证预检期间固定的行内补全请求。
pub(crate) struct PendingInlineCompletion {
    panel: PanelId,
    buffer: teal_workbench_core::BufferId,
    revision: u64,
    head: usize,
    prefix: String,
    suffix: String,
    acceptance: String,
}

impl App {
    pub(super) fn inline_complete(&mut self, acceptance: &str) {
        if let Err(error) = self.connectivity.require(self.active_model_capability()) {
            self.message(Severity::Error, "agent", error.to_string());
            return;
        }
        let panel = self.windows.focused();
        let Some(buffer) = self
            .panels
            .get(panel)
            .and_then(AppPanel::as_editor)
            .map(|e| e.document().clone())
        else {
            return;
        };

        // 保留请求的缓冲区标识、修订和光标，供补全结果检查使用。
        let head = buffer
            .selections()
            .primary()
            .head
            .min(buffer.text().len_chars());
        let byte = buffer.text().char_to_byte(head);
        let source = buffer.to_string();
        let (prefix, suffix) = (source[..byte].to_owned(), source[byte..].to_owned());
        let (id, revision) = (buffer.id(), buffer.revision());
        let pending = PendingInlineCompletion {
            panel,
            buffer: id,
            revision,
            head,
            prefix,
            suffix,
            acceptance: acceptance.to_owned(),
        };
        let Some(completer) = self.inline_completion.clone() else {
            self.pending_inline_completion = Some(pending);
            self.prepare_active_agent(None);
            return;
        };
        self.launch_inline_completion(pending, completer);
    }

    pub(super) fn launch_inline_completion(
        &mut self,
        pending: PendingInlineCompletion,
        completer: teal_agent::InlineCompletion,
    ) {
        let output = self.runtime_tx.clone();

        self.tasks.spawn(async move {
            let result = completer
                .complete(
                    &pending.prefix,
                    &pending.suffix,
                    teal_model::Cancellation::new(),
                )
                .await
                .map(|completion| match pending.acceptance.as_str() {
                    "word" => completion
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect(),
                    "line" => completion.lines().next().unwrap_or_default().to_owned(),
                    _ => completion,
                })
                .map_err(|e| e.to_string());
            let _ = output
                .send(RuntimeEvent::InlineCompleted(InlineResult {
                    panel: pending.panel,
                    buffer: pending.buffer,
                    revision: pending.revision,
                    head: pending.head,
                    result,
                }))
                .await;
        });
    }

    pub(super) fn finish_inline(&mut self, result: InlineResult) {
        let Some(buffer) = self
            .panels
            .get(result.panel)
            .and_then(AppPanel::as_editor)
            .map(|e| e.document().clone())
        else {
            return;
        };

        if buffer.id() != result.buffer
            || buffer.revision() != result.revision
            || buffer.selections().primary().head != result.head
        {
            return;
        }

        match result.result {
            Ok(completion) if !completion.is_empty() => {
                let count = completion.chars().count();
                if buffer
                    .apply(Transaction::insert(result.head, completion))
                    .is_ok()
                {
                    buffer.set_selections(teal_workbench_core::SelectionSet::single(
                        teal_workbench_core::Selection::cursor(result.head + count),
                    ));
                }
                self.process_buffer_changes();
            }
            Err(error) => self.message(Severity::Error, "completion", error),
            Ok(_) => {}
        }
    }

    pub(super) fn run_process(&mut self, command: &str) {
        let (command, root) = (command.to_owned(), self.workspace.clone());
        let output = self.runtime_tx.clone();
        self.tasks.spawn(async move {
            let result = run_command(&command, &root)
                .await
                .map(|result| {
                    TextPanel::plain(
                        PanelKind::Output,
                        format!("Process ({})", result.status),
                        result.output,
                    )
                })
                .map_err(|e| e.to_string());
            let _ = output
                .send(RuntimeEvent::TextLoaded {
                    result,
                    axis: SplitAxis::Rows,
                })
                .await;
        });
    }

    pub(super) fn preview_markdown(&mut self, path: Option<PathBuf>) {
        let path = path.map(|path| {
            if path.is_absolute() {
                path
            } else {
                self.workspace.join(path)
            }
        });
        let snapshot = self
            .panels
            .get(self.windows.focused())
            .and_then(AppPanel::as_editor)
            .map(|e| e.document().to_string());
        let output = self.runtime_tx.clone();

        // 在界面线程获取未保存文本，再在后台解析拥有所有权的快照。
        self.tasks.spawn(async move {
            let result = crate::runtime_bridge::spawn_blocking(move || {
                let source = if let Some(path) = path {
                    std::fs::read_to_string(path).map_err(|e| e.to_string())?
                } else {
                    snapshot.ok_or("Focused panel is not an editor")?
                };
                Ok(TextPanel::markdown(
                    "Markdown",
                    &MarkdownDocument::parse(&source),
                ))
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(std::convert::identity);
            let _ = output
                .send(RuntimeEvent::TextLoaded {
                    result,
                    axis: SplitAxis::Columns,
                })
                .await;
        });
    }
}
