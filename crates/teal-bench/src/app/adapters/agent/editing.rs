//! Agent编辑审查、执行与工作区事务交互。

use super::*;

impl App {
    /// 展示一个多文件 ChangeSet，并让用户选择整组、单文件或单 hunk 应用。
    pub(in crate::app) fn show_agent_changeset(
        &mut self,
        mut request: crate::app::ChangeSetRequest,
    ) {
        if self.should_quit {
            let _ = request
                .response
                .send(Err("ChangeSet review was cancelled during shutdown".into()));
            return;
        }
        request.change_set = match teal_workbench_core::ChangeSet::new(
            None,
            self.agent_generation,
            request.change_set.files,
        ) {
            Ok(change_set) => change_set,
            Err(error) => {
                let _ = request.response.send(Err(error.to_string()));
                return;
            }
        };
        let id = request.change_set.id.clone();
        let resources = request
            .change_set
            .files
            .iter()
            .map(|file| file.path.display().to_string())
            .collect::<Vec<_>>();
        let panel = self.ensure_agent_panel();
        if let Some(output) = self
            .panels
            .get_mut(panel)
            .and_then(|panel| panel.as_text_mut())
        {
            output.push_agent_block(AgentBlock::ChangeSetReview(request.change_set.clone()));
        }
        self.append_agent_approval(
            format!("changeset:{id}"),
            "workspace.apply_changeset".into(),
            resources,
            None,
        );

        let mut items = vec![MenuItem::enabled(
            "all",
            format!("Apply all {} files", request.change_set.files.len()),
        )];
        for (file_index, file) in request.change_set.files.iter().enumerate() {
            items.push(MenuItem::enabled(
                format!("file:{file_index}"),
                format!("Apply file {}", file.path.display()),
            ));
            items.push(MenuItem::enabled(
                format!("reject-file:{file_index}"),
                format!("Reject file {}", file.path.display()),
            ));
            for (hunk_index, hunk) in file.hunks.iter().enumerate() {
                items.push(MenuItem::enabled(
                    format!("hunk:{file_index}:{hunk_index}"),
                    format!(
                        "Apply hunk {}.{} {}",
                        file_index + 1,
                        hunk.index,
                        file.path.display()
                    ),
                ));
                items.push(MenuItem::enabled(
                    format!("reject-hunk:{file_index}:{hunk_index}"),
                    format!(
                        "Reject hunk {}.{} {}",
                        file_index + 1,
                        hunk.index,
                        file.path.display()
                    ),
                ));
            }
        }
        items.push(MenuItem::enabled("reject", "Reject ChangeSet"));
        self.overlays
            .push(PickerOverlay::with_items("Review ChangeSet", items));
        self.pending_overlay = Some(PendingOverlay::ChangeSet(Box::new(request)));
    }

    /// 将 ChangeSet 审查选择提交到串行事务队列。
    pub(in crate::app) fn finish_change_set_review(
        &mut self,
        request: crate::app::ChangeSetRequest,
        choice: Option<&str>,
    ) {
        let approval_id = format!("changeset:{}", request.change_set.id);
        if let Some(value) = choice
            && let Some(index) = value.strip_prefix("reject-file:")
        {
            let Some(file) = index
                .parse::<usize>()
                .ok()
                .and_then(|index| request.change_set.files.get(index))
            else {
                let _ = request
                    .response
                    .send(Err("Invalid ChangeSet selection".into()));
                return;
            };
            let path = file.path.display().to_string();
            self.append_agent_approval(
                approval_id,
                "workspace.apply_changeset".into(),
                vec![path.clone()],
                Some("rejected-file".into()),
            );
            let _ = request
                .response
                .send(Err(format!("ChangeSet file was rejected: {path}")));
            return;
        }
        if let Some(value) = choice
            && let Some(indexes) = value.strip_prefix("reject-hunk:")
        {
            let mut indexes = indexes.split(':');
            let selected = indexes
                .next()
                .and_then(|value| value.parse::<usize>().ok())
                .zip(indexes.next().and_then(|value| value.parse::<usize>().ok()))
                .filter(|_| indexes.next().is_none())
                .and_then(|(file_index, hunk_index)| {
                    request
                        .change_set
                        .files
                        .get(file_index)
                        .and_then(|file| file.hunks.get(hunk_index).map(|hunk| (file, hunk)))
                });
            let Some((file, hunk)) = selected else {
                let _ = request
                    .response
                    .send(Err("Invalid ChangeSet selection".into()));
                return;
            };
            let resource = format!("{}#hunk:{}", file.path.display(), hunk.index);
            self.append_agent_approval(
                approval_id,
                "workspace.apply_changeset".into(),
                vec![resource.clone()],
                Some("rejected-hunk".into()),
            );
            let _ = request
                .response
                .send(Err(format!("ChangeSet hunk was rejected: {resource}")));
            return;
        }
        let selection = match choice {
            Some("all") => Some((teal_workbench_core::ChangeSetSelection::All, "allowed-all")),
            Some(value) if value.starts_with("file:") => value[5..]
                .parse::<usize>()
                .ok()
                .and_then(|index| request.change_set.files.get(index))
                .map(|file| {
                    (
                        teal_workbench_core::ChangeSetSelection::File(file.path.clone()),
                        "allowed-file",
                    )
                }),
            Some(value) if value.starts_with("hunk:") => {
                let mut indexes = value[5..]
                    .split(':')
                    .filter_map(|part| part.parse::<usize>().ok());
                indexes
                    .next()
                    .zip(indexes.next())
                    .filter(|_| indexes.next().is_none())
                    .and_then(|(file_index, hunk_index)| {
                        let file = request.change_set.files.get(file_index)?;
                        let hunk = file.hunks.get(hunk_index)?;
                        Some((
                            teal_workbench_core::ChangeSetSelection::Hunk {
                                path: file.path.clone(),
                                id: hunk.id.clone(),
                            },
                            "allowed-hunk",
                        ))
                    })
            }
            Some("reject") | None => None,
            Some(_) => {
                let _ = request
                    .response
                    .send(Err("Invalid ChangeSet selection".into()));
                return;
            }
        };
        let Some((selection, decision)) = selection else {
            self.append_agent_approval(
                approval_id,
                "workspace.apply_changeset".into(),
                request
                    .change_set
                    .files
                    .iter()
                    .map(|file| file.path.display().to_string())
                    .collect(),
                Some("denied".into()),
            );
            let _ = request.response.send(Err("ChangeSet was rejected".into()));
            return;
        };
        // 用户接受后先固定整组检查点，使已应用的批量写入可以用一次回滚撤销。
        let selected =
            mutation::selected_change_roots(&self.workspace, &request.change_set, &selection);
        let _checkpoint = match self.mutations.group_checkpoint(&selected) {
            Ok(checkpoint) => checkpoint,
            Err(error) => {
                self.append_agent_approval(
                    approval_id,
                    "workspace.apply_changeset".into(),
                    request
                        .change_set
                        .files
                        .iter()
                        .map(|file| file.path.display().to_string())
                        .collect(),
                    Some("failed".into()),
                );
                let _ = request
                    .response
                    .send(Err(format!("ChangeSet checkpoint failed: {error}")));
                return;
            }
        };
        self.append_agent_approval(
            approval_id,
            "workspace.apply_changeset".into(),
            request
                .change_set
                .files
                .iter()
                .map(|file| file.path.display().to_string())
                .collect(),
            Some(decision.into()),
        );
        let summary = decision.to_owned();
        let response = request.response;
        let (completed, result) = tokio::sync::oneshot::channel();
        self.queue_work(
            mutation::Work::AgentChangeSet(request.change_set, selection),
            None,
            Some(completed),
            None,
        );
        self.tasks.spawn(async move {
            let outcome = result
                .await
                .unwrap_or_else(|_| Err("ChangeSet transaction response was closed".into()))
                .map(|()| summary);
            let _ = response.send(outcome);
        });
    }

    /// 将 Agent buffer 中编号从 1 开始的代码块打开为可编辑缓冲区。
    pub(in crate::app) fn insert_agent_code(&mut self, argument: &str) {
        let Ok(index) = argument.trim().parse::<usize>() else {
            self.message(
                Severity::Warning,
                "agent",
                "Usage: :agent-insert <code-block-number>",
            );
            return;
        };
        let Some(panel) = self.agent_panel else {
            self.message(Severity::Warning, "agent", "The Agent buffer is not open");
            return;
        };
        let Some((language, source)) = self
            .panels
            .get(panel)
            .and_then(AppPanel::as_text)
            .and_then(|panel| panel.agent_code(index))
        else {
            self.message(Severity::Warning, "agent", "Agent code block was not found");
            return;
        };
        let grammar_key = match language.as_str() {
            "teal" => "tl",
            "rust" => "rs",
            other => other,
        };
        let mut editor = EditorPanel::new(
            teal_workbench_core::Document::from_text(&source),
            self.keymap,
        );
        if let Some(grammar) = self.grammars.get(grammar_key) {
            let _ = editor.set_grammar(grammar);
        }
        self.add_panel(editor, SplitAxis::Columns);
        self.message(
            Severity::Info,
            "agent",
            format!("Agent code block {index} opened in an editable buffer"),
        );
    }

    /// 把 Agent buffer 中编号从 1 开始的代码块写入工作区文件。
    ///
    /// 目标必须位于工作区内；已存在且非空的文件需要先确认，写入经过共享事务与
    /// 打开缓冲区版本检查，因此不会覆盖审阅后又被修改的内容。
    pub(in crate::app) fn write_agent_code(&mut self, argument: &str) {
        let mut parts = argument.split_whitespace();
        let (Some(index), Some(path)) = (parts.next(), parts.next()) else {
            self.message(
                Severity::Warning,
                "agent",
                "Usage: :agent-write <code-block-number> <workspace-path>",
            );
            return;
        };
        if parts.next().is_some() {
            self.message(
                Severity::Warning,
                "agent",
                "Usage: :agent-write <code-block-number> <workspace-path>",
            );
            return;
        }
        let Ok(index) = index.parse::<usize>() else {
            self.message(
                Severity::Warning,
                "agent",
                "Usage: :agent-write <code-block-number> <workspace-path>",
            );
            return;
        };
        let Some(panel) = self.agent_panel else {
            self.message(Severity::Warning, "agent", "The Agent buffer is not open");
            return;
        };
        let Some((_language, source)) = self
            .panels
            .get(panel)
            .and_then(AppPanel::as_text)
            .and_then(|panel| panel.agent_code(index))
        else {
            self.message(Severity::Warning, "agent", "Agent code block was not found");
            return;
        };
        let requested = std::path::PathBuf::from(path);
        let target = if requested.is_absolute() {
            requested
        } else {
            self.workspace.join(requested)
        };
        if let Err(error) = crate::app::ensure_workspace_path(&self.workspace, &target) {
            self.message(Severity::Error, "agent", error);
            return;
        }
        let nonempty = std::fs::metadata(&target).is_ok_and(|metadata| metadata.len() > 0);
        if nonempty {
            self.overlays.push(ConfirmOverlay::new(
                "Overwrite file",
                format!(
                    "Replace {} with Agent code block {index}?",
                    target.display()
                ),
            ));
            self.pending_overlay = Some(PendingOverlay::Confirmation(
                PendingConfirmation::AgentWrite {
                    path: target,
                    source,
                },
            ));
            return;
        }
        self.queue_agent_block_write(target, source);
    }

    /// 通过共享事务把代码块写入目标路径。
    pub(in crate::app) fn queue_agent_block_write(
        &mut self,
        path: std::path::PathBuf,
        source: String,
    ) {
        let expected = self.buffers.find(&path).map(|buffer| {
            format!(
                "{}:{}",
                buffer.revision(),
                teal_workbench_services::agent_algorithms::digest(buffer.to_string().as_bytes())
            )
        });
        self.queue_work(
            mutation::Work::Agent(path.clone(), MutationKind::Write(source), expected),
            self.agent_panel,
            None,
            None,
        );
        self.message(
            Severity::Info,
            "agent",
            format!("Wrote Agent code block to {}", path.display()),
        );
    }
    /// 执行 Agent buffer 中编号从 1 开始的 Teal 代码块。
    pub(in crate::app) fn run_agent_code(&mut self, argument: &str) {
        let Ok(index) = argument.trim().parse::<usize>() else {
            self.message(
                Severity::Warning,
                "agent",
                "Usage: :agent-run <code-block-number>",
            );
            return;
        };
        let Some(panel) = self.agent_panel else {
            self.message(Severity::Warning, "agent", "The Agent buffer is not open");
            return;
        };
        let Some((language, source)) = self
            .panels
            .get(panel)
            .and_then(AppPanel::as_text)
            .and_then(|panel| panel.agent_code(index))
        else {
            self.message(Severity::Warning, "agent", "Agent code block was not found");
            return;
        };
        if !matches!(language.to_ascii_lowercase().as_str(), "teal" | "tl") {
            self.message(
                Severity::Warning,
                "agent",
                "Only Teal code blocks can be run directly; insert Rust into an editor first",
            );
            return;
        }
        let Some(executable) = teal_workbench_services::tools::distributed_teal_executable() else {
            self.message(
                Severity::Error,
                "agent",
                "The distributed Teal executable was not found",
            );
            return;
        };
        let cache = self.paths.cache.join("agent-code");
        if let Err(error) = std::fs::create_dir_all(&cache) {
            self.message(Severity::Error, "agent", error.to_string());
            return;
        }
        let mut file = match tempfile::Builder::new()
            .prefix("agent-")
            .suffix(".tl")
            .tempfile_in(&cache)
        {
            Ok(file) => file,
            Err(error) => {
                self.message(Severity::Error, "agent", error.to_string());
                return;
            }
        };
        if let Err(error) = file.write_all(source.as_bytes()) {
            self.message(Severity::Error, "agent", error.to_string());
            return;
        }
        if let Err(error) = file.flush() {
            self.message(Severity::Error, "agent", error.to_string());
            return;
        }
        let path = file.path().to_owned();
        let root = self.workspace.clone();
        let output = self.runtime_tx.clone();
        self.tasks.spawn(async move {
            let result = teal_workbench_services::run_program(
                &executable,
                &["run".to_owned(), path.to_string_lossy().into_owned()],
                &root,
            )
            .await
            .map_err(|error| error.to_string());
            let _ = output
                .send(RuntimeEvent::AgentCodeFinished { index, result })
                .await;
            drop(file);
        });
    }

    /// 将 Agent 代码块运行结果写回结构化 buffer。
    pub(in crate::app) fn finish_agent_code(
        &mut self,
        index: usize,
        result: Result<teal_workbench_services::ProcessOutput, String>,
    ) {
        let panel = self.ensure_agent_panel();
        let Some(output) = self
            .panels
            .get_mut(panel)
            .and_then(|panel| panel.as_text_mut())
        else {
            return;
        };
        match result {
            Ok(result) if result.status == 0 => output.push_agent_block(AgentBlock::ToolResult {
                name: format!("agent-code-{index}"),
                text: result.output,
                checkpoint: None,
            }),
            Ok(result) => output.push_agent_block(AgentBlock::Diagnostic {
                summary: format!(
                    "Agent code block {index} failed with status {}",
                    result.status
                ),
                detail: result.output,
            }),
            Err(error) => output.push_agent_block(AgentBlock::Diagnostic {
                summary: format!("Agent code block {index} could not run"),
                detail: error,
            }),
        }
    }

    pub(in crate::app) fn rollback_agent_checkpoint(&mut self) {
        self.message(
            Severity::Warning,
            "agent",
            "Agent checkpoints are owned by the source Agent RuntimeStore",
        );
    }

    pub(in crate::app) fn show_agent_mutation(&mut self, mutation: MutationRequest) {
        if !matches!(
            &mutation.kind,
            MutationKind::Write(_) | MutationKind::Replace { .. } | MutationKind::ApplyPatch { .. }
        ) {
            self.queue_agent_mutation(mutation);
            self.pump_approvals();
            return;
        }

        let target = mutation
            .path
            .canonicalize()
            .unwrap_or_else(|_| mutation.path.clone());
        let current = if let Some(buffer) = self.buffers.find(&target) {
            buffer.to_string()
        } else if target.exists() {
            match teal_workbench_core::Document::open(&target) {
                Ok(document) if document.is_read_only() => {
                    let _ = mutation.response.send(Err(
                        "Agent change target is not an editable text document".into(),
                    ));
                    return;
                }
                Ok(document) => document.to_string(),
                Err(error) => {
                    let _ = mutation.response.send(Err(error.to_string()));
                    return;
                }
            }
        } else {
            String::new()
        };
        let replacement = match crate::app::mutation_text(&current, mutation.kind.clone()) {
            Ok(replacement) => replacement,
            Err(error) => {
                let _ = mutation.response.send(Err(error));
                return;
            }
        };
        if replacement == current {
            let _ = mutation
                .response
                .send(Err("Agent change does not modify the target".into()));
            return;
        }
        let display = target
            .strip_prefix(&self.workspace)
            .unwrap_or(&target)
            .to_string_lossy()
            .trim_start_matches(['\\', '/'])
            .to_owned();
        let diff = TextDiff::from_lines(&current, &replacement)
            .unified_diff()
            .header(&format!("a/{display}"), &format!("b/{display}"))
            .to_string();
        let change_set = match teal_workbench_core::ChangeSetFile::new(
            PathBuf::from(&display),
            teal_workbench_services::agent_algorithms::digest(current.as_bytes()),
            teal_workbench_services::agent_algorithms::digest(replacement.as_bytes()),
            diff.clone(),
        )
        .and_then(|file| {
            teal_workbench_core::ChangeSet::new(None, self.agent_generation, vec![file])
        }) {
            Ok(change_set) => change_set,
            Err(error) => {
                let _ = mutation.response.send(Err(error.to_string()));
                return;
            }
        };
        let approval_id = format!("changeset:{}", change_set.id);
        let agent_panel = self.ensure_agent_panel();
        if let Some(output) = self
            .panels
            .get_mut(agent_panel)
            .and_then(|panel| panel.as_text_mut())
        {
            output.push_agent_block(AgentBlock::ChangeSetReview(change_set));
        }
        self.append_agent_approval(
            approval_id.clone(),
            "workspace.change".into(),
            vec![display.clone()],
            None,
        );
        self.add_panel(
            TextPanel::plain(PanelKind::Diff, format!("ChangeSet: {display}"), diff),
            SplitAxis::Columns,
        );
        self.overlays.push(ConfirmOverlay::new(
            "Review Agent change",
            format!("Review the diff in the ChangeSet panel for {display}.\n\nApply this change?"),
        ));
        self.pending_overlay = Some(PendingOverlay::Confirmation(
            PendingConfirmation::AgentMutation {
                approval_id,
                mutation: Box::new(mutation),
            },
        ));
    }

    pub(in crate::app) fn queue_agent_mutation(&mut self, mutation: MutationRequest) {
        // 审阅确认可能晚于原事件作用域；排入 IO 队列时恢复请求自身的订阅器和父跨度。
        let observation = mutation.observation.clone();
        observation.in_scope(|| {
            self.queue_work(
                mutation::Work::AgentChecked(
                    mutation.path,
                    mutation.kind,
                    mutation.expected_buffer_version,
                    mutation.expected_content_sha256,
                ),
                None,
                Some(mutation.response),
                None,
            );
        });
    }

    #[cfg(test)]
    pub(in crate::app) fn apply_agent_mutation(
        &mut self,
        path: &Path,
        kind: MutationKind,
    ) -> Result<(), String> {
        crate::app::ensure_workspace_path(&self.workspace, path)?;
        let target = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let current = if let Some(buffer) = self.buffers.find(&target) {
            buffer.to_string()
        } else if target.exists() {
            std::fs::read_to_string(&target).map_err(|error| error.to_string())?
        } else {
            String::new()
        };
        let replacement = crate::app::mutation_text(&current, kind)?;
        self.mutations
            .write(&self.buffers, &target, &replacement, true)?;
        self.process_buffer_changes();
        Ok(())
    }
}
