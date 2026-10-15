//! 从命令行调用到类型化应用动作的分发。
//!
//! 注册表列出命令并不意味着执行路径已接通；每个分支仍须明确参数处理与实际效果。

use super::{
    App, AppCommand, AppPanel, CmdlineMode, CommandInvocation, CommandRegistry, Direction,
    ExplorerPanel, InputEvent, KeymapPreset, MenuItem, Message, PathBuf, PendingOverlay,
    PickerOverlay, ScriptKind, Severity, TextBrowser, Widget, WindowKind, lsp,
};
use crate::workbench::ui::CompletionOutcome;
use std::sync::Arc;

use teal_agent::{
    KnowledgeProvider, MemoryApplicability, MemoryCandidate, MemoryEvidence, MemoryEvidenceKind,
    MemoryKind, MemoryRecord, MemoryScope, MemoryStore,
};
use tokio::sync::oneshot;

use crate::app::mutation;
use teal_workbench_services::RuntimeLockStatus;
use teal_workbench_services::tools::MutationKind;

enum MemoryCommand {
    Candidates,
    Find(String),
    Approve(MemoryScope, String),
    Delete(MemoryScope, String),
    Maintain { model_assisted: bool },
}

impl App {
    /// 返回活动窗口的类型名；没有活动窗口时返回 `None`。
    pub(super) fn active_window_kind(&self) -> Option<&'static str> {
        self.windows
            .get(self.windows.active_id())
            .map(|window| window.kind().as_str())
    }

    /// 按活动窗口刷新 `:` 命令补全菜单。
    ///
    /// 只在命令行处于命令模式且尚未输入参数时给出候选；候选按命令名前缀过滤，并受
    /// 窗口作用域限制。
    pub(super) fn refresh_command_completions(&mut self) {
        let Some(text) = self
            .cmdline
            .as_ref()
            .filter(|cmdline| cmdline.mode() == CmdlineMode::Command)
            .map(|cmdline| cmdline.body().to_owned())
        else {
            self.completion_menu.clear();
            return;
        };
        if text.contains(char::is_whitespace) {
            // 已经进入参数时不再给出顶层命令候选。
            self.completion_menu.clear();
            return;
        }
        let window = self.active_window_kind();
        let mut candidates = self
            .commands
            .complete_scoped(&text, window)
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        // 先排序再去重与截断，保证候选集是稳定且字典序最小的子集。
        candidates.sort();
        candidates.dedup();
        candidates.truncate(8);
        self.completion_menu.set_candidates(candidates);
    }

    /// 处理补全菜单按键；返回 `true` 表示按键已被菜单消费。
    pub(super) fn handle_completion_key(&mut self, input: InputEvent) -> bool {
        match self.completion_menu.handle_key(input) {
            CompletionOutcome::Ignored => false,
            CompletionOutcome::Consumed => true,
            CompletionOutcome::Accepted(candidate) => {
                self.accept_command_completion(&candidate);
                true
            }
        }
    }

    /// 把补全候选写回命令行首词并刷新菜单。
    ///
    /// 候选是完整命令名，因此替换首个词后保留其余光标位置以整词为单位更新。
    pub(super) fn accept_command_completion(&mut self, candidate: &str) {
        if let Some(cmdline) = &mut self.cmdline {
            cmdline.set_body(candidate, candidate.chars().count() + 1);
        }
        self.completion_menu.clear();
    }

    pub(super) fn execute_command_line(&mut self, line: &str) {
        let invocation = match CommandRegistry::parse(line) {
            Ok(invocation) => invocation,
            Err(error) => {
                self.message(Severity::Error, "cmdline", error.to_string());
                return;
            }
        };
        self.execute_invocation(&invocation);
    }

    pub(super) fn execute_invocation(&mut self, invocation: &CommandInvocation) {
        if invocation.name == "!" {
            if let Err(error) = self
                .connectivity
                .require(teal_workbench_services::agent_algorithms::ConnectivityCapability::ExternalSideEffect)
            {
                self.message(Severity::Error, "process", error.to_string());
                return;
            }
            if let Some(command) = invocation.args.first() {
                self.dispatch(AppCommand::Run(command.clone()));
            }
            return;
        }
        // `:scripts` 打开脚本清单视图，与 `:script` 的子命令互不冲突。
        if invocation.name == "scripts" {
            match teal_workbench_services::ScriptManifest::load(&self.workspace) {
                Ok(manifest) => self.list_scripts(&manifest),
                Err(error) => self.message(Severity::Error, "script", error.to_string()),
            }
            return;
        }
        // `:delivery` 由源码 Agent 生成交付视图。
        if invocation.name == "delivery" {
            self.submit_source_agent_input(":delivery".into());
            return;
        }
        let Some(spec) = self.commands.resolve(&invocation.name) else {
            self.message(
                Severity::Error,
                "cmdline",
                format!("Unknown command `{}`", invocation.name),
            );
            return;
        };
        let name = spec.name.clone();
        let arg = invocation.args.first().cloned().unwrap_or_default();
        // 命令作用域限制可执行判定：只在对应工作流窗口中运行的命令在其窗口中拒绝。
        if !self.commands.allows(&name, self.active_window_kind()) {
            self.message(
                Severity::Error,
                "cmdline",
                format!("`{name}` is not available in this window"),
            );
            return;
        }
        if self.route_extension_command(&name, &invocation.args) {
            return;
        }
        if let Some(registration) = self.script_commands.get(&name).cloned() {
            self.execute_script_path(&registration.path, registration.kind);
            return;
        }
        match name.as_str() {
            "extension" => self.extension_command(&invocation.args),
            "mode" => self.select_work_mode(&arg),
            "browse-head" => self.browse_request(&arg, true),
            "browse-find" => {
                if let Some(browser) = self
                    .panels
                    .get_mut(self.windows.focused())
                    .and_then(AppPanel::as_browser_mut)
                    && !browser.find(&arg)
                {
                    self.message(Severity::Info, "browser", "no match");
                }
            }
            "browse-refresh" => {
                if let Some(url) = self
                    .panels
                    .get(self.windows.focused())
                    .and_then(AppPanel::as_browser)
                    .map(|b| b.url().to_string())
                {
                    self.browse_request(&url, false);
                }
            }
            "panel" => self.open_custom_panel(&arg),
            "lsp-complete" => self.request_lsp(lsp::RequestKind::Complete),
            "lsp-hover" => self.request_lsp(lsp::RequestKind::Hover),
            "lsp-definition" => self.request_lsp(lsp::RequestKind::Definition),
            "lsp-references" => self.request_lsp(lsp::RequestKind::References),
            "lsp-rename" => {
                if arg.is_empty() {
                    self.start_lsp_rename();
                } else {
                    self.request_lsp(lsp::RequestKind::Rename(arg));
                }
            }
            "lsp-format" => self.request_lsp(lsp::RequestKind::Format),
            "lsp-code-action" => self.request_lsp(lsp::RequestKind::CodeAction),
            "lsp-semantic-tokens" => self.request_lsp(lsp::RequestKind::Semantic),
            "open" => self.dispatch(AppCommand::Open(PathBuf::from(arg))),
            "write" if invocation.args.is_empty() => self.dispatch(AppCommand::Save),
            "write" if invocation.args.len() == 1 => {
                let path = PathBuf::from(&invocation.args[0]);
                let path = if path.is_absolute() {
                    path
                } else {
                    self.workspace.join(path)
                };
                self.dispatch(AppCommand::SaveAs(path));
            }
            "write" => self.message(
                Severity::Error,
                "cmdline",
                "write only accepts a path; use quotes when containing spaces",
            ),
            "quit" => {
                self.dispatch(AppCommand::Quit {
                    force: invocation.bang,
                });
            }
            "window" => self.window_command(&invocation.args, invocation.bang),
            "plan" => self.submit_source_agent_input(
                format!(":plan {}", invocation.args.join(" "))
                    .trim()
                    .to_owned(),
            ),
            "roles"
                if self
                    .active_agent
                    .as_ref()
                    .is_some_and(|name| self.agent_app_scripts.contains_key(name)) =>
            {
                self.submit_source_agent_input(":roles".into());
            }
            "script" => self.script_command(&invocation.args),
            "packages" => self.packages_command(&invocation.args),
            "diagnostics" => self.diagnostics_command(),
            "test" => self.tests_command(&invocation.args),
            "test-select" => self.tests_select(&arg),
            "test-jump" => self.tests_jump(&arg),
            "test-rerun" => self.tests_rerun(&arg),
            "evidence" => self.submit_source_agent_input(":evidence".into()),
            "diff" => self.diff_command(&arg),
            "split" => self.dispatch(AppCommand::Split { vertical: false }),
            "vsplit" => self.dispatch(AppCommand::Split { vertical: true }),
            "focus" => {
                let direction = match arg.as_str() {
                    "up" | "k" => Some(Direction::Up),
                    "down" | "j" => Some(Direction::Down),
                    "left" | "h" => Some(Direction::Left),
                    "right" | "l" => Some(Direction::Right),
                    _ => None,
                };
                if let Some(direction) = direction {
                    self.dispatch(AppCommand::Focus(direction));
                }
            }
            "buffer" => {
                let panels = self.panels.iter().map(|(id, _)| id).collect::<Vec<_>>();
                let items = self
                    .panels
                    .iter()
                    .enumerate()
                    .map(|(index, (_, panel))| MenuItem::enabled(index.to_string(), panel.title()))
                    .collect();
                self.overlays
                    .push(PickerOverlay::with_items("buffer", items));
                self.pending_overlay = Some(PendingOverlay::BufferPicker(panels));
            }
            "explorer" => {
                self.dispatch(AppCommand::Explorer(if arg.is_empty() {
                    self.workspace.clone()
                } else {
                    PathBuf::from(arg)
                }));
            }
            "file-new" => self.explorer_operation(|explorer| explorer.create_file(&arg)),
            "dir-new" => self.explorer_operation(|explorer| explorer.create_directory(&arg)),
            "rename" => self.explorer_operation(|explorer| explorer.rename_selected(&arg)),
            "copy" => {
                let destination = PathBuf::from(&arg);
                self.explorer_operation(|explorer| {
                    let destination = if destination.is_absolute() {
                        destination.clone()
                    } else {
                        explorer.operation_directory().join(&destination)
                    };
                    explorer.copy_selected(&destination)
                });
            }
            "move" => {
                let destination = PathBuf::from(&arg);
                self.explorer_operation(|explorer| {
                    let destination = if destination.is_absolute() {
                        destination.clone()
                    } else {
                        explorer.operation_directory().join(&destination)
                    };
                    explorer.move_selected(&destination)
                });
            }
            "trash" => self.explorer_operation(ExplorerPanel::trash_selected),
            "delete" if invocation.bang => self.explorer_operation(ExplorerPanel::delete_selected),
            "delete" => self.message(
                Severity::Warning,
                "explorer",
                "To permanently delete, use :delete!",
            ),
            "search" => {
                self.dispatch(AppCommand::Search {
                    pattern: arg,
                    backwards: false,
                });
            }
            "grep" => self.dispatch(AppCommand::Grep(arg)),
            "browse" => self.dispatch(AppCommand::Browse(arg)),
            "preview" => self.preview_markdown(if arg.is_empty() {
                None
            } else {
                Some(PathBuf::from(arg))
            }),
            "agent" => {
                let input = invocation.args.join(" ");
                let input =
                    if self.extensions.mode == "task" && !input.trim_start().starts_with("task ") {
                        format!("task {input}")
                    } else {
                        input
                    };
                self.dispatch(AppCommand::Agent(input));
            }
            "agent-insert" => self.insert_agent_code(&arg),
            "agent-write" => self.write_agent_code(&invocation.args.join(" ")),
            "agent-run" => self.run_agent_code(&arg),
            "agent-graph" => self.load_agent_graph(&arg),
            "agent-branch" => self.submit_source_agent_input(
                format!(":agent-branch {}", invocation.args.join(" "))
                    .trim()
                    .to_owned(),
            ),
            "agent-merge" => self.start_three_way_merge(&invocation.args),
            "agent-merge-verify" => self.verify_three_way_merge(),
            "agent-imports" => self.load_agent_imports(&arg),
            "agent-callers" => self.load_agent_callers(&arg),
            "agent-callees" => self.load_agent_callees(&arg),
            "agent-symbols" => self.load_agent_symbols(&arg),
            "agent-occurrences" => self.load_agent_occurrences(&arg),
            "agent-visible" => self.load_agent_visible(&arg),
            "agent-references" => self.load_agent_references(&arg),
            "agent-tokens" => self.load_agent_tokens(&arg),
            "diagnose" => self.run_diagnose(&arg),
            "agent-context" => self.open_agent_diagnostic_context(&arg),
            "agent-view" => self.load_agent_analysis_view(&invocation.args),
            "agent-type" => self.load_agent_type(&arg),
            "task-commit"
                if self
                    .active_agent
                    .as_ref()
                    .is_some_and(|name| self.agent_app_scripts.contains_key(name)) =>
            {
                self.submit_source_agent_input(format!(":task-commit {arg}"));
            }
            "task-evidence"
                if self
                    .active_agent
                    .as_ref()
                    .is_some_and(|name| self.agent_app_scripts.contains_key(name)) =>
            {
                self.submit_source_agent_input(":evidence".into());
            }
            "task-commit" => self.message(
                Severity::Error,
                "agent",
                "The source Agent application is unavailable",
            ),
            "task-evidence" => self.message(
                Severity::Error,
                "agent",
                "The source Agent application is unavailable",
            ),
            "agents" => self.select_agent(&arg),
            "agent-answer" => self.open_durable_question(&invocation.args),
            "agent-config" => self.open_agent_config(&arg),
            "memory" => self.memory_command(&invocation.args),
            "knowledge" => self.knowledge_command(&invocation.args),
            "goal" => self.submit_source_agent_input(
                format!(":goal {}", invocation.args.join(" "))
                    .trim()
                    .to_owned(),
            ),
            "permissions" => self.permissions_command(&invocation.args),
            "runs" => self.runs_command(&invocation.args),
            "agent-setup" => self.setup_agent(&invocation.args),
            "runtime" => self.runtime_command(&invocation.args),
            "debug" => self.start_debug(&arg),
            "debug-continue" => {
                self.debug_command(crate::integration::debug::DebugCommand::Continue)
            }
            "debug-next" => self.debug_command(crate::integration::debug::DebugCommand::Next),
            "debug-step" => self.debug_command(crate::integration::debug::DebugCommand::StepIn),
            "debug-out" => self.debug_command(crate::integration::debug::DebugCommand::StepOut),
            "debug-stop" => self.debug_command(crate::integration::debug::DebugCommand::Stop),
            "break" => self.debug_breakpoints(&invocation.args),
            "debug-frame" => self.debug_select_frame(&arg),
            "debug-eval" => self.debug_evaluate(&invocation.args),
            "complete" => self.inline_complete(&arg),
            "source" => {
                let requested = PathBuf::from(arg);
                let requested = if requested.is_absolute() {
                    requested
                } else {
                    self.workspace.join(requested)
                };
                let path = match requested.canonicalize() {
                    Ok(path) => path,
                    Err(error) => {
                        self.message(
                            Severity::Error,
                            "teal",
                            format!("Unable to resolve script path: {error}"),
                        );
                        return;
                    }
                };
                let user_root = teal_config::ConfigPaths::discover(Some(&self.workspace))
                    .map(|paths| paths.teal_home)
                    .unwrap_or_else(|_| self.paths.config.clone());
                let user_root = user_root.canonicalize().unwrap_or(user_root);
                let kind = if path.starts_with(user_root) {
                    ScriptKind::UserConfig
                } else {
                    ScriptKind::WorkspaceConfig
                };
                self.load_script(&path, kind, invocation.bang);
            }
            "lsp" => self.start_lsp(&arg),
            "messages" => self.dispatch(AppCommand::Messages),
            "session" => self.dispatch(AppCommand::SaveSession),
            "keymap" => {
                let preset = if arg.eq_ignore_ascii_case("helix") {
                    KeymapPreset::HelixFirst
                } else {
                    KeymapPreset::VimFirst
                };
                self.dispatch(AppCommand::SetKeymap(preset));
            }
            "reload" => self.dispatch(AppCommand::Reload),
            _ => self.message(
                Severity::Warning,
                "cmdline",
                format!("Command `{name}` is registered but not yet implemented"),
            ),
        }
    }

    fn show_runtime_status(&mut self) {
        let message = match &self.runtime_lock_status {
            teal_workbench_services::RuntimeLockStatus::Ready => {
                "Project-locked Teal runtime is ready".to_owned()
            }
            teal_workbench_services::RuntimeLockStatus::Missing => {
                "The project does not have a .teal/runtime.lock; currently using the runtime distributed with the Bench".to_owned()
            }
            teal_workbench_services::RuntimeLockStatus::VersionMismatch { expected, actual } => {
                format!("Teal version mismatch: project requirement {expected}, current {actual}")
            }
            teal_workbench_services::RuntimeLockStatus::PackageMismatch(packages) => {
                format!("Runtime package lock differs from the current catalog: {}", packages.join(", "))
            }
            teal_workbench_services::RuntimeLockStatus::MissingArtifacts(artifacts) => {
                format!("Runtime artifact missing from user cache: {}", artifacts.join(", "))
            }
            teal_workbench_services::RuntimeLockStatus::MissingKnowledge => {
                "The project lock does not record Teal knowledge identity; run :runtime lock"
                    .to_owned()
            }
            teal_workbench_services::RuntimeLockStatus::KnowledgeMismatch {
                expected_version,
                actual_version,
                expected,
                actual,
            } => format!(
                "Teal knowledge lock mismatch: project v{expected_version} {expected}, current v{actual_version} {actual}; run :runtime lock"
            ),
        };
        self.message(Severity::Info, "runtime", message);
    }

    fn knowledge_command(&mut self, arguments: &[String]) {
        match arguments {
            [action] if action == "proposals" => {
                match self.proposal_store().and_then(|(store, workspace, _)| {
                    store.list(&workspace).map_err(|error| error.to_string())
                }) {
                    Ok(proposals) if proposals.is_empty() => self.message(Severity::Info, "knowledge", "No Agent knowledge proposals"),
                    Ok(proposals) => for proposal in proposals {
                        self.message(Severity::Info, "knowledge", format!("{} {:?} {}", proposal.id, proposal.status, proposal.title));
                    },
                    Err(error) => self.message(Severity::Error, "knowledge", error),
                }
            }
            [action, id] if action == "show" => {
                match self.proposal_store().and_then(|(store, workspace, _)| {
                    store.get(id).map_err(|error| error.to_string())?
                        .filter(|proposal| proposal.workspace == workspace)
                        .ok_or_else(|| "knowledge proposal does not exist in this workspace".into())
                }) {
                    Ok(proposal) => self.message(Severity::Info, "knowledge", format!(
                        "{} {:?} {}\nSource: {}\n\n{}\nReview: {:?}\nReceipt: {:?}",
                        proposal.id, proposal.status, proposal.title, proposal.source_run,
                        proposal.body, proposal.review, proposal.receipt,
                    )),
                    Err(error) => self.message(Severity::Error, "knowledge", error),
                }
            }
            [action, id] if action == "retry-review" => {
                let (proposals, workspace_id, root) = match self.proposal_store() {
                    Ok(value) => value,
                    Err(error) => {
                        self.message(Severity::Error, "knowledge", error);
                        return;
                    }
                };
                let id = id.clone();
                let workspace = self.workspace.clone();
                let knowledge = Arc::clone(&self.agent_knowledge);
                let permissions = Arc::clone(&self.agent_permissions);
                let tools = Arc::clone(&self.workspace_tools);
                let output = self.background_tx.clone();
                self.tasks.spawn(async move {
                    let result: Result<_, String> = async {
                        let proposal = proposals.get(&id).map_err(|error| error.to_string())?
                            .filter(|proposal| proposal.workspace == workspace_id)
                            .ok_or_else(|| "knowledge proposal does not exist in this workspace".to_owned())?;
                        if proposal.status == teal_knowledge::ProposalStatus::NeedsEvidence {
                            proposals.retry_review(&id).map_err(|error| error.to_string())?;
                        }
                        let status = teal_workbench_automation::process_knowledge_proposal(
                            proposals.as_ref(), &id, &workspace, &root,
                            Some(knowledge.as_ref()), permissions.as_ref(),
                            teal_workbench_automation::KnowledgePublicationWriter::Workspace(tools.as_ref()),
                            teal_security::process_context(),
                        ).await?;
                        if status == teal_knowledge::ProposalStatus::Published {
                            knowledge.refresh_layer(
                                teal_knowledge::KnowledgeScope::Project,
                                permissions.as_ref(),
                                teal_agent::ActionContext {
                                    run_id: "teal-bench/knowledge".into(),
                                    task_id: "knowledge".into(),
                                    generation: 0,
                                    policy_version: String::new(),
                                },
                                false,
                            ).await?;
                        }
                        Ok(status)
                    }.await;
                    let message = match result {
                        Ok(status) => Message::new(Severity::Info, "knowledge", format!("{id} {status:?}")),
                        Err(error) => Message::new(Severity::Error, "knowledge", error),
                    };
                    let _ = output.send(message).await;
                });
            }
            [] => {
                self.message(
                    Severity::Info,
                    "knowledge",
                    format!(
                        "Teal knowledge v{} sha256 {} (official embedded)",
                        self.agent_knowledge.toolchain_version(),
                        self.agent_knowledge.content_digest(),
                    ),
                );
                for scope in [
                    teal_knowledge::KnowledgeScope::User,
                    teal_knowledge::KnowledgeScope::Project,
                ] {
                    let status = self.agent_knowledge.status(scope);
                    let name = if scope == teal_knowledge::KnowledgeScope::User {
                        "user"
                    } else {
                        "workspace"
                    };
                    if !status.configured() {
                        self.message(
                            Severity::Info,
                            "knowledge",
                            format!("{name} layer: not configured"),
                        );
                        continue;
                    }
                    self.message(
                        Severity::Info,
                        "knowledge",
                        format!(
                            "{name} layer: {} files, {} bytes, cache {}{}",
                            status.files,
                            status.bytes,
                            if status.cached { "hit" } else { "rebuilt" },
                            status
                                .error
                                .as_deref()
                                .map_or(String::new(), |error| format!("; error {error}"))
                        ),
                    );
                }
            }
            [action, layer @ ..] if action == "refresh" => {
                let scope = match layer {
                    [] => None,
                    [single] if single == "all" => None,
                    [single] if single == "user" => Some(teal_knowledge::KnowledgeScope::User),
                    [single] if single == "workspace" || single == "project" => {
                        Some(teal_knowledge::KnowledgeScope::Project)
                    }
                    _ => {
                        self.message(
                            Severity::Error,
                            "knowledge",
                            "Usage: :knowledge refresh [user|workspace|all]",
                        );
                        return;
                    }
                };
                let workspace = self.workspace.clone();
                let knowledge = Arc::clone(&self.agent_knowledge);
                let permissions = Arc::clone(&self.agent_permissions);
                let output = self.background_tx.clone();
                let config = match teal_security::load_declarations(Some(&workspace)) {
                    Ok(config) => match crate::config::knowledge(&config) { Ok(value) => value, Err(error) => { self.message(Severity::Error, "knowledge", error.to_string()); return; } },
                    Err(error) => {
                        self.message(Severity::Error, "knowledge", error.to_string());
                        return;
                    }
                };
                self.tasks.spawn(async move {
                    let context = teal_agent::ActionContext {
                        run_id: "teal-bench/knowledge".into(),
                        task_id: "knowledge".into(),
                        generation: 0,
                        policy_version: String::new(),
                    };
                    let write_cache = config.cache;
                    let result = match scope {
                        Some(scope) => knowledge
                            .refresh_layer(scope, permissions.as_ref(), context, write_cache)
                            .await
                            .map(|status| vec![status]),
                        None => knowledge
                            .refresh(permissions.as_ref(), context, write_cache)
                            .await,
                    };
                    let message = match result {
                        Ok(statuses) if statuses.is_empty() => Message::new(
                            Severity::Info,
                            "knowledge",
                            "Knowledge layers are disabled",
                        ),
                        Ok(statuses) => Message::new(
                            Severity::Info,
                            "knowledge",
                            statuses
                                .iter()
                                .map(|status| {
                                    format!(
                                        "{}: {} files, {} bytes, cache {}",
                                        status.scope.name(),
                                        status.files,
                                        status.bytes,
                                        if status.cached { "hit" } else { "written" }
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("; "),
                        ),
                        Err(error) => Message::new(Severity::Error, "knowledge", error),
                    };
                    let _ = output.send(message).await;
                });
            }
            [action, query @ ..] if action == "find" && !query.is_empty() => {
                let query = query.join(" ");
                match self.agent_knowledge.search(&query, None, 10) {
                    Ok(hits) if hits.is_empty() => {
                        self.message(Severity::Info, "knowledge", "No matching knowledge entries");
                    }
                    Ok(hits) => {
                        for hit in hits {
                            self.message(
                                Severity::Info,
                                "knowledge",
                                format!(
                                    "[{}:{}] {} {}",
                                    hit.scope.name(),
                                    hit.kind.name(),
                                    hit.id,
                                    hit.snippet
                                ),
                            );
                        }
                    }
                    Err(error) => self.message(Severity::Error, "knowledge", error.to_string()),
                }
            }
            [action, id] if action == "read" => match self.agent_knowledge.read(id, 8192) {
                Ok(entry) => {
                    self.message(
                        Severity::Info,
                        "knowledge",
                        format!(
                            "# {}\n{}:{}-{}\n\n{}",
                            entry.title, entry.source, entry.line_start, entry.line_end, entry.body
                        ),
                    );
                }
                Err(error) => self.message(Severity::Error, "knowledge", error.to_string()),
            },
            _ => self.message(
                Severity::Error,
                "knowledge",
                "Usage: :knowledge | :knowledge proposals | :knowledge show <id> | :knowledge retry-review <id> | :knowledge refresh [user|workspace|all] | :knowledge find <query> | :knowledge read <id>",
            ),
        }
    }

    fn memory_command(&mut self, arguments: &[String]) {
        let command = match arguments {
            [] => MemoryCommand::Candidates,
            [action, query @ ..] if action == "find" && !query.is_empty() => {
                MemoryCommand::Find(query.join(" "))
            }
            [action, scope, id] if action == "approve" => {
                let Some(scope) = parse_memory_scope(scope) else {
                    self.message(
                        Severity::Error,
                        "memory",
                        "Memory scope must be user or workspace",
                    );
                    return;
                };
                MemoryCommand::Approve(scope, id.clone())
            }
            [action, scope, id] if action == "delete" => {
                let Some(scope) = parse_memory_scope(scope) else {
                    self.message(
                        Severity::Error,
                        "memory",
                        "Memory scope must be user or workspace",
                    );
                    return;
                };
                MemoryCommand::Delete(scope, id.clone())
            }
            [action] if action == "maintain" => MemoryCommand::Maintain {
                model_assisted: false,
            },
            [action, flag] if action == "maintain" && flag == "--model" => {
                MemoryCommand::Maintain {
                    model_assisted: true,
                }
            }
            _ => {
                self.message(
                    Severity::Error,
                    "memory",
                    "Usage: :memory | :memory find <query> | :memory approve <user|workspace> <id> | :memory delete <user|workspace> <id> | :memory maintain [--model]",
                );
                return;
            }
        };

        if let MemoryCommand::Maintain { model_assisted } = &command {
            let model_assisted = *model_assisted;
            self.run_memory_maintenance(model_assisted);
            return;
        }

        let memory = self.agent_memory.clone();
        let output = self.background_tx.clone();
        self.tasks.spawn(async move {
            let result =
                tokio::task::spawn_blocking(move || execute_memory_command(&memory, command)).await;
            let messages = match result {
                Ok(Ok(messages)) => messages,
                Ok(Err(error)) => vec![Message::new(Severity::Error, "memory", error.to_string())],
                Err(error) => vec![Message::new(
                    Severity::Error,
                    "memory",
                    format!("Memory background task failed: {error}"),
                )],
            };
            for message in messages {
                if output.send(message).await.is_err() {
                    break;
                }
            }
        });
    }

    /// 显式维护记忆候选：默认只做确定性提取，模型辅助提取必须由配置显式启用。
    fn run_memory_maintenance(&mut self, model_assisted: bool) {
        let (config, trusted) = match teal_security::load_declarations(Some(&self.workspace))
            .and_then(|effective| {
                Ok((
                    effective
                        .category::<teal_security::TealConfig>()?
                        .into_document(),
                    crate::config::trusted(&effective)?,
                ))
            }) {
            Ok(config) => config,
            Err(error) => {
                self.message(Severity::Error, "memory", error.to_string());
                return;
            }
        };
        if !config.memory.maintenance {
            self.message(
                Severity::Info,
                "memory",
                "Memory maintenance is disabled; enable memory.maintenance in the user configuration",
            );
            return;
        }
        if model_assisted && !(config.memory.maintenance && config.memory.model_assisted && trusted)
        {
            self.message(
                Severity::Error,
                "memory",
                "Model-assisted maintenance requires memory.model_assisted in the user configuration and a trusted workspace",
            );
            return;
        }
        let sources = match self.memory_maintenance_sources() {
            Ok(sources) => sources,
            Err(error) => {
                self.message(Severity::Error, "memory", error);
                return;
            }
        };
        if sources.is_empty() {
            self.message(
                Severity::Info,
                "memory",
                "No completed record with definite evidence provides memory candidates",
            );
            return;
        }
        if !model_assisted {
            self.finish_memory_maintenance(&sources);
            return;
        }
        self.start_model_assisted_maintenance(sources);
    }

    /// 收集确定来源；未完成或没有确定证据的记录不产生候选。
    fn memory_maintenance_sources(
        &self,
    ) -> Result<Vec<teal_agent::MemoryMaintenanceSource>, String> {
        let runtime = teal_agent::RuntimeStore::connect(&self.workspace)
            .map_err(|error| error.to_string())?;
        let mut sources = Vec::new();
        for record in self
            .agent_memory
            .candidates(100)
            .map_err(|error| error.to_string())?
        {
            let Some(run) = runtime
                .run(&record.source_run)
                .map_err(|error| error.to_string())?
            else {
                continue;
            };
            if run.state != teal_agent::RunState::Completed {
                continue;
            }
            let candidate = teal_agent::MemoryCandidate {
                scope: record.scope,
                kind: record.kind,
                content: record.content,
                source_run: record.source_run.clone(),
                source_event: record.source_event.clone(),
                confidence: record.confidence,
                verified: false,
                conflicts_with: record.conflicts_with,
                evidence: record.evidence,
                applicability: record.applicability,
                source_digest: record.source_digest.clone(),
                supersedes: record.supersedes,
            };
            let digest = if candidate.source_digest.is_empty() {
                teal_workbench_services::agent_algorithms::digest(candidate.content.as_bytes())
            } else {
                candidate.source_digest.clone()
            };
            sources.push(teal_agent::MemoryMaintenanceSource {
                run: candidate.source_run.clone(),
                event: candidate.source_event.clone(),
                digest,
                candidate,
            });
        }
        Ok(sources)
    }

    /// 通过受限维护预算向模型请求候选，并把结果交给同一个审计入口。
    fn start_model_assisted_maintenance(
        &mut self,
        sources: Vec<teal_agent::MemoryMaintenanceSource>,
    ) {
        let Some(name) = self.active_agent.clone() else {
            self.message(Severity::Error, "memory", "No active Agent is configured");
            return;
        };
        let Some((_, settings)) = self.agent_settings.get(&name).cloned() else {
            self.message(
                Severity::Error,
                "memory",
                format!("Active Agent {name} does not exist"),
            );
            return;
        };
        let model_access = self.model_access.clone();
        let output = self.background_tx.clone();
        let memory = self.agent_memory.clone();
        let prompt = maintenance_prompt(&sources);
        self.tasks.spawn(async move {
            let selector = if settings.provider == "default" || settings.model == "default" {
                teal_model::ModelSelector::default()
            } else {
                match (
                    teal_model::ProviderId::new(settings.provider.clone())
                        .map_err(|error| error.to_string()),
                    settings
                        .profile
                        .clone()
                        .map(teal_model::AuthProfileName::new)
                        .transpose()
                        .map_err(|error| error.to_string()),
                    teal_model::ModelId::new(settings.model.clone())
                        .map_err(|error| error.to_string()),
                ) {
                    (Ok(provider), Ok(profile), Ok(model)) => match profile {
                        Some(auth_profile) => teal_model::ModelSelector::Authenticated {
                            target: teal_model::ModelRef { provider, model },
                            auth_profile,
                        },
                        None => teal_model::ModelSelector::Provider {
                            provider,
                            model: Some(model),
                        },
                    },
                    (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => {
                        let _ = output
                            .send(Message::new(Severity::Error, "memory", error.to_string()))
                            .await;
                        return;
                    }
                }
            };
            let confirmed = match model_access
                .confirm(selector, teal_model::Cancellation::new())
                .await
            {
                Ok(confirmed) => confirmed,
                Err(error) => {
                    let _ = output
                        .send(Message::new(Severity::Error, "memory", error.to_string()))
                        .await;
                    return;
                }
            };
            let budget = teal_agent::RootBudgetLimits {
                model_rounds: 1,
                tool_calls: 1,
                input_tokens: 65_536,
                output_tokens: 4_096,
            };
            let services = match teal_agent::RuntimeServices::default().with_root_budget(budget) {
                Ok(services) => services,
                Err(error) => {
                    let _ = output
                        .send(Message::new(Severity::Error, "memory", error.to_string()))
                        .await;
                    return;
                }
            };
            let limits = teal_agent::RuntimeLimits {
                model_rounds: 1,
                tool_calls: 1,
                ..teal_agent::RuntimeLimits::default()
            };
            let config = teal_agent::AgentConfig {
                compaction_model: None,
                model: confirmed.session_context(),
                instructions: MAINTENANCE_INSTRUCTIONS.to_owned(),
                limits,
            };
            let agent = match services.build_agent(config, confirmed.provider()) {
                Ok(agent) => agent,
                Err(error) => {
                    let _ = output
                        .send(Message::new(Severity::Error, "memory", error.to_string()))
                        .await;
                    return;
                }
            };
            let run = crate::app::memory_maintenance_run(&sources);
            let invocation = teal_agent::AgentInvocation {
                context: teal_agent::ActionContext {
                    run_id: run.clone(),
                    task_id: run.clone(),
                    generation: 1,
                    policy_version: String::new(),
                },
                input: prompt,
                history: Vec::new(),
                window_checkpoint: None,
                window_origin_id: None,
                result_schema: teal_agent::DataShape::String,
                memory: None,
            };
            let result = agent.run(invocation, teal_model::Cancellation::new()).await;
            match result {
                Ok(result) => {
                    let proposals = result.value.as_str().unwrap_or_default();
                    let Some(model_sources) = parse_model_proposals(&run, proposals) else {
                        let _ = output
                            .send(Message::new(
                                Severity::Error,
                                "memory",
                                "Model-assisted extraction did not return a usable candidate list",
                            ))
                            .await;
                        return;
                    };
                    let request = teal_agent::MemoryMaintenanceRequest {
                        run: crate::app::memory_maintenance_run(&model_sources),
                        source_digest: crate::app::memory_maintenance_digest(&model_sources),
                        extraction: teal_agent::MemoryExtraction::ModelAssisted,
                        budget: Some(teal_agent::MemoryMaintenanceBudget {
                            identity: run,
                            candidate_limit: MODEL_MAINTENANCE_CANDIDATES,
                        }),
                        scope: MemoryScope::Workspace,
                        sources: model_sources,
                    };
                    submit_memory_maintenance(output, memory, request).await;
                }
                Err(error) => {
                    let _ = output
                        .send(Message::new(Severity::Error, "memory", error.to_string()))
                        .await;
                }
            }
        });
    }

    /// 把已经确定的候选写入审计账本；模型辅助提取也复用同一入口。
    fn finish_memory_maintenance(&mut self, sources: &[teal_agent::MemoryMaintenanceSource]) {
        let run = crate::app::memory_maintenance_run(sources);
        let source_digest = crate::app::memory_maintenance_digest(sources);
        let request = teal_agent::MemoryMaintenanceRequest {
            run,
            source_digest,
            extraction: teal_agent::MemoryExtraction::Deterministic,
            budget: None,
            scope: MemoryScope::Workspace,
            sources: sources.to_vec(),
        };
        let memory = self.agent_memory.clone();
        let output = self.background_tx.clone();
        self.tasks.spawn(async move {
            submit_memory_maintenance(output, memory, request).await;
        });
    }

    fn permissions_command(&mut self, arguments: &[String]) {
        let paths = match teal_config::ConfigPaths::discover(Some(&self.workspace)) {
            Ok(paths) => paths,
            Err(error) => {
                self.message(Severity::Error, "permissions", error.to_string());
                return;
            }
        };
        let show = || {
            teal_security::load_declarations_paths(paths.clone())
                .map_err(|error| error.to_string())
                .and_then(|effective| {
                    crate::config::permissions(&effective).map_err(|error| error.to_string())
                })
                .map(|profile| {
                    format!(
                        "preset={:?} generation={} rules={}",
                        profile.preset,
                        profile.generation,
                        profile.rules.len()
                    )
                })
        };
        let result = match arguments {
            [] => show(),
            [action] if action == "show" => show(),
            [action] if action == "develop" => teal_security::set_permission_preset(
                &paths,
                teal_security::PermissionPreset::Develop,
            )
            .map_err(|error| error.to_string())
            .map(|()| "Workspace development permissions enabled".to_owned()),
            [action] if action == "ask" => {
                teal_security::set_permission_preset(&paths, teal_security::PermissionPreset::Ask)
                    .map_err(|error| error.to_string())
                    .map(|()| "Workspace permission mode set to ask".to_owned())
            }
            [action] if action == "explore" => teal_security::set_permission_preset(
                &paths,
                teal_security::PermissionPreset::Explore,
            )
            .map_err(|error| error.to_string())
            .map(|()| "Workspace permission mode set to explore (read-only)".to_owned()),
            [action] if action == "revoke" => teal_security::revoke_workspace_permissions(&paths)
                .map_err(|error| error.to_string())
                .and_then(|()| {
                    self.trust
                        .clear_directory_approvals()
                        .map_err(|error| error.to_string())
                })
                .map(|()| "Workspace permissions revoked".to_owned()),
            _ => {
                self.message(
                    Severity::Error,
                    "permissions",
                    "Usage: :permissions [show|explore|ask|develop|revoke]",
                );
                return;
            }
        };
        match result {
            Ok(message) => self.message(Severity::Info, "permissions", message),
            Err(error) => self.message(Severity::Error, "permissions", error),
        }
    }

    fn runtime_command(&mut self, arguments: &[String]) {
        if arguments.is_empty() {
            self.show_runtime_status();
            return;
        }
        if matches!(arguments, [action] if action == "lock") {
            let artifacts = self
                .runtime_lock
                .as_ref()
                .map_or_else(Vec::new, |lock| lock.artifacts.clone());
            let profile = self
                .runtime_lock
                .as_ref()
                .map_or_else(|| "agentic".to_owned(), |lock| lock.tier.clone());
            let lock = match teal_workbench_services::RuntimeLock::new(
                env!("CARGO_PKG_VERSION"),
                self.runtime_packages.clone(),
                artifacts,
                profile,
            ) {
                Ok(lock) => lock
                    .with_catalog_identity(
                        self.runtime_core_catalog_digest.clone(),
                        self.runtime_extension_catalog_digest.clone(),
                        teal::PACKAGE_FORMAT_VERSION,
                    )
                    .with_knowledge(teal_workbench_services::RuntimeKnowledge {
                        format_version: teal_knowledge::KNOWLEDGE_FORMAT_VERSION,
                        toolchain_digest: self.agent_knowledge.toolchain_digest().to_owned(),
                        content_digest: self.agent_knowledge.content_digest().to_owned(),
                    }),
                Err(error) => {
                    self.message(Severity::Error, "runtime", error.to_string());
                    return;
                }
            };
            let contents = match serde_json::to_string_pretty(&lock) {
                Ok(mut contents) => {
                    contents.push('\n');
                    contents
                }
                Err(error) => {
                    self.message(Severity::Error, "runtime", error.to_string());
                    return;
                }
            };
            let path = self.workspace.join(".teal/runtime.lock");
            let (response, result) = oneshot::channel();
            self.queue_work(
                mutation::Work::Agent(path, MutationKind::Write(contents), None),
                None,
                Some(response),
                None,
            );
            let output = self.runtime_tx.clone();
            self.tasks.spawn(async move {
                let result = result
                    .await
                    .unwrap_or_else(|_| Err("Runtime lock mutation response was closed".into()));
                let _ = output
                    .send(crate::runtime_bridge::RuntimeEvent::RuntimeLockSaved { lock, result })
                    .await;
            });
            return;
        }
        if let Err(error) = self
            .connectivity
            .require(teal_workbench_services::agent_algorithms::ConnectivityCapability::Network)
        {
            self.message(Severity::Error, "runtime", error.to_string());
            return;
        }
        let [action, name, source] = arguments else {
            self.message(
                Severity::Error,
                "runtime",
                "Usage: :runtime | :runtime lock | :runtime install <artifact> <https-url>",
            );
            return;
        };
        if action != "install" {
            self.message(
                Severity::Error,
                "runtime",
                "The runtime only supports the install operation",
            );
            return;
        }
        let Some(lock) = &self.runtime_lock else {
            self.message(
                Severity::Error,
                "runtime",
                "Project does not have .teal/runtime.lock",
            );
            return;
        };
        let Some(artifact) = lock
            .artifacts
            .iter()
            .find(|artifact| &artifact.name == name)
            .cloned()
        else {
            self.message(
                Severity::Error,
                "runtime",
                format!("Lock file has no artifact {name}"),
            );
            return;
        };
        let cache = self.runtime_cache.clone();
        let source = source.clone();
        let output = self.background_tx.clone();
        self.tasks.spawn(async move {
            let message = match cache.download(&artifact, &source).await {
                Ok(path) => Message::new(
                    Severity::Info,
                    "runtime",
                    format!("Artifact verified and cached: {}", path.display()),
                ),
                Err(error) => Message::new(Severity::Error, "runtime", error.to_string()),
            };
            let _ = output.send(message).await;
        });
    }

    /// 处理 `:window` 子命令：列出、新建、关闭与切换窗口。
    fn window_command(&mut self, args: &[String], force: bool) {
        match args {
            [] => {
                let listing = self
                    .windows
                    .iter()
                    .enumerate()
                    .map(|(index, (id, window))| {
                        format!(
                            "{} {}{}{}",
                            index + 1,
                            window.kind().as_str(),
                            if window.title().is_empty() {
                                String::new()
                            } else {
                                format!(" {}", window.title())
                            },
                            if id == self.windows.active_id() {
                                " *"
                            } else {
                                ""
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");
                self.message(Severity::Info, "window", listing);
            }
            [action] if action == "new" => self.open_window(WindowKind::Editor),
            [action, kind] if action == "new" => match WindowKind::parse(kind) {
                Some(kind) => self.open_window(kind),
                None => self.message(
                    Severity::Error,
                    "window",
                    "unknown window kind; expected agent, debug or editor",
                ),
            },
            [action] if action == "float" => self.open_float_window(WindowKind::Editor),
            [action, kind] if action == "float" => match WindowKind::parse(kind) {
                Some(kind) => self.open_float_window(kind),
                None => self.message(
                    Severity::Error,
                    "window",
                    "unknown window kind; expected agent, debug or editor",
                ),
            },
            [action] if action == "dock" => {
                let id = self.windows.active_id();
                self.windows.set_docked(id);
            }
            [action] if action == "grow" => {
                self.resize_active_window(Direction::Right, true);
                self.resize_active_window(Direction::Down, true);
            }
            [action] if action == "shrink" => {
                self.resize_active_window(Direction::Right, false);
                self.resize_active_window(Direction::Down, false);
            }
            [action] if action == "maximize" => {
                let id = self.windows.active_id();
                self.windows.set_docked(id);
            }
            [action] if action == "minimize" => {
                let id = self.windows.active_id();
                let rect = self.default_float_rect();
                self.windows.set_floating(id, rect);
            }
            [action] if action == "close" => self.close_active_window(force),
            [action] if action == "next" => {
                self.windows.select_next();
            }
            [action] if action == "prev" => {
                self.windows.select_previous();
            }
            [action, index] if action == "select" => match index.parse::<usize>() {
                Ok(index) if self.windows.select_index(index) => {}
                _ => self.message(Severity::Error, "window", "no such window"),
            },
            _ => self.message(
                Severity::Error,
                "window",
                "usage: :window [list|new|float [kind]|dock|maximize|minimize|close|next|prev|select N]",
            ),
        }
    }

    /// 处理 `:diff [PATH]`：比较打开缓冲区与磁盘内容的差异。
    fn diff_command(&mut self, argument: &str) {
        let requested = argument.trim();
        let panel = self.windows.focused();
        let editor = self.panels.get(panel).and_then(AppPanel::as_editor);
        let (path, text) = match editor {
            Some(editor) if requested.is_empty() => {
                let Some(path) = editor.document().path() else {
                    self.message(
                        Severity::Warning,
                        "diff",
                        "The focused buffer has no file; pass an explicit path",
                    );
                    return;
                };
                (path, editor.document().text().to_string())
            }
            _ => {
                if requested.is_empty() {
                    self.message(Severity::Warning, "diff", "Usage: :diff <workspace-path>");
                    return;
                }
                let requested = std::path::PathBuf::from(requested);
                let target = if requested.is_absolute() {
                    requested
                } else {
                    self.workspace.join(requested)
                };
                if let Err(error) = crate::app::ensure_workspace_path(&self.workspace, &target) {
                    self.message(Severity::Error, "diff", error);
                    return;
                }
                let text = self
                    .buffers
                    .find(&target)
                    .map(|buffer| buffer.to_string())
                    .or_else(|| std::fs::read_to_string(&target).ok())
                    .unwrap_or_default();
                (target, text)
            }
        };
        let disk = std::fs::read_to_string(&path).unwrap_or_default();
        let text = render_text_diff(&disk, &text, &path);
        self.open_report_window(WindowKind::Diff, "Diff", &text);
        self.message(
            Severity::Info,
            "diff",
            format!("Diff for {}", path.display()),
        );
    }
    /// 处理 `:diagnostics`：把当前语言服务器与分析器诊断聚合成只读视图。
    fn diagnostics_command(&mut self) {
        let entries = self.diagnostics.snapshot(None);
        let text = render_diagnostics(&entries);
        self.open_report_window(WindowKind::Diagnostics, "Diagnostics", &text);
        self.message(
            Severity::Info,
            "diagnostics",
            format!("{} diagnostics", entries.len()),
        );
    }
    /// 处理 `:packages`：列出内置扩展包目录或核对项目锁与本地缓存。
    fn packages_command(&mut self, arguments: &[String]) {
        match arguments {
            [] => self.show_packages(),
            [action] if action == "list" => self.show_packages(),
            [action] if action == "verify" => self.verify_packages(),
            _ => self.message(
                Severity::Error,
                "packages",
                "Usage: :packages [list|verify]",
            ),
        }
    }

    /// 显示当前发行提供的内置扩展包及其摘要前缀。
    fn show_packages(&mut self) {
        if self.runtime_packages.is_empty() {
            self.message(
                Severity::Error,
                "packages",
                "No first-party package is available in this distribution",
            );
            return;
        }
        let listing = self
            .runtime_packages
            .iter()
            .map(|package| {
                let digest = &package.digest;
                let short = &digest[..digest.len().min(12)];
                format!("{} {} ({short})", package.name, package.version)
            })
            .collect::<Vec<_>>()
            .join(" | ");
        self.message(
            Severity::Info,
            "packages",
            format!("{} packages: {listing}", self.runtime_packages.len()),
        );
    }

    /// 核对项目锁、发行包与本地缓存，报告全部依赖问题。
    fn verify_packages(&mut self) {
        let problems = self.package_diagnostics();
        if problems.is_empty() {
            self.message(
                Severity::Info,
                "packages",
                "Package catalog, runtime lock and cache are consistent",
            );
        } else {
            self.message(
                Severity::Error,
                "packages",
                format!("Dependency problems: {}", problems.join("; ")),
            );
        }
    }

    /// 返回当前依赖问题的稳定描述；空结果表示一致。
    fn package_diagnostics(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.runtime_packages.is_empty() {
            problems.push("no first-party package is installed".to_owned());
        }
        match &self.runtime_lock_status {
            RuntimeLockStatus::Ready => {}
            RuntimeLockStatus::Missing => {
                problems.push("the project has no runtime lock; run `:runtime lock`".to_owned());
            }
            RuntimeLockStatus::VersionMismatch { expected, actual } => problems.push(format!(
                "locked Teal version {expected} differs from running {actual}"
            )),
            RuntimeLockStatus::PackageMismatch(names) => problems.push(format!(
                "locked packages differ from this distribution: {}",
                names.join(", ")
            )),
            RuntimeLockStatus::MissingKnowledge => {
                problems.push("the lock has no knowledge snapshot; refresh it".to_owned());
            }
            RuntimeLockStatus::KnowledgeMismatch {
                expected_version,
                actual_version,
                expected,
                actual,
            } => problems.push(format!(
                "locked knowledge v{expected_version} {expected} differs from v{actual_version} {actual}"
            )),
            RuntimeLockStatus::MissingArtifacts(names) => problems.push(format!(
                "the runtime cache is missing artifacts: {}",
                names.join(", ")
            )),
        }
        if let Some(lock) = &self.runtime_lock {
            let available = self
                .runtime_packages
                .iter()
                .map(|package| package.name.as_str())
                .collect::<std::collections::HashSet<_>>();
            let missing = lock
                .packages
                .iter()
                .map(|package| package.name.clone())
                .filter(|name| !available.contains(name.as_str()))
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                problems.push(format!(
                    "packages required by the lock are unavailable: {}",
                    missing.join(", ")
                ));
            }
        }
        problems
    }
    /// 处理 `:script`：列出、脚手架、打开与改写配置档。
    fn script_command(&mut self, args: &[String]) {
        let mut manifest = match teal_workbench_services::ScriptManifest::load(&self.workspace) {
            Ok(manifest) => manifest,
            Err(error) => {
                self.message(Severity::Error, "script", error.to_string());
                return;
            }
        };
        match args {
            [] => self.list_scripts(&manifest),
            [action] if action == "list" => self.list_scripts(&manifest),
            [action, name] if action == "new" => {
                let entry = teal_workbench_services::ScriptEntry::new(
                    name.clone(),
                    std::path::Path::new(teal_workbench_services::DEFAULT_SCRIPT_DIR)
                        .join(format!("{name}.tl")),
                );
                manifest.upsert(entry.clone());
                if let Err(error) = manifest.validate() {
                    manifest.remove(name);
                    self.message(Severity::Error, "script", error.to_string());
                    return;
                }
                let path = self.workspace.join(&entry.entry);
                if !path.exists() {
                    if let Some(parent) = path.parent()
                        && !parent.exists()
                        && let Err(error) = self.mutations.execute(
                            &self.buffers,
                            teal_workbench_core::WorkspaceIntent::Create {
                                path: parent.to_path_buf(),
                                directory: true,
                            },
                            true,
                        )
                    {
                        self.message(Severity::Error, "script", error);
                        return;
                    }
                    if let Err(error) = self.mutations.create_new_text(
                        &self.buffers,
                        &path,
                        &script_scaffold(name),
                        true,
                    ) {
                        self.message(Severity::Error, "script", error.to_string());
                        return;
                    }
                }
                if let Err(error) = manifest.save_with_workspace_transaction(
                    &self.workspace,
                    &self.buffers,
                    &self.mutations,
                ) {
                    self.message(Severity::Error, "script", error.to_string());
                    return;
                }
                self.message(
                    Severity::Info,
                    "script",
                    format!("Registered script `{name}` at {}", entry.entry.display()),
                );
                self.open_path(path);
            }
            [action, name] if action == "open" => {
                let Some(entry) = manifest.find(name).cloned() else {
                    self.message(
                        Severity::Error,
                        "script",
                        format!("unknown script `{name}`"),
                    );
                    return;
                };
                self.open_path(self.workspace.join(entry.entry));
            }
            [action, name, profile] if action == "tier" => {
                if manifest.find(name).is_none() {
                    self.message(
                        Severity::Error,
                        "script",
                        format!("unknown script `{name}`"),
                    );
                    return;
                }
                self.apply_script_tier(name, profile);
            }
            [action, name] if action == "tier" => {
                if manifest.find(name).is_none() {
                    self.message(
                        Severity::Error,
                        "script",
                        format!("unknown script `{name}`"),
                    );
                    return;
                }
                let profiles = self.tier_names();
                if profiles.is_empty() {
                    self.message(Severity::Warning, "script", "No runtime tier is available");
                    return;
                }
                let items = profiles
                    .iter()
                    .enumerate()
                    .map(|(index, profile)| MenuItem::enabled(index.to_string(), profile.clone()))
                    .collect();
                self.overlays
                    .push(PickerOverlay::with_items("runtime tier", items));
                self.pending_overlay = Some(PendingOverlay::ScriptProfile {
                    name: name.clone(),
                    profiles,
                });
            }
            _ => self.message(
                Severity::Error,
                "script",
                "Usage: :script [list|new NAME|open NAME|tier NAME TIER]",
            ),
        }
    }

    /// 把脚本的运行时配置档写回清单。
    pub(in crate::app) fn apply_script_tier(&mut self, name: &str, profile: &str) {
        let mut manifest = match teal_workbench_services::ScriptManifest::load(&self.workspace) {
            Ok(manifest) => manifest,
            Err(error) => {
                self.message(Severity::Error, "script", error.to_string());
                return;
            }
        };
        let Some(mut entry) = manifest.find(name).cloned() else {
            self.message(
                Severity::Error,
                "script",
                format!("unknown script `{name}`"),
            );
            return;
        };
        entry.tier = profile.to_owned();
        manifest.upsert(entry);
        match manifest.save_with_workspace_transaction(
            &self.workspace,
            &self.buffers,
            &self.mutations,
        ) {
            Ok(()) => self.message(
                Severity::Info,
                "script",
                format!("Script `{name}` uses profile `{profile}`"),
            ),
            Err(error) => self.message(Severity::Error, "script", error.to_string()),
        }
    }

    /// 返回内置与配置中声明的运行时配置档名称。
    fn tier_names(&self) -> Vec<String> {
        ["embedded", "hosted", "agentic"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }
    fn list_scripts(&mut self, manifest: &teal_workbench_services::ScriptManifest) {
        if manifest.scripts().is_empty() {
            self.message(
                Severity::Info,
                "script",
                "No scripts are registered; use `:script new NAME`",
            );
            return;
        }
        let listing = render_scripts(manifest);
        // 脚本清单有独立视图：既有窗口就地刷新，否则新建。
        self.open_report_window(
            WindowKind::Scripts,
            WindowKind::Scripts.default_title(),
            &listing,
        );
    }

    pub(super) fn dispatch(&mut self, command: AppCommand) {
        match command {
            AppCommand::Open(path) => self.open_path(path),
            AppCommand::Save => self.save_focused(),
            AppCommand::SaveAs(path) => self.save_focused_as(&path),
            AppCommand::Close { panel, force } => self.close_panel(panel, force),
            AppCommand::Quit { force } => self.request_quit(force),
            AppCommand::Split { vertical } => self.split_scratch(vertical),
            AppCommand::WindowNew(kind) => self.open_window(kind),
            AppCommand::WindowFloat(kind) => self.open_float_window(kind),
            AppCommand::WindowToggleFloat => self.toggle_active_float(),
            AppCommand::WindowResize { direction, grow } => {
                self.resize_active_window(direction, grow);
            }
            AppCommand::WindowClose => self.close_active_window(false),
            AppCommand::WindowCycle { backwards } => {
                if backwards {
                    self.windows.select_previous();
                } else {
                    self.windows.select_next();
                }
            }
            AppCommand::WindowSelect(index) => {
                if !self.windows.select_index(index) {
                    self.message(Severity::Error, "window", "no such window");
                }
            }
            AppCommand::Command(line) => self.execute_command_line(&line),
            AppCommand::Focus(direction) => {
                self.windows.focus_direction(direction, self.last_area);
            }
            AppCommand::Explorer(path) => self.queue_open(path, true, None),
            AppCommand::Search { pattern, .. } => self.search_buffer(&pattern),
            AppCommand::Grep(pattern) => self.grep(&pattern),
            AppCommand::Browse(url) => self.browse(&url),
            AppCommand::BrowseBack => self.navigate_browser(false),
            AppCommand::BrowseForward => self.navigate_browser(true),
            AppCommand::BrowseExternal => {
                if let Err(error) = self.connectivity.require(
                    teal_workbench_services::agent_algorithms::ConnectivityCapability::Network,
                ) {
                    self.message(Severity::Error, "browser", error.to_string());
                    return;
                }
                if let Some(url) = self
                    .panels
                    .get(self.windows.focused())
                    .and_then(AppPanel::as_browser)
                    .map(|b| b.url().clone())
                    && let Err(error) = TextBrowser::open_url_external_with_audit(
                        &url,
                        &teal_security::process_context().into(),
                    )
                {
                    self.message(Severity::Error, "browser", error.to_string());
                }
            }
            AppCommand::BrowseExternalUrl(url) => {
                if let Err(error) = self.connectivity.require(
                    teal_workbench_services::agent_algorithms::ConnectivityCapability::Network,
                ) {
                    self.message(Severity::Error, "browser", error.to_string());
                    return;
                }
                match url::Url::parse(&url) {
                    Ok(url) => {
                        if let Err(error) = TextBrowser::open_url_external_with_audit(
                            &url,
                            &teal_security::process_context().into(),
                        ) {
                            self.message(Severity::Error, "browser", error.to_string());
                        }
                    }
                    Err(error) => self.message(Severity::Error, "browser", error.to_string()),
                }
            }
            AppCommand::Agent(prompt) => self.run_agent(prompt),
            AppCommand::Source(path) => {
                self.load_script(&path, ScriptKind::WorkspaceConfig, false);
            }
            AppCommand::Run(command) => self.run_process(&command),
            AppCommand::SetKeymap(preset) => self.set_keymap(preset),
            AppCommand::Messages => self.open_messages(),
            AppCommand::SaveSession => {
                if let Err(error) = self.save_session() {
                    self.message(Severity::Error, "session", error.to_string());
                } else {
                    self.message(Severity::Info, "session", "Session saved");
                }
            }
            AppCommand::Reload => self.message(
                Severity::Info,
                "config",
                "Reload requested; to explicitly execute the script use :source",
            ),
        }
    }
}

fn parse_memory_scope(value: &str) -> Option<MemoryScope> {
    match value {
        "user" => Some(MemoryScope::User),
        "workspace" => Some(MemoryScope::Workspace),
        _ => None,
    }
}

/// 模型辅助维护只允许提交有界的候选数量。
const MODEL_MAINTENANCE_CANDIDATES: u32 = 8;

/// 受限维护调用使用的固定指令；它不授予工具，也不产生交付证据。
const MAINTENANCE_INSTRUCTIONS: &str = "You extract durable long-term memory candidates from verified workspace records. Return only a JSON array of objects shaped {\"kind\":\"preference|fact|decision|procedure|pitfall\",\"content\":\"...\"}. Each content must be one short self-contained sentence, must not contain credentials, secrets, or private keys, and must not speculate beyond the provided records. Return [] when nothing is worth remembering.";

async fn submit_memory_maintenance(
    output: tokio::sync::mpsc::Sender<Message>,
    memory: Arc<MemoryStore>,
    request: teal_agent::MemoryMaintenanceRequest,
) {
    let result = crate::runtime_bridge::spawn_blocking(move || memory.maintain(request)).await;
    let messages = match result {
        Ok(Ok(report)) => vec![Message::new(
            Severity::Info,
            "memory",
            describe_maintenance(&report),
        )],
        Ok(Err(error)) => vec![Message::new(Severity::Error, "memory", error.to_string())],
        Err(error) => vec![Message::new(
            Severity::Error,
            "memory",
            format!("Memory maintenance task failed: {error}"),
        )],
    };
    for message in messages {
        if output.send(message).await.is_err() {
            break;
        }
    }
}

fn describe_maintenance(report: &teal_agent::MemoryMaintenanceReport) -> String {
    let mut text = format!(
        "Memory maintenance `{}` [{}] proposed {}, preserved {}, rejected {}, stale {}{}",
        report.run,
        match report.extraction {
            teal_agent::MemoryExtraction::Deterministic => "deterministic",
            teal_agent::MemoryExtraction::ModelAssisted => "model_assisted",
        },
        report.proposed,
        report.preserved,
        report.rejected,
        report.stale,
        report
            .budget
            .as_deref()
            .map_or(String::new(), |budget| format!("; budget {budget}")),
    );
    for entry in &report.entries {
        text.push_str(&format!(
            "\n- [{}] {}#{} {}{}",
            match entry.outcome {
                teal_agent::MemoryMaintenanceOutcome::Proposed => "proposed",
                teal_agent::MemoryMaintenanceOutcome::Preserved => "preserved",
                teal_agent::MemoryMaintenanceOutcome::Rejected => "rejected",
                teal_agent::MemoryMaintenanceOutcome::Stale => "stale",
            },
            entry.run,
            entry.event,
            entry
                .memory
                .as_deref()
                .map_or(String::new(), |id| format!("memory {id} ")),
            entry.detail.as_deref().unwrap_or(""),
        ));
    }
    if report.proposed > 0 {
        text.push_str("\nApprove candidates with :memory approve workspace <id>");
    }
    text
}

fn maintenance_prompt(sources: &[teal_agent::MemoryMaintenanceSource]) -> String {
    let mut prompt = String::from(
        "Extract durable long-term memory candidates from these verified workspace records. \
         Return only the JSON array described by the system instructions.\n",
    );
    for source in sources {
        prompt.push_str(&format!(
            "\n- source {}#{}: {}\n",
            source.run, source.event, source.candidate.content
        ));
    }
    prompt
}

fn parse_model_proposals(
    run: &str,
    proposals: &str,
) -> Option<Vec<teal_agent::MemoryMaintenanceSource>> {
    let text = proposals.trim();
    let text = text
        .strip_prefix("```json")
        .and_then(|text| text.strip_suffix("```"))
        .map_or(text, str::trim);
    let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
    let entries = parsed.as_array()?;
    let mut sources = Vec::new();
    for (index, entry) in entries
        .iter()
        .take(MODEL_MAINTENANCE_CANDIDATES as usize)
        .enumerate()
    {
        let kind = match entry.get("kind").and_then(serde_json::Value::as_str)? {
            "preference" => MemoryKind::Preference,
            "fact" => MemoryKind::Fact,
            "decision" => MemoryKind::Decision,
            "procedure" => MemoryKind::Procedure,
            "pitfall" => MemoryKind::Pitfall,
            _ => continue,
        };
        let content = entry
            .get("content")
            .and_then(serde_json::Value::as_str)?
            .trim()
            .to_owned();
        if content.is_empty() || content.len() > 2048 {
            continue;
        }
        let content_digest = teal_workbench_services::agent_algorithms::digest(content.as_bytes());
        sources.push(teal_agent::MemoryMaintenanceSource {
            run: run.to_owned(),
            event: format!("model/{index}"),
            digest: content_digest.clone(),
            candidate: MemoryCandidate {
                scope: MemoryScope::Workspace,
                kind,
                content,
                source_run: run.to_owned(),
                source_event: format!("model/{index}"),
                confidence: 0.6,
                verified: false,
                conflicts_with: None,
                evidence: Vec::new(),
                applicability: teal_agent::MemoryApplicability::default(),
                source_digest: content_digest,
                supersedes: None,
            },
        });
    }
    if sources.is_empty() {
        None
    } else {
        Some(sources)
    }
}

fn execute_memory_command(
    memory: &teal_agent::MemoryStore,
    command: MemoryCommand,
) -> Result<Vec<Message>, teal_agent::MemoryError> {
    let (summary, records) = match command {
        MemoryCommand::Candidates => {
            let records = memory.candidates(50)?;
            (
                format!("Memory candidates to be approved: {} items", records.len()),
                records,
            )
        }
        MemoryCommand::Find(query) => {
            let records = memory.recall(&query, 50)?;
            (
                format!("Recallable memories: {} items", records.len()),
                records,
            )
        }
        MemoryCommand::Approve(scope, id) => {
            memory.promote(
                scope,
                &id,
                vec![MemoryEvidence {
                    kind: MemoryEvidenceKind::UserStatement,
                    reference: format!("memory-approve:{id}"),
                    digest: id.to_owned(),
                }],
                MemoryApplicability::default(),
            )?;
            return Ok(vec![Message::new(
                Severity::Info,
                "memory",
                format!("Approved {} Memory {id}", scope.name()),
            )]);
        }
        // 维护走独立的异步入口；这里不会收到该命令。
        MemoryCommand::Maintain { .. } => return Ok(Vec::new()),
        MemoryCommand::Delete(scope, id) => {
            memory.delete(scope, &id)?;
            return Ok(vec![Message::new(
                Severity::Info,
                "memory",
                format!("Deleted {} memory {id}", scope.name()),
            )]);
        }
    };

    let mut messages = vec![Message::new(Severity::Info, "memory", summary)];
    messages.extend(
        records
            .iter()
            .map(|record| Message::new(Severity::Info, "memory", describe_memory(record))),
    );
    Ok(messages)
}

fn describe_memory(record: &MemoryRecord) -> String {
    format!(
        "[{}: {}] {}
ID: {}
Source: {}/{}; Confidence: {:.0}%",
        record.scope.name(),
        record.kind.name(),
        record.content,
        record.id,
        record.source_run,
        record.source_event,
        record.confidence * 100.0,
    )
}

/// 渲染磁盘文本与当前文本的行级差异；没有差异时给出明确说明。
fn render_text_diff(disk: &str, current: &str, path: &std::path::Path) -> String {
    let diff = similar::TextDiff::from_lines(disk, current);
    let mut lines = vec![format!("--- {}", path.display()), "+++ buffer".to_owned()];
    let mut added = 0_usize;
    let mut removed = 0_usize;
    for change in diff.iter_all_changes() {
        let (prefix, value) = match change.tag() {
            similar::ChangeTag::Delete => {
                removed += 1;
                ('-', change.value())
            }
            similar::ChangeTag::Insert => {
                added += 1;
                ('+', change.value())
            }
            similar::ChangeTag::Equal => (' ', change.value()),
        };
        lines.push(format!("{prefix}{}", value.trim_end_matches('\n')));
    }
    if added == 0 && removed == 0 {
        return format!(
            "No differences between {} and the open buffer.\n",
            path.display()
        );
    }
    lines.push(String::new());
    lines.push(format!("{added} added lines, {removed} removed lines"));
    lines.join("\n")
}
/// 渲染聚合诊断视图：按严重度计数，并逐条列出位置、来源与消息。
fn render_diagnostics(entries: &[teal_lsp_client::Diagnostic]) -> String {
    if entries.is_empty() {
        return "No diagnostics are recorded for the open documents.\n".to_owned();
    }
    let mut errors = 0_usize;
    let mut warnings = 0_usize;
    let mut other = 0_usize;
    let mut lines = Vec::with_capacity(entries.len() + 1);
    let mut ordered = entries.to_vec();
    ordered.sort_by(|left, right| {
        left.uri
            .cmp(&right.uri)
            .then_with(|| diagnostic_line(left).cmp(&diagnostic_line(right)))
            .then_with(|| left.severity.cmp(&right.severity))
    });
    for entry in &ordered {
        match entry.severity {
            1 => errors += 1,
            2 => warnings += 1,
            _ => other += 1,
        }
        let severity = match entry.severity {
            1 => "error",
            2 => "warning",
            3 => "info",
            _ => "hint",
        };
        let source = entry.source.as_deref().unwrap_or("analysis");
        lines.push(format!(
            "{}:{}:{} [{severity}] ({source}) {}",
            entry.uri,
            diagnostic_line(entry),
            diagnostic_column(entry),
            entry.message.replace('\n', " ")
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "{} diagnostics: {errors} errors, {warnings} warnings, {other} other",
        ordered.len()
    ));
    lines.join("\n")
}

fn diagnostic_line(entry: &teal_lsp_client::Diagnostic) -> u64 {
    entry
        .range_utf16
        .pointer("/start/line")
        .and_then(serde_json::Value::as_u64)
        .map_or(0, |line| line.saturating_add(1))
}

fn diagnostic_column(entry: &teal_lsp_client::Diagnostic) -> u64 {
    entry
        .range_utf16
        .pointer("/start/character")
        .and_then(serde_json::Value::as_u64)
        .map_or(0, |column| column.saturating_add(1))
}
/// 返回新建脚本的入口骨架：私有 `fn main`、整数退出码，并打印一行可发现的落点。
fn script_scaffold(name: &str) -> String {
    format!(
        "// {name} — created by Teal Bench; keep step-by-step work in this script.\n\
         fn main() -> int {{\n\
         \x20   print(\"hello from {name}\");\n\
         \x20   return 0;\n\
         }}\n"
    )
}

/// 把脚本清单渲染为可读视图文本。
///
/// 每行给出脚本名、运行时配置档、入口、权限文件、测试与产物计数，便于在 Scripts
/// 窗口中直接核对清单内容；不改变清单本体。
fn render_scripts(manifest: &teal_workbench_services::ScriptManifest) -> String {
    let mut lines = vec!["Workspace scripts".to_owned(), String::new()];
    for entry in manifest.scripts() {
        lines.push(format!("{}  [tier: {}]", entry.name, entry.tier));
        lines.push(format!("  entry: {}", entry.entry.display()));
        if let Some(permissions) = &entry.permissions {
            lines.push(format!("  permissions: {}", permissions.display()));
        }
        if !entry.tests.is_empty() {
            let tests = entry
                .tests
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("  tests: {tests}"));
        }
        if !entry.artifacts.is_empty() {
            let artifacts = entry
                .artifacts
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("  artifacts: {artifacts}"));
        }
        lines.push(String::new());
    }
    lines.push(format!("{} scripts registered", manifest.scripts().len()));
    lines.join("\n")
}

/// 把一条交付收据渲染为 Delivery 窗口的正文。
pub(super) fn render_delivery(
    flow: &str,
    receipt: &teal_workbench_services::DeliveryReceipt,
) -> String {
    let mut lines = vec![
        format!("Delivery record for flow {flow}"),
        String::new(),
        format!("summary: {}", receipt.summary),
        format!("runtime tier: {}", receipt.tier),
        format!("program digest: {}", receipt.program_digest),
        format!("accepted at (unix ms): {}", receipt.accepted_at_ms),
        String::new(),
        format!(
            "entry script: {} ({} bytes, {})",
            receipt.script.path, receipt.script.bytes, receipt.script.sha256
        ),
    ];
    if receipt.artifacts.is_empty() {
        lines.push("artifacts: none".to_owned());
    } else {
        lines.push("artifacts:".to_owned());
        for artifact in &receipt.artifacts {
            lines.push(format!(
                "  {} ({} bytes, {})",
                artifact.path, artifact.bytes, artifact.sha256
            ));
        }
    }
    if receipt.evidence.is_empty() {
        lines.push("evidence: none".to_owned());
    } else {
        lines.push("evidence:".to_owned());
        for item in &receipt.evidence {
            lines.push(format!("  {} : {}", item.call_id, item.claim));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_assisted_proposals_are_bounded_and_typed() {
        let run = "memory-maintenance/one";
        let accepted = parse_model_proposals(
            run,
            "```json\n[{\"kind\":\"fact\",\"content\":\"Teal uses \u{56db}\u{4e2a}\u{7a7a}\u{683c} indentation\"},{\"kind\":\"unknown\",\"content\":\"drop\"}]\n```",
        )
        .expect("usable candidate list");
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].candidate.kind, MemoryKind::Fact);
        assert!(!accepted[0].candidate.verified);
        assert_eq!(accepted[0].run, run);

        assert!(parse_model_proposals(run, "not json").is_none());
        assert!(parse_model_proposals(run, "[]").is_none());
        assert!(
            parse_model_proposals(
                run,
                &format!(
                    "[{{\"kind\":\"fact\",\"content\":\"{}\"}}]",
                    "x".repeat(2049)
                )
            )
            .is_none()
        );
    }

    #[test]
    fn maintenance_summary_reports_every_outcome() {
        let report = teal_agent::MemoryMaintenanceReport {
            run: "memory-maintenance/one".into(),
            extraction: teal_agent::MemoryExtraction::ModelAssisted,
            budget: Some("memory-maintenance/one".into()),
            source_digest: "digest".into(),
            proposed: 1,
            preserved: 1,
            rejected: 1,
            stale: 1,
            entries: vec![
                teal_agent::MemoryMaintenanceEntry {
                    run: "run".into(),
                    event: "event".into(),
                    digest: "digest".into(),
                    outcome: teal_agent::MemoryMaintenanceOutcome::Proposed,
                    memory: Some("memory-1".into()),
                    detail: None,
                },
                teal_agent::MemoryMaintenanceEntry {
                    run: "run".into(),
                    event: "other".into(),
                    digest: "digest".into(),
                    outcome: teal_agent::MemoryMaintenanceOutcome::Stale,
                    memory: None,
                    detail: Some("source changed".into()),
                },
            ],
            started_at_ms: 1,
            finished_at_ms: 2,
            replayed: false,
        };
        let text = describe_maintenance(&report);
        assert!(text.contains("model_assisted"));
        assert!(text.contains("budget memory-maintenance/one"));
        assert!(text.contains("[proposed] run#event memory memory-1"));
        assert!(text.contains("[stale] run#other"));
        assert!(text.contains(":memory approve workspace"));
    }
}
