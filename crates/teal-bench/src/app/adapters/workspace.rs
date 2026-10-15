//! 文件浏览器操作、工作区搜索及浏览器请求的界面接线。
//!
//! 路径与操作上下文在用户触发时确定，后台返回后按目标面板和请求代次安装结果。
//! 永久删除、移动和外部动作保留各自确认路径。

use super::{
    App, AppPanel, Arc, BrowserPanel, CmdlineMode, ConfirmOverlay, ExplorerPanel, PanelId, PathBuf,
    PendingConfirmation, PendingOverlay, PendingPrompt, Prompt, RuntimeEvent, SearchService,
    Severity, SplitAxis, TextBrowser, TextPanel, WorkspaceIntent, lsp, mutation,
};

impl App {
    pub(crate) fn search_buffer(&mut self, pattern: &str) {
        let focused = self.windows.focused();
        let source = self
            .panels
            .get(focused)
            .and_then(|panel| panel.as_editor())
            .map(|editor| editor.document().to_string());
        let Some(source) = source else {
            self.message(
                Severity::Warning,
                "search",
                "Focused panel is not an editor",
            );
            return;
        };
        match SearchService::buffer(&source, pattern, true) {
            Ok(matches) => {
                // 命中同时高亮在来源编辑器中：按行首字节偏移累加匹配列得到字节区间。
                if let Some(editor) = self
                    .panels
                    .get_mut(focused)
                    .and_then(AppPanel::as_editor_mut)
                {
                    let captures =
                        search_captures(editor.document().text().to_string().as_str(), &matches);
                    editor.set_search_captures(captures);
                }
                let panel = TextPanel::search(pattern, &matches);
                self.add_panel(panel, SplitAxis::Rows);
            }
            Err(error) => self.message(Severity::Error, "search", error.to_string()),
        }
    }

    pub(crate) fn explorer_operation(
        &mut self,
        operation: impl FnOnce(
            &mut ExplorerPanel,
        )
            -> Result<WorkspaceIntent, teal_workbench_services::ExplorerError>,
    ) {
        self.explorer_operation_for(self.windows.focused(), operation);
    }

    pub(crate) fn explorer_operation_for(
        &mut self,
        panel: PanelId,
        operation: impl FnOnce(
            &mut ExplorerPanel,
        )
            -> Result<WorkspaceIntent, teal_workbench_services::ExplorerError>,
    ) {
        if self.io.panel_busy(panel) {
            self.message(
                Severity::Warning,
                "workspace",
                "File operations are still running",
            );
            return;
        }
        let result = self
            .panels
            .get_mut(panel)
            .and_then(|panel| panel.as_explorer_mut())
            .map(operation);
        match result {
            Some(Ok(intent)) => {
                self.queue_work(
                    mutation::Work::Resource(intent, false),
                    Some(panel),
                    None,
                    None,
                );
            }
            Some(Err(error)) => self.message(Severity::Error, "explorer", error.to_string()),
            None => self.message(
                Severity::Warning,
                "explorer",
                "Focus panel is not a file browser",
            ),
        }
    }

    pub(crate) fn start_explorer_prompt(&mut self, panel: PanelId, prompt: PendingPrompt) {
        let initial = self
            .panels
            .get(panel)
            .and_then(|target| target.as_explorer())
            .and_then(|explorer| match prompt {
                PendingPrompt::LspRename(_) => None,
                PendingPrompt::Rename(_) => explorer.selected_name(),
                PendingPrompt::CopyTo(_) | PendingPrompt::MoveTo(_) => Some(
                    explorer
                        .operation_directory()
                        .to_string_lossy()
                        .into_owned(),
                ),
            });
        let Some(initial) = initial else {
            self.message(Severity::Warning, "explorer", "No file selected");
            return;
        };
        self.pending_prompt = Some(prompt);
        self.cmdline = Some(Prompt::with_text(CmdlineMode::Prompt, initial));
    }

    pub(crate) fn finish_prompt(&mut self, text: &str) {
        if let Some(selection) = self.pending_model_profile_name.take() {
            self.finish_model_profile_name(text, selection);
            return;
        }
        let Some(prompt) = self.pending_prompt.take() else {
            return;
        };
        if text.trim().is_empty() {
            self.message(Severity::Warning, "explorer", "Input value cannot be empty");
            return;
        }
        match prompt {
            PendingPrompt::LspRename(panel) => {
                self.request_lsp_for(panel, lsp::RequestKind::Rename(text.to_owned()));
            }
            PendingPrompt::Rename(panel) => {
                self.explorer_operation_for(panel, |explorer| explorer.rename_selected(text));
            }
            PendingPrompt::CopyTo(panel) => {
                let destination = self.explorer_destination(panel, text);
                if let Some(destination) = destination {
                    self.explorer_operation_for(panel, |explorer| {
                        explorer.copy_selected(&destination)
                    });
                }
            }
            PendingPrompt::MoveTo(panel) => {
                let Some(destination) = self.explorer_destination(panel, text) else {
                    return;
                };
                let paths = self
                    .panels
                    .get(panel)
                    .and_then(|target| target.as_explorer())
                    .map(ExplorerPanel::selected_paths)
                    .unwrap_or_default();
                self.overlays.push(ConfirmOverlay::new(
                    "Move files",
                    format!(
                        "Move {} entries to {}?",
                        paths.len(),
                        destination.to_string_lossy()
                    ),
                ));
                self.pending_overlay = Some(PendingOverlay::Confirmation(
                    PendingConfirmation::ExplorerMove { panel, destination },
                ));
            }
        }
    }

    pub(crate) fn explorer_destination(&self, panel: PanelId, text: &str) -> Option<PathBuf> {
        let destination = PathBuf::from(text);
        if destination.is_absolute() {
            return Some(destination);
        }
        self.panels
            .get(panel)
            .and_then(|target| target.as_explorer())
            .map(|explorer| explorer.operation_directory().join(destination))
    }

    pub(crate) fn confirm_explorer_action(&mut self, panel: PanelId, permanent: bool) {
        let paths = self
            .panels
            .get(panel)
            .and_then(|target| target.as_explorer())
            .map(ExplorerPanel::selected_paths)
            .unwrap_or_default();
        if paths.is_empty() {
            self.message(Severity::Warning, "explorer", "No file selected");
            return;
        }
        let first = paths[0].to_string_lossy();
        let (title, message, confirmation) = if permanent {
            (
                "Delete permanently",
                format!(
                    "Permanently delete {} entries, starting with {first}?",
                    paths.len()
                ),
                PendingConfirmation::ExplorerDelete(panel),
            )
        } else {
            (
                "Move to recycle bin",
                format!(
                    "Move {} items to the Trash, starting with {first}?",
                    paths.len()
                ),
                PendingConfirmation::ExplorerTrash(panel),
            )
        };
        self.overlays.push(ConfirmOverlay::new(title, message));
        self.pending_overlay = Some(PendingOverlay::Confirmation(confirmation));
    }

    pub(crate) fn grep(&mut self, pattern: &str) {
        match SearchService::workspace(
            &self.workspace,
            pattern,
            false,
            1000,
            &Arc::new(std::sync::atomic::AtomicBool::new(false)),
        ) {
            Ok(matches) => {
                self.add_panel(TextPanel::search(pattern, &matches), SplitAxis::Rows);
            }
            Err(error) => self.message(Severity::Error, "grep", error.to_string()),
        }
    }

    pub(crate) fn browse(&mut self, url: &str) {
        self.browse_request(url, false);
    }

    #[tracing::instrument(name = "teal.bench.task", skip_all, fields(operation.id = teal_workbench_services::observability::operation_id(), operation.result = tracing::field::Empty))]
    pub(crate) fn browse_request(&mut self, url: &str, head: bool) {
        if let Err(error) = self
            .connectivity
            .require(teal_workbench_services::agent_algorithms::ConnectivityCapability::Network)
        {
            self.message(Severity::Error, "browser", error.to_string());
            return;
        }
        let parsed = match url::Url::parse(url) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => url,
            _ => {
                self.message(Severity::Error, "browser", "Requires HTTP/HTTPS URL");
                return;
            }
        };
        let panel = if self
            .panels
            .get(self.windows.focused())
            .and_then(AppPanel::as_browser)
            .is_some()
        {
            self.windows.focused()
        } else {
            self.add_panel(
                BrowserPanel::new(teal_host::web::WebDocument {
                    url: parsed.clone(),
                    title: parsed.to_string(),
                    status: 0,
                    lines: vec!["Loading…".into()],
                    links: Vec::new(),
                }),
                SplitAxis::Columns,
            )
        };
        self.browser_sequence += 1;
        let sequence = self.browser_sequence;
        self.browser_requests.insert(panel, sequence);
        let output = self.runtime_tx.clone();
        self.tasks.spawn(async move {
            let result = match TextBrowser::new()
                .map(|browser| browser.with_audit_context(teal_security::process_context()))
            {
                Ok(mut browser) => {
                    let result = if head {
                        browser.head(parsed.as_str()).await
                    } else {
                        browser.open(parsed.as_str()).await
                    };
                    result.cloned().map_err(|e| e.to_string())
                }
                Err(error) => Err(error.to_string()),
            };
            teal_workbench_services::observability::outcome(&result);
            let _ = output
                .send(RuntimeEvent::BrowserLoaded {
                    panel,
                    sequence,
                    result,
                })
                .await;
        });
    }

    pub(crate) fn navigate_browser(&mut self, forward: bool) {
        self.browser_requests.remove(&self.windows.focused());
        if let Some(browser) = self
            .panels
            .get_mut(self.windows.focused())
            .and_then(AppPanel::as_browser_mut)
        {
            browser.navigate(forward);
        }
    }
}

/// 把缓冲区搜索命中转换为编辑器高亮捕获。
///
/// `SearchMatch` 给出从 1 开始的行号与行内字节范围；这里按行首字节偏移换算绝对区间，
/// 并使用高优先级名称让命中覆盖同位置的基础语法高亮。
fn search_captures(
    source: &str,
    matches: &[teal_workbench_services::SearchMatch],
) -> Vec<teal_workbench_core::syntax::Capture> {
    let mut line_starts = Vec::with_capacity(matches.len());
    let mut offset = 0_usize;
    for (index, line) in source.split_inclusive('\n').enumerate() {
        line_starts.push((index + 1, offset));
        offset += line.len();
    }
    matches
        .iter()
        .filter_map(|entry| {
            let base = line_starts
                .iter()
                .find(|(line, _)| *line == entry.line)
                .map(|(_, offset)| *offset)
                .or_else(|| {
                    (entry.line == line_starts.len() + 1 && offset > 0).then_some(offset)
                })?;
            let start = base.saturating_add(entry.range.start);
            let end = base.saturating_add(entry.range.end);
            (start < end && end <= source.len()).then_some(teal_workbench_core::syntax::Capture {
                name: "search".to_owned(),
                range: start..end,
                priority: 200,
            })
        })
        .collect()
}
