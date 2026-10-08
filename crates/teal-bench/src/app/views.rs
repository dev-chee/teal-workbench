//! 面板生命周期、布局焦点、保存与退出确认。
//!
//! 面板实例、布局叶节点和共享缓冲区的生命周期不同；关闭一个视图不能直接视为文档已无人使用。

use super::{
    App, AppPanel, SplitAxis, ConfirmOverlay, EditorPanel, ExplorerPanel, KeymapPreset,
    LayoutError, Message, PanelId, PanelKind, Path, PendingConfirmation, PendingOverlay, Severity,
    TextPanel, mutation,
};
use tokio::sync::oneshot;

impl App {
    pub(crate) fn add_panel(&mut self, panel: impl Into<AppPanel>, axis: SplitAxis) -> PanelId {
        let mut panel = panel.into();
        if let Some(editor) = panel.as_editor_mut() {
            editor.bind_buffer(&self.buffers.adopt(editor.document().clone()));
        }
        let target = self.windows.focused();
        let id = self.panels.insert(panel);
        if let Err(error) = self.windows.split(target, id, axis) {
            self.message(Severity::Error, "layout", error.to_string());
        }
        self.ensure_recovery_identity(id);
        id
    }

    pub(super) fn split_scratch(&mut self, vertical: bool) {
        let mut panel = EditorPanel::new(teal_workbench_core::Document::from_text(""), self.keymap);
        if let Some(grammar) = self.grammars.get("txt") {
            let _ = panel.set_grammar(grammar);
        }
        self.add_panel(
            panel,
            if vertical {
                SplitAxis::Columns
            } else {
                SplitAxis::Rows
            },
        );
    }

    pub(super) fn save_focused(&mut self) {
        let panel = self.windows.focused();
        if let Some(buffer) = self
            .panels
            .get(panel)
            .and_then(AppPanel::as_editor)
            .map(|e| e.document().id())
        {
            self.queue_work(mutation::Work::Save(buffer, None), Some(panel), None, None);
        }
    }

    pub(super) fn save_focused_as(&mut self, path: &Path) {
        let panel = self.windows.focused();
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            self.workspace.join(path)
        };
        if let Some(buffer) = self
            .panels
            .get(panel)
            .and_then(AppPanel::as_editor)
            .map(|e| e.document().id())
        {
            if self
                .buffers
                .find(&path)
                .is_some_and(|target| target.id() != buffer)
            {
                self.message(
                    Severity::Error,
                    "save",
                    "The target belongs to another opened buffer",
                );
                return;
            }
            self.queue_work(
                mutation::Work::Save(buffer, Some(path)),
                Some(panel),
                None,
                None,
            );
        }
    }

    pub(super) fn close_panel(&mut self, panel: PanelId, force: bool) {
        if self
            .panels
            .get(panel)
            .and_then(AppPanel::as_editor)
            .is_some_and(|e| e.document().is_busy())
        {
            self.message(
                Severity::Warning,
                "buffer",
                "Please wait until the current file operation is completed before closing",
            );
            return;
        }
        let dirty = self
            .panels
            .get(panel)
            .and_then(|panel| panel.as_editor())
            .is_some_and(|editor| editor.document().is_dirty());
        if dirty && !force {
            self.overlays.push(ConfirmOverlay::new(
                "Unsaved changes",
                "Close this unsaved buffer?",
            ));
            self.pending_overlay = Some(PendingOverlay::Confirmation(PendingConfirmation::Close(
                panel,
            )));
            return;
        }
        match self.windows.close_panel(panel) {
            Ok(()) => {
                self.release_panel(panel);
            }
            Err(LayoutError::LastPanel) => self.request_quit(force),
            Err(error) => self.message(Severity::Error, "layout", error.to_string()),
        }
    }

    /// 移除面板实例，并在没有视图继续引用时释放共享缓冲区身份。
    pub(super) fn release_panel(&mut self, panel: PanelId) {
        if let Some(removed) = self.panels.remove(panel)
            && let Some(editor) = removed.as_editor()
            && !self.panels.iter().any(|(_, p)| {
                p.as_editor()
                    .is_some_and(|e| e.document().id() == editor.document().id())
            })
        {
            let buffer = editor.document();
            self.buffers.remove(buffer.id());
            self.recovery_keys.remove(&buffer.id());
            self.invalidate_lsp_buffer(buffer.id(), true);
            self.journals.remove(&buffer.id());
            if let Some(path) = buffer.path()
                && let Some(server) = self.lsp.for_path(&path)
                && let Ok(uri) = url::Url::from_file_path(&path)
            {
                self.tasks.spawn(async move {
                    let _ = server.close(uri.as_str()).await;
                });
            }
        }
        if self.agent_panel == Some(panel) {
            self.agent_panel = None;
        }
        self.browser_requests.remove(&panel);
        self.fire_hook(
            "buffer.closed",
            serde_json::json!({"panel":format!("{panel:?}")}),
        );
    }

    pub(super) fn request_quit(&mut self, force: bool) {
        let dirty = self.panels.iter().any(|(_, panel)| {
            panel
                .as_editor()
                .is_some_and(|editor| editor.document().is_dirty())
        });
        if dirty && !force {
            self.overlays.push(ConfirmOverlay::new(
                "Unsaved changes",
                "Exit with unsaved buffer?",
            ));
            self.pending_overlay = Some(PendingOverlay::Confirmation(PendingConfirmation::Quit));
        } else {
            self.should_quit = true;
        }
    }

    pub(super) fn finish_confirmation(
        &mut self,
        confirmation: PendingConfirmation,
        accepted: bool,
    ) {
        match confirmation {
            PendingConfirmation::ExtensionLoad(loaded) => {
                if accepted {
                    self.install_extension(*loaded, true);
                }
            }
            PendingConfirmation::ExtensionApproval { extension, receipt } => {
                self.answer_extension_approval(&extension, &receipt, accepted)
            }
            PendingConfirmation::LspCommand {
                client,
                command,
                edit,
            } if accepted => {
                let edit_result = if let Some(edit) = edit {
                    let (sender, receiver) = oneshot::channel();
                    if let Err(error) = self.queue_workspace_edit(&edit, Some(sender)) {
                        self.message(Severity::Error, "lsp", error);
                        return;
                    }
                    Some(receiver)
                } else {
                    None
                };
                let errors = self.background_tx.clone();
                self.tasks.spawn(async move {
                    if let Some(receiver) = edit_result {
                        let result = receiver
                            .await
                            .unwrap_or_else(|_| Err("LSP edit job was cancelled".into()));
                        if let Err(error) = result {
                            let _ = errors
                                .send(Message::new(Severity::Error, "lsp", error))
                                .await;
                            return;
                        }
                    }
                    if let Err(error) = client.request("workspace/executeCommand", command).await {
                        let _ = errors
                            .send(Message::new(Severity::Error, "lsp", error.to_string()))
                            .await;
                    }
                });
            }
            PendingConfirmation::Agent(prompt) => {
                let response = if accepted {
                    teal_agent::ApprovalResponse::Allow(teal_agent::ApprovalScope::Once)
                } else {
                    teal_agent::ApprovalResponse::Deny
                };
                self.finish_agent_approval(*prompt, response);
            }
            PendingConfirmation::AgentMutation {
                approval_id,
                mutation,
            } if accepted => {
                let target = mutation
                    .path
                    .canonicalize()
                    .unwrap_or_else(|_| mutation.path.clone());
                let display = target
                    .strip_prefix(&self.workspace)
                    .unwrap_or(&target)
                    .to_string_lossy()
                    .trim_start_matches(['\\', '/'])
                    .to_owned();
                self.append_agent_approval(
                    approval_id,
                    "workspace.change".into(),
                    vec![display],
                    Some("allowed-once".into()),
                );
                self.queue_agent_mutation(*mutation);
            }
            PendingConfirmation::AgentMutation {
                approval_id,
                mutation,
            } => {
                let target = mutation
                    .path
                    .canonicalize()
                    .unwrap_or_else(|_| mutation.path.clone());
                let display = target
                    .strip_prefix(&self.workspace)
                    .unwrap_or(&target)
                    .to_string_lossy()
                    .trim_start_matches(['\\', '/'])
                    .to_owned();
                self.append_agent_approval(
                    approval_id,
                    "workspace.change".into(),
                    vec![display],
                    Some("denied".into()),
                );
                let _ = mutation
                    .response
                    .send(Err("Agent ChangeSet was rejected".into()));
            }
            PendingConfirmation::Close(panel) if accepted => {
                self.close_panel(panel, true);
            }
            PendingConfirmation::CloseWindow(id) if accepted => {
                let panels = self
                    .windows
                    .get(id)
                    .and_then(|window| window.validate().ok())
                    .unwrap_or_default();
                self.finish_close_window(id, &panels);
            }
            PendingConfirmation::AgentWrite { path, source } if accepted => {
                self.queue_agent_block_write(path, source);
            }
            PendingConfirmation::Quit if accepted => self.should_quit = true,
            PendingConfirmation::RestoreSession(session) if accepted => {
                if self.restore_session(*session) {
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
            PendingConfirmation::RestoreSession(_) => {
                self.message(Severity::Info, "session", "Last session not restored");
            }
            PendingConfirmation::ExplorerMove { panel, destination } if accepted => {
                self.explorer_operation_for(panel, |explorer| explorer.move_selected(&destination));
            }
            PendingConfirmation::ExplorerTrash(panel) if accepted => {
                self.explorer_operation_for(panel, ExplorerPanel::trash_selected);
            }
            PendingConfirmation::ExplorerDelete(panel) if accepted => {
                self.explorer_operation_for(panel, ExplorerPanel::delete_selected);
            }
            _ => {}
        }
    }
}

impl App {
    pub(super) fn set_keymap(&mut self, preset: KeymapPreset) {
        self.keymap = preset;
        for (_, panel) in self.panels.iter() {
            let _ = panel;
        }
        let ids = self.panels.iter().map(|(id, _)| id).collect::<Vec<_>>();
        for id in ids {
            if let Some(editor) = self
                .panels
                .get_mut(id)
                .and_then(|panel| panel.as_editor_mut())
            {
                editor.set_keymap(preset);
            }
        }
        let label = match preset {
            KeymapPreset::VimFirst => "Vim first",
            KeymapPreset::HelixFirst => "Helix first",
        };
        self.message(Severity::Info, "keymap", format!("Using {label} keymap"));
    }

    pub(super) fn open_messages(&mut self) {
        let text = self
            .messages
            .iter()
            .map(|message| {
                let severity = match message.severity {
                    Severity::Info => "info",
                    Severity::Warning => "warning",
                    Severity::Error => "error",
                };
                format!("{severity} [{}] {}", message.source, message.text)
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.add_panel(
            TextPanel::plain(PanelKind::Messages, "Messages", text),
            SplitAxis::Rows,
        );
    }
}
