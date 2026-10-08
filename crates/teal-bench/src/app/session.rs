//! 会话保存恢复、缓冲区恢复身份与事务日志接线。
//!
//! 会话描述工作台视图，恢复日志保存未落盘编辑；二者的版本、基线和恢复时机分别检查。
//! 同一共享缓冲区只有一份变化队列和日志，拆分视图不重复追加事务。

use super::*;

impl App {
    pub(super) fn ensure_recovery_identity(&mut self, id: PanelId) {
        let Some(buffer) = self
            .panels
            .get(id)
            .and_then(AppPanel::as_editor)
            .map(|editor| editor.document().clone())
        else {
            return;
        };
        if self.recovery_keys.contains_key(&buffer.id()) {
            return;
        }
        let identity = buffer.path().map_or_else(
            || {
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                format!("scratch:{}:{stamp}:{:?}", std::process::id(), buffer.id())
            },
            |path| path.to_string_lossy().into_owned(),
        );
        self.recovery_keys.insert(buffer.id(), identity);
    }

    pub(super) fn buffer_resource_changed(
        &mut self,
        buffer: &teal_workbench_core::BufferHandle,
        old_path: Option<&Path>,
    ) {
        self.invalidate_lsp_buffer(buffer.id(), true);
        let panel = self.panels.iter().find_map(|(id, p)| {
            p.as_editor()
                .filter(|e| e.document().id() == buffer.id())
                .map(|_| id)
        });
        if let Some(old_path) = old_path
            && let Ok(uri) = url::Url::from_file_path(old_path)
        {
            self.diagnostics.remove(uri.as_str());
            if let Some(server) = self.lsp.for_path(old_path) {
                self.tasks.spawn(async move {
                    let _ = server.close(uri.as_str()).await;
                });
            }
        }
        if let Some(path) = buffer.path() {
            if let Some(server) = self.lsp.for_path(&path)
                && let Ok(uri) = url::Url::from_file_path(&path)
            {
                let revision = buffer.revision();
                let text = buffer.to_string();
                self.tasks.spawn(async move {
                    let _ = server.open(uri.as_str(), revision, &text).await;
                });
            }
        } else if let Some(id) = panel {
            self.recovery_keys.remove(&buffer.id());
            self.ensure_recovery_identity(id);
            let hash: [u8; 32] = Sha256::digest(b"").into();
            match RecoveryJournal::create(self.recovery_path(id), hash) {
                Ok(mut journal) => {
                    let text = buffer.to_string();
                    if journal.append(&Transaction::insert(0, &text)).is_ok() {
                        self.journals.insert(
                            buffer.id(),
                            JournalTracker {
                                journal,
                                revision: buffer.revision(),
                                text,
                            },
                        );
                    }
                }
                Err(error) => self.message(Severity::Error, "recovery", error.to_string()),
            }
        }
    }

    pub(super) fn configure_session_restore_policy(&mut self, value: &str) {
        if let Some(policy) = SessionRestorePolicy::parse(value) {
            self.session_restore_policy = policy;
        } else {
            self.message(
                Severity::Warning,
                "config",
                format!(
                    "Invalid session recovery policy `{value}`; should be prompt, always, or never"
                ),
            );
        }
    }

    pub(super) fn begin_session_restore(
        &mut self,
        session: SessionState,
        policy: SessionRestorePolicy,
    ) {
        match policy {
            SessionRestorePolicy::Always => {
                if self.restore_session(session) {
                    self.recover_open_documents();
                    self.message(Severity::Info, "session", "Last session restored");
                } else {
                    self.message(
                        Severity::Warning,
                        "session",
                        "Unable to restore last session",
                    );
                }
            }
            SessionRestorePolicy::Never => {
                self.message(Severity::Info, "session", "Last session not restored");
            }
            SessionRestorePolicy::Prompt => {
                self.overlays.push(ConfirmOverlay::new(
                    "Restore last session",
                    "Restore last workspace session?",
                ));
                self.pending_overlay = Some(PendingOverlay::Confirmation(
                    PendingConfirmation::RestoreSession(Box::new(session)),
                ));
            }
        }
    }

    pub(super) fn restore_session(&mut self, session: SessionState) -> bool {
        if session.workspace.canonicalize().ok() != self.workspace.canonicalize().ok() {
            self.message(
                Severity::Error,
                "session",
                "The session belongs to another workspace, please open the corresponding workspace first",
            );
            return false;
        }
        // Reject legacy Agent payloads before touching any panel, window or buffer state.
        if !session.agents.is_empty() {
            self.message(
                Severity::Error,
                "session",
                "Legacy Bench Agent session cannot be restored by the Teal Agent application",
            );
            return false;
        }
        if session
            .active_agent
            .as_ref()
            .is_some_and(|name| !self.agent_app_scripts.contains_key(name))
        {
            self.message(
                Severity::Error,
                "session",
                "The session refers to an unknown Teal Agent application",
            );
            return false;
        }
        let mut panels = PanelStore::default();
        let mut buffers = teal_workbench_core::BufferStore::default();
        let mut recovery_keys = HashMap::new();
        let mut mapping = HashMap::new();
        let mut restored_tests_source = None;
        for panel in &session.panels {
            if let teal_workbench_services::PanelSessionState::TealCustom { name, .. } =
                &panel.state
                && !self.scripting.panels.contains_key(name)
            {
                self.message(
                    Severity::Warning,
                    "session",
                    format!("Custom panel {name} is not registered"),
                );
                continue;
            }
            if let Some(mut runtime) =
                restore_panel(panel, session.keymap, &self.grammars, &self.grammar_tables)
            {
                if let teal_workbench_services::PanelSessionState::Tests { source, .. } =
                    &panel.state
                {
                    restored_tests_source.clone_from(source);
                }
                if let Some(editor) = runtime.as_editor_mut() {
                    editor.bind_buffer(&buffers.adopt(editor.document().clone()));
                    if let teal_workbench_services::PanelSessionState::Editor {
                        recovery_key: Some(key),
                        ..
                    } = &panel.state
                    {
                        recovery_keys.insert(editor.document().id(), key.clone());
                    }
                }
                let id = panels.insert(runtime);
                mapping.insert(panel.id, id);
            }
        }
        if mapping.is_empty() {
            return false;
        }

        let mut windows = session.windows;
        for window_id in windows.order().to_vec() {
            let Some(window) = windows.get_mut(window_id) else {
                return false;
            };
            let layout = window.layout_mut();
            let Ok(ids) = layout.validate() else {
                return false;
            };
            for id in ids {
                if !mapping.contains_key(&id) {
                    let _ = layout.close(id);
                }
            }
            if layout.remap_panels(&mapping).is_err() {
                return false;
            }
        }

        // 恢复的面板标识组成有效布局后，才能替换当前状态。
        self.panels = panels;
        self.agent_panel = self
            .panels
            .iter()
            .find(|(_, p)| p.as_text().is_some() && p.kind() == PanelKind::Agent)
            .map(|(id, _)| id);
        self.buffers = buffers;
        self.recovery_keys = recovery_keys;
        for id in self.panels.iter().map(|(id, _)| id).collect::<Vec<_>>() {
            self.ensure_recovery_identity(id);
        }
        self.windows = windows;
        self.upgrade_primary_window();
        // 容错处理：丢弃意外持久化的瞬态窗口，保持会话只含持久窗口。
        self.windows.retain_persistent();
        // Tests 窗口恢复后重新绑定列表与同窗口的输出面板，使跳转、重跑与输出刷新继续可用。
        self.tests_panels = self
            .panels
            .iter()
            .find(|(_, panel)| panel.as_tests().is_some())
            .and_then(|(list, _)| {
                let window = self.windows.window_of(list)?;
                let output = self
                    .windows
                    .get(window)?
                    .validate()
                    .ok()?
                    .into_iter()
                    .find(|panel| {
                        *panel != list
                            && self
                                .panels
                                .get(*panel)
                                .is_some_and(|panel| panel.as_text().is_some())
                    })?;
                Some(crate::app::tests_view::TestsPanels {
                    window,
                    list,
                    output,
                })
            });
        if self.tests_panels.is_some() {
            self.tests_source = restored_tests_source;
        }
        self.keymap = session.keymap;
        // 源码 Agent 的历史、字段和 VM 检查点只属于 RuntimeStore run；工作台会话只恢复视图状态。
        if session.active_agent.is_some() {
            self.active_agent = session.active_agent;
        }
        self.journals.clear();
        self.fire_hook("session.restored", serde_json::json!({}));
        true
    }
}

impl App {
    pub(super) fn editor_snapshot(&self, id: PanelId) -> Option<(u64, String)> {
        self.panels
            .get(id)
            .and_then(|panel| panel.as_editor())
            .map(|editor| (editor.document().revision(), editor.document().to_string()))
    }

    pub(super) fn journal_if_changed(&mut self, _id: PanelId, _before: Option<(u64, String)>) {
        self.process_buffer_changes();
    }

    pub(super) fn process_buffer_changes(&mut self) {
        let buffers = self
            .panels
            .iter()
            .filter_map(|(id, panel)| {
                panel
                    .as_editor()
                    .map(|editor| (id, editor.document().clone()))
            })
            .collect::<Vec<_>>();

        for (id, buffer) in buffers {
            // 拆分视图共用此队列，因此每次已提交变化只消费一次。
            let changes = buffer.take_changes();
            let has_changes = !changes.is_empty();
            let journal_id = buffer.id();
            if !self.recovery_keys.contains_key(&journal_id) {
                self.ensure_recovery_identity(id);
            }

            for change in changes {
                if !self.journals.contains_key(&journal_id) {
                    let hash: [u8; 32] = Sha256::digest(change.old_text.as_bytes()).into();
                    if let Ok(journal) = RecoveryJournal::create(self.recovery_path(id), hash) {
                        self.journals.insert(
                            journal_id,
                            JournalTracker {
                                journal,
                                revision: change.old_revision,
                                text: change.old_text.clone(),
                            },
                        );
                    }
                }
                if let Some(tracker) = self.journals.get_mut(&journal_id)
                    && tracker.revision == change.old_revision
                    && tracker.journal.append(&change.transaction).is_ok()
                {
                    tracker.revision = change.revision;
                    tracker.text.clone_from(&change.new_text);
                }

                if let Some(path) = buffer.path()
                    && let Some(server) = self.lsp.for_path(&path)
                    && let Ok(uri) = url::Url::from_file_path(&path)
                {
                    let change = TextDocumentChange {
                        old_text: change.old_text.clone(),
                        new_text: change.new_text.clone(),
                        old_revision: change.old_revision,
                        revision: change.revision,
                        old_byte_range: change.old_byte_range.clone(),
                        inserted: change.inserted.clone(),
                    };
                    let errors = self.background_tx.clone();
                    self.tasks.spawn(async move {
                        if let Err(error) = server.change(uri.as_str(), &change).await {
                            let _ = errors
                                .send(Message::new(Severity::Error, "lsp", error.to_string()))
                                .await;
                        }
                    });
                }
            }

            if has_changes && !buffer.is_dirty() {
                self.reset_journal(id);
            }
            if has_changes {
                self.invalidate_lsp_buffer(buffer.id(), false);
                if let Some(path) = buffer.path() {
                    if let Ok(uri) = url::Url::from_file_path(&path) {
                        self.diagnostics.remove(uri.as_str());
                    }
                    self.note_buffer_change(buffer.id(), path);
                }
                self.request_lsp_for(id, lsp::RequestKind::Semantic);
            }
        }
        if let Err(error) = self.workspace_tools.synchronize_buffers(&self.buffers) {
            self.message(Severity::Error, "workspace", error.to_string());
        }
    }

    pub(super) fn reset_journal(&mut self, id: PanelId) {
        let Some(journal_id) = self
            .panels
            .get(id)
            .and_then(AppPanel::as_editor)
            .map(|e| e.document().id())
        else {
            return;
        };
        let Some((revision, text)) = self.editor_snapshot(id) else {
            return;
        };
        let hash: [u8; 32] = Sha256::digest(text.as_bytes()).into();
        if let Some(tracker) = self.journals.get_mut(&journal_id)
            && let Ok(journal) = RecoveryJournal::create(tracker.journal.path(), hash)
        {
            tracker.journal = journal;
            tracker.revision = revision;
            tracker.text = text;
        }
    }

    pub(super) fn recovery_identity(&self, id: PanelId) -> String {
        if let Some(key) = self
            .panels
            .get(id)
            .and_then(AppPanel::as_editor)
            .and_then(|editor| self.recovery_keys.get(&editor.document().id()))
        {
            return key.clone();
        }
        let identity = self
            .panels
            .get(id)
            .and_then(|panel| panel.as_editor())
            .and_then(|editor| editor.document().path())
            .map(|path| path.to_string_lossy().into_owned());
        identity.unwrap_or_else(|| {
            format!(
                "unregistered-scratch:{}:{id:?}",
                self.workspace.to_string_lossy()
            )
        })
    }

    pub(super) fn recovery_path(&self, id: PanelId) -> PathBuf {
        let identity = self.recovery_identity(id);
        let hash = Sha256::digest(identity.as_bytes());
        let mut name = String::with_capacity(24);
        for byte in &hash[..12] {
            write!(name, "{byte:02x}").expect("Writing to string does not fail");
        }
        self.paths
            .state
            .join("recovery")
            .join(format!("{name}.journal"))
    }

    pub(super) fn recover_open_documents(&mut self) {
        let ids = self.panels.iter().map(|(id, _)| id).collect::<Vec<_>>();
        for id in ids {
            self.recover_editor(id);
        }
    }

    pub(super) fn recover_editor(&mut self, id: PanelId) {
        if self
            .panels
            .get(id)
            .and_then(AppPanel::as_editor)
            .is_some_and(|editor| editor.document().is_dirty())
        {
            return;
        }
        let path = self.recovery_path(id);
        if !path.exists() {
            return;
        }
        let Some((_, current)) = self.editor_snapshot(id) else {
            return;
        };
        match RecoveryJournal::replay(&path) {
            Ok((base_hash, transactions)) => {
                let current_hash: [u8; 32] = Sha256::digest(current.as_bytes()).into();
                if current_hash != base_hash {
                    self.message(
                        Severity::Warning,
                        "recovery",
                        format!(
                            "The recovery baseline for {} is inconsistent with the disk",
                            path.display()
                        ),
                    );
                    return;
                }
                let mut applied = 0;
                if let Some(editor) = self
                    .panels
                    .get_mut(id)
                    .and_then(|panel| panel.as_editor_mut())
                {
                    for transaction in transactions {
                        if editor.document_mut().apply(transaction).is_ok() {
                            applied += 1;
                        } else {
                            break;
                        }
                    }
                }
                if applied > 0 {
                    self.message(
                        Severity::Info,
                        "recovery",
                        format!("Replayed {applied} times without saving edits"),
                    );
                }
            }
            Err(error) => self.message(Severity::Error, "recovery", error.to_string()),
        }
    }

    pub(super) fn save_session(&self) -> Result<(), teal_workbench_services::SessionError> {
        let mut state = SessionState::new(self.workspace.clone(), self.windows.clone());
        state.keymap = self.keymap;
        state.active_agent.clone_from(&self.active_agent);
        state.panels = self
            .panels
            .iter()
            .map(|(id, panel)| {
                let mut state = panel.session_state();
                if let teal_workbench_services::PanelSessionState::Editor { recovery_key, .. } =
                    &mut state
                {
                    *recovery_key = Some(self.recovery_identity(id));
                }
                if let teal_workbench_services::PanelSessionState::Text {
                    category, content, ..
                } = &mut state
                    && category == "agent"
                {
                    content.clear();
                }
                // 测试列表不保存编译程序，只记录本次运行的源码路径，恢复后据此重新编译。
                if let teal_workbench_services::PanelSessionState::Tests { source, .. } = &mut state
                    && self.tests_panels.is_some_and(|panels| panels.list == id)
                {
                    source.clone_from(&self.tests_source);
                }
                PanelSession { id, state }
            })
            .collect();
        let mut serialized = serde_json::to_value(state)?;
        redact_session_strings(&mut serialized, &[]);
        save_session(&self.session_path, &serde_json::from_value(serialized)?)
    }
}

fn redact_session_strings(value: &mut serde_json::Value, secrets: &[String]) {
    match value {
        serde_json::Value::String(text) => {
            for secret in secrets {
                *text = text.replace(secret, "[REDACTED]");
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_session_strings(value, secrets);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                redact_session_strings(value, secrets);
            }
        }
        _ => {}
    }
}

pub(super) fn open_editor(
    path: &Path,
    keymap: KeymapPreset,
    grammars: &HashMap<String, GrammarSpec>,
    tables: &HashMap<String, ParserTable>,
) -> Result<EditorPanel, teal_workbench_core::DocumentError> {
    let mut panel = EditorPanel::open(path, keymap)?;
    if let Some(extension) = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        && let Some(grammar) = grammars.get(&extension)
    {
        if let Some(table) = tables.get(&extension) {
            let languages = grammars
                .iter()
                .filter_map(|(key, grammar)| {
                    tables
                        .get(key)
                        .map(|table| (grammar.clone(), table.clone()))
                })
                .collect();
            let _ = panel.set_grammar_environment(grammar, table.clone(), languages);
        } else {
            let _ = panel.set_grammar(grammar);
        }
    }
    Ok(panel)
}

fn restore_panel(
    panel: &PanelSession,
    keymap: KeymapPreset,
    grammars: &HashMap<String, GrammarSpec>,
    tables: &HashMap<String, ParserTable>,
) -> Option<AppPanel> {
    use teal_workbench_services::PanelSessionState;
    match &panel.state {
        PanelSessionState::Editor {
            path,
            selections,
            scroll,
            column,
            ..
        } => {
            let mut editor = if let Some(path) = path {
                open_editor(path, keymap, grammars, tables).ok()?
            } else {
                EditorPanel::new(teal_workbench_core::Document::from_text(""), keymap)
            };
            editor.document().set_selections(selections.clone());
            editor.restore_view(*scroll, *column);
            Some(editor.into())
        }
        PanelSessionState::Explorer { tree } => {
            ExplorerPanel::restore(tree, false).ok().map(Into::into)
        }
        PanelSessionState::Browser {
            browser,
            scroll,
            column,
        } => BrowserPanel::restore(browser, *scroll, *column)
            .ok()
            .map(Into::into),
        PanelSessionState::Text {
            category,
            title,
            content,
            blocks,
            resource,
            scroll,
            column,
        } => {
            let kind = match category.as_str() {
                "agent" => PanelKind::Agent,
                "diff" => PanelKind::Diff,
                "messages" => PanelKind::Messages,
                "search" => PanelKind::Search,
                "markdown" => PanelKind::Markdown,
                _ => PanelKind::Output,
            };
            let mut panel = TextPanel::plain(kind, title.clone(), content);
            panel.restore_agent_blocks(blocks.as_ref());
            panel.restore_view(resource.clone(), *scroll, *column);
            Some(panel.into())
        }
        PanelSessionState::TealCustom {
            name,
            specification,
            widget_state,
        } => {
            let mut panel = crate::presentation::CustomPanel::new(name.clone());
            panel.ui.replace(specification).ok()?;
            panel.ui.restore_state(widget_state.clone()).ok()?;
            Some(panel.into())
        }
        PanelSessionState::Tests {
            rows,
            selected,
            scroll,
            filter,
            summary,
            ..
        } => {
            let mut widget = crate::workbench::ui::TestsWidget::new();
            widget.restore(
                rows.iter()
                    .map(crate::presentation::test_row_from_session)
                    .collect(),
                *selected,
                *scroll,
                filter.clone(),
                summary.clone(),
            );
            Some(widget.into())
        }
    }
}
