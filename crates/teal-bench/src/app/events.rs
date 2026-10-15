//! 终端输入、运行时结果、浮层与剪贴板的应用级路由。
//!
//! 输入按浮层和命令行优先级消费；鼠标使用最近渲染的几何，并保留拖动最初的所有者。
//! 后台结果在此回到应用状态，不允许工作线程直接操作布局或面板存储。

use super::{
    App, AppCommand, AppPanel, ClipboardAction, CmdlineAction, CmdlineMode, ContextMenu,
    ContextMenuOverlay, ContextTarget, Direction, Event, InputEvent, Instant, Key, KeyEventKind,
    MenuItem, MouseButton, MouseCapture, MouseEvent, MouseEventKind, OverlayAction, PanelId,
    PendingConfirmation, PendingOverlay, PendingPrompt, Prompt, RuntimeEvent, Severity, Widget,
    WidgetAction, WindowKind, lsp, normalize_key, tui_rect,
};

use crate::workbench::ui::WindowFramePart;

/// 浮动窗口的最小宽高；缩到该下界以下时窗口转为停靠。
pub(super) const WINDOW_MINIMUM: (u16, u16) = (40, 10);

impl App {
    pub(super) fn pump_approvals(&mut self) {
        self.pump_extension_approvals();
        if self.pending_overlay.is_none()
            && let Some(mutation) = self.pending_mutations.pop_front()
        {
            self.show_agent_mutation(mutation);
            return;
        }
        if self.pending_overlay.is_none()
            && let Some(change_set) = self.pending_changesets.pop_front()
        {
            self.show_agent_changeset(change_set);
            return;
        }
        if self.pending_overlay.is_none()
            && let Some(prompt) = self.pending_approvals.pop_front()
        {
            self.show_approval(prompt);
        }
    }

    fn show_approval(&mut self, prompt: teal_workbench_services::tools::ApprovalPrompt) {
        if self.should_quit {
            let _ = prompt.response.send(teal_agent::ApprovalResponse::Deny);
            return;
        }

        let allow_task = prompt
            .permission
            .scopes
            .contains(&teal_agent::ApprovalScope::Task);
        let allow_workspace = prompt
            .permission
            .scopes
            .contains(&teal_agent::ApprovalScope::Workspace);
        let default_scope = if allow_task {
            "the current task"
        } else {
            "this operation"
        };
        self.append_agent_approval(
            prompt.permission.binding.digest.clone(),
            prompt.call.name.clone(),
            prompt
                .permission
                .resources
                .iter()
                .map(|resource| format!("{resource:?}"))
                .collect(),
            None,
        );
        self.overlays.push(crate::workbench::ui::PermissionOverlay::new(
            "Agent approval",
            format!(
                "Tool: {}\nArguments: {}\nResources: {:?}\nChoose how long to allow this exact scope. Enter defaults to {}.",
                prompt.call.name,
                prompt.call.arguments,
                prompt.permission.resources,
                default_scope
            ),
            allow_task,
            allow_workspace,
        ));
        self.pending_overlay = Some(PendingOverlay::Confirmation(PendingConfirmation::Agent(
            Box::new(prompt),
        )));
    }

    pub(super) fn finish_agent_approval(
        &mut self,
        prompt: teal_workbench_services::tools::ApprovalPrompt,
        response: teal_agent::ApprovalResponse,
    ) {
        let decision = match response {
            teal_agent::ApprovalResponse::Deny => "denied",
            teal_agent::ApprovalResponse::Allow(teal_agent::ApprovalScope::Once) => "allowed-once",
            teal_agent::ApprovalResponse::Allow(teal_agent::ApprovalScope::Task) => "allowed-task",
            teal_agent::ApprovalResponse::Allow(teal_agent::ApprovalScope::Workspace) => {
                "allowed-workspace"
            }
        };
        self.append_agent_approval(
            prompt.permission.binding.digest.clone(),
            prompt.call.name.clone(),
            prompt
                .permission
                .resources
                .iter()
                .map(|resource| format!("{resource:?}"))
                .collect(),
            Some(decision.into()),
        );
        let _ = prompt.response.send(response);
    }

    /// 将宿主批准请求投影为当前 Agent 面板的展示块；批准事实仍由宿主账本保存。
    pub(in crate::app) fn append_agent_approval(
        &mut self,
        id: String,
        action: String,
        resources: Vec<String>,
        decision: Option<String>,
    ) {
        let panel = self.ensure_agent_panel();
        if let Some(output) = self
            .panels
            .get_mut(panel)
            .and_then(|panel| panel.as_text_mut())
        {
            output.upsert_agent_approval(crate::presentation::AgentBlock::Approval {
                id,
                action,
                resources,
                decision,
            });
        }
    }

    pub(super) async fn handle_runtime_event(&mut self, event: RuntimeEvent) {
        let mut event = event;
        let mut context = crate::runtime_bridge::Context::current();
        while let RuntimeEvent::Observed {
            context: observed,
            event: inner,
        } = event
        {
            event = *inner;
            context = observed;
        }

        // 转发的批准和修改请求保留原工具上下文。
        match &event {
            RuntimeEvent::Mutation(request) => context = request.observation.clone(),
            RuntimeEvent::ChangeSet(request) => context = request.observation.clone(),
            RuntimeEvent::Approval(prompt) => context = prompt.observation.clone(),
            _ => {}
        }

        context.instrument(self.handle_observed_event(event)).await;
    }

    async fn handle_observed_event(&mut self, event: RuntimeEvent) {
        match event {
            RuntimeEvent::ExtensionLoaded(result) => self.extension_loaded(result),
            RuntimeEvent::ExtensionEventFinished(result) => self.finish_extension_event(result),
            RuntimeEvent::ExtensionAuthenticationReady {
                extension,
                key,
                result,
            } => self.finish_extension_authentication(&extension, &key, result),
            RuntimeEvent::ExtensionTaskFinished {
                extension,
                key,
                result,
            } => self.finish_extension_task(&extension, &key, result),
            RuntimeEvent::ExtensionActivity { extension, event } => {
                self.extension_activity(&extension, event)
            }
            RuntimeEvent::Observed { .. } => {
                unreachable!("the event boundary no longer uses envelope wrapping")
            }
            RuntimeEvent::LspResolved { request, result } => {
                self.finish_lsp_resolve(request, result);
            }
            RuntimeEvent::InlineCompleted(result) => self.finish_inline(result),
            RuntimeEvent::AgentCodeFinished { index, result } => {
                self.finish_agent_code(index, result)
            }
            RuntimeEvent::AgentGraphLoaded { run, result } => {
                self.finish_agent_graph_load(run, result)
            }
            RuntimeEvent::TextLoaded { result, axis } => match result {
                Ok(panel) => {
                    self.add_panel(panel, axis);
                }
                Err(error) => self.message(Severity::Error, "background", error),
            },
            RuntimeEvent::PathLoaded {
                result,
                path,
                generation,
                address,
                position,
            } => {
                self.finish_open(result, path, generation, address, position)
                    .await
            }
            RuntimeEvent::AgentAppEvent(event) => self.handle_source_agent_event(event),
            RuntimeEvent::AgentAppReady(services) => {
                self.agent_app_services = Some(*services);
                // 源码 Agent 已完成 RuntimeStore 装配；后续 Graph/运行管理查询复用同一账本。
                if let Ok(store) = teal_agent::RuntimeStore::connect(&self.workspace) {
                    self.agent_session_store = Some(std::sync::Arc::new(store));
                }
            }
            RuntimeEvent::AgentAppBaseline { reply } => {
                let workspace = self.workspace.clone();
                let overlays = self
                    .buffers
                    .iter()
                    .filter_map(|buffer| Some((buffer.path()?.to_path_buf(), buffer.to_string())))
                    .collect::<Vec<_>>();
                self.tasks.spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        teal_workbench_services::agent_algorithms::ConversationBaseline::capture_with_overlays(
                            workspace, &overlays,
                        )
                        .map_err(|error| error.to_string())
                        .and_then(|baseline| {
                            serde_json::to_string(&baseline).map_err(|error| error.to_string())
                        })
                    })
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result);
                    let _ = reply.send(result);
                });
            }
            RuntimeEvent::AgentAppWaiting(request) => self.show_source_agent_waiting(request),
            RuntimeEvent::AgentAppStopped(result) => self.finish_source_agent_app(result),
            RuntimeEvent::Debug(event) => self.handle_debug_event(event),
            RuntimeEvent::TestsFinished {
                path,
                filter,
                outcome,
            } => self.finish_tests(path, filter, outcome),
            RuntimeEvent::ScriptFinished(result) => self.finish_script(result),
            RuntimeEvent::WorkspaceFinished(result) => self.finish_mutation(result),
            RuntimeEvent::BrowserLoaded {
                panel,
                sequence,
                result,
            } => {
                if self.browser_requests.get(&panel) != Some(&sequence) {
                    return;
                }
                self.browser_requests.remove(&panel);
                match result {
                    Ok(document) => {
                        if let Some(browser) = self
                            .panels
                            .get_mut(panel)
                            .and_then(AppPanel::as_browser_mut)
                        {
                            browser.replace(document);
                        }
                    }
                    Err(error) => self.message(Severity::Error, "browser", error),
                }
            }
            RuntimeEvent::Approval(prompt) => {
                if self.should_quit || self.pending_approvals.len() >= 16 {
                    let _ = prompt.response.send(teal_agent::ApprovalResponse::Deny);
                } else if self.pending_overlay.is_some() {
                    self.pending_approvals.push_back(prompt);
                } else {
                    self.show_approval(prompt);
                }
            }
            RuntimeEvent::Mutation(mutation) => {
                if self.should_quit || self.pending_mutations.len() >= 16 {
                    let _ = mutation
                        .response
                        .send(Err("Agent change review queue is full".into()));
                } else {
                    self.pending_mutations.push_back(mutation);
                    self.pump_approvals();
                }
            }
            RuntimeEvent::ChangeSet(change_set) => {
                if self.should_quit || self.pending_changesets.len() >= 16 {
                    let _ = change_set
                        .response
                        .send(Err("ChangeSet review queue is full".into()));
                } else {
                    self.pending_changesets.push_back(change_set);
                    self.pump_approvals();
                }
            }
            RuntimeEvent::RuntimeLockSaved { lock, result } => match result {
                Ok(()) => {
                    self.runtime_lock_status = lock.status_with_catalog_identity(
                        env!("CARGO_PKG_VERSION"),
                        &self.runtime_packages,
                        &self.runtime_core_catalog_digest,
                        self.runtime_extension_catalog_digest.as_deref(),
                        teal::PACKAGE_FORMAT_VERSION,
                        &self.runtime_cache,
                    );
                    self.runtime_lock = Some(lock);
                    self.message(Severity::Info, "runtime", "Project runtime lock updated");
                }
                Err(error) => self.message(Severity::Error, "runtime", error),
            },
            RuntimeEvent::Message(message) => self.messages.push(message),
            RuntimeEvent::Lsp(client, event) => self.handle_lsp_event(client, event).await,
            RuntimeEvent::LspResponse(response) => self.handle_lsp_response(response),
            RuntimeEvent::LspStarted { language, result } => {
                self.attach_lsp(language, result).await;
            }
        }
    }

    pub(super) fn handle_terminal_event(&mut self, event: &Event) {
        let old_focus = self.windows.focused();
        match event {
            Event::Resize(width, height) => {
                self.last_area.width = *width;
                self.last_area.height = *height;
                let ids = self
                    .panels
                    .iter()
                    .filter(|(_, panel)| panel.as_custom().is_some())
                    .map(|(id, _)| id)
                    .collect::<Vec<_>>();
                for id in ids {
                    self.queue_panel_event(
                        id,
                        std::collections::BTreeMap::from([
                            ("kind".into(), "resize".into()),
                            ("width".into(), width.to_string()),
                            ("height".into(), height.to_string()),
                        ]),
                    );
                }
            }
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if let Some(input) = normalize_key(key.code, key.modifiers) {
                    self.handle_input(input);
                }
            }
            Event::Mouse(mouse) => {
                let mouse = self.mouse_normalizer.normalize(*mouse, Instant::now());
                self.handle_mouse(mouse);
            }
            _ => {}
        }
        if old_focus != self.windows.focused() {
            self.fire_hook(
                "panel.focused",
                serde_json::json!({"panel":format!("{:?}",self.windows.focused())}),
            );
        }
    }

    pub(super) fn handle_input(&mut self, input: InputEvent) {
        if matches!(
            self.pending_overlay,
            Some(PendingOverlay::DurableQuestion { .. })
        ) && input.modifiers.control
            && input.key == Key::Char('c')
        {
            let _ = self.overlays.handle_input(InputEvent {
                key: Key::Escape,
                modifiers: Default::default(),
            });
            self.pending_overlay = None;
            self.handle_overlay_action(OverlayAction::Consumed);
            return;
        }
        // 模态输入优先消费，不再传给全局键位或聚焦面板。
        if !self.overlays.is_empty() {
            let action = self.overlays.handle_input(input);
            self.handle_overlay_action(action);
            return;
        }

        if self.cmdline.is_some() && self.handle_completion_key(input) {
            return;
        }
        if let Some(action) = self.cmdline.as_mut().map(|cmdline| cmdline.handle(input)) {
            let changed = matches!(action, CmdlineAction::Changed);
            match action {
                CmdlineAction::Submit { mode, text } => {
                    self.cmdline_history = self
                        .cmdline
                        .as_ref()
                        .map(|cmdline| cmdline.history().to_vec())
                        .unwrap_or_default();
                    self.cmdline = None;
                    self.completion_menu.clear();
                    match mode {
                        CmdlineMode::Command => self.execute_command_line(&text),
                        CmdlineMode::SearchForward | CmdlineMode::SearchBackward => {
                            self.dispatch(AppCommand::Search {
                                pattern: text,
                                backwards: mode == CmdlineMode::SearchBackward,
                            });
                        }
                        CmdlineMode::Prompt => self.finish_prompt(&text),
                    }
                }
                CmdlineAction::Cancel => {
                    self.cmdline = None;
                    self.pending_prompt = None;
                    self.pending_model_profile_name = None;
                    self.completion_menu.clear();
                }
                CmdlineAction::Menu(menu) => self.open_context_menu(menu, ContextTarget::Cmdline),
                CmdlineAction::Clipboard(action) => self.apply_cmdline_clipboard(action),
                CmdlineAction::Changed | CmdlineAction::Ignored => {}
            }
            if changed {
                self.refresh_command_completions();
            }
            return;
        }

        let focused = self.windows.focused();
        let focused_panel = self.panels.get(focused);
        let allow_global_prompt = focused_panel.is_none_or(AppPanel::allows_global_prompt);
        // 输入框自己解释行首前缀，但窗口前缀与面板拆分键仍然可用。
        let allow_window_keys = focused_panel.is_none_or(AppPanel::allows_window_keys);
        if input.modifiers.control && input.key == Key::Char(' ') {
            self.request_lsp(lsp::RequestKind::Complete);
            return;
        }
        if allow_global_prompt {
            let action = match input.key {
                Key::Char('K') => Some(lsp::RequestKind::Hover),
                Key::Function(12) if input.modifiers.shift => Some(lsp::RequestKind::References),
                Key::Function(12) => Some(lsp::RequestKind::Definition),
                Key::Enter if input.modifiers.alt => Some(lsp::RequestKind::CodeAction),
                Key::Function(2) => {
                    self.start_lsp_rename();
                    return;
                }
                _ => None,
            };
            if let Some(action) = action {
                self.request_lsp(action);
                return;
            }
        }
        if allow_window_keys && self.window_prefix {
            self.window_prefix = false;
            let command = match input.key {
                // h/l 调整宽度、j/k 调整高度；窗口循环改用 `Ctrl-w w`。
                Key::Char('h') => Some(AppCommand::WindowResize {
                    direction: Direction::Left,
                    grow: false,
                }),
                Key::Char('l') => Some(AppCommand::WindowResize {
                    direction: Direction::Right,
                    grow: true,
                }),
                Key::Char('k') => Some(AppCommand::WindowResize {
                    direction: Direction::Up,
                    grow: false,
                }),
                Key::Char('j') => Some(AppCommand::WindowResize {
                    direction: Direction::Down,
                    grow: true,
                }),
                Key::Char('w') => Some(AppCommand::WindowCycle { backwards: false }),
                Key::Char('n') => Some(AppCommand::WindowNew(WindowKind::Editor)),
                Key::Char('a') => Some(AppCommand::WindowNew(WindowKind::Agent)),
                Key::Char('d') => Some(AppCommand::WindowNew(WindowKind::Debug)),
                Key::Char('f') => Some(AppCommand::WindowFloat(WindowKind::Editor)),
                Key::Char('m') => Some(AppCommand::WindowToggleFloat),
                Key::Char('c' | 'q') => Some(AppCommand::WindowClose),
                Key::Char(digit @ '1'..='9') => digit
                    .to_digit(10)
                    .map(|index| AppCommand::WindowSelect(index as usize)),
                _ => None,
            };
            if let Some(command) = command {
                self.dispatch(command);
            }
            return;
        }
        if allow_window_keys && input.modifiers.control && input.key == Key::Char('w') {
            self.window_prefix = true;
            return;
        }
        // 面板焦点与拆分使用 Alt 前缀；Ctrl-w 保留给窗口工作流命令。
        if allow_window_keys && input.modifiers.alt {
            let command = match input.key {
                Key::Char('h') | Key::Left => Some(AppCommand::Focus(Direction::Left)),
                Key::Char('j') | Key::Down => Some(AppCommand::Focus(Direction::Down)),
                Key::Char('k') | Key::Up => Some(AppCommand::Focus(Direction::Up)),
                Key::Char('l') | Key::Right => Some(AppCommand::Focus(Direction::Right)),
                Key::Char('s') => Some(AppCommand::Split { vertical: false }),
                Key::Char('v') => Some(AppCommand::Split { vertical: true }),
                _ => None,
            };
            if let Some(command) = command {
                self.dispatch(command);
                return;
            }
        }
        if allow_global_prompt {
            match input.key {
                Key::Char(':') => {
                    // Agent 窗口只有一个统一输入入口：即使会话区当前聚焦，冒号也回到
                    // 主输入框中编辑，避免命令文本跳到底部的旧全局命令行。
                    if let Some(composer) = self.active_agent_composer() {
                        let _ = self.windows.focus(composer);
                        let action = self
                            .panels
                            .get_mut(composer)
                            .map_or(WidgetAction::Ignored, |panel| panel.handle_input(input));
                        self.apply_panel_action(composer, action);
                        self.sync_composer_height(composer);
                        return;
                    }
                    self.cmdline = Some(Prompt::with_history(
                        CmdlineMode::Command,
                        self.cmdline_history.clone(),
                    ));
                    self.refresh_command_completions();
                    return;
                }
                Key::Char('/') => {
                    self.cmdline = Some(Prompt::with_history(
                        CmdlineMode::SearchForward,
                        self.cmdline_history.clone(),
                    ));
                    return;
                }
                Key::Char('?') => {
                    self.cmdline = Some(Prompt::with_history(
                        CmdlineMode::SearchBackward,
                        self.cmdline_history.clone(),
                    ));
                    return;
                }
                _ => {}
            }
        }
        // 面板可能直接修改缓冲区；应用其动作后须排空这些变化。
        let focused = self.windows.focused();
        let before = self.editor_snapshot(focused);
        let action = self
            .panels
            .get_mut(focused)
            .map_or(WidgetAction::Ignored, |panel| panel.handle_input(input));
        self.apply_panel_action(focused, action);
        self.sync_composer_height(focused);
        self.journal_if_changed(focused, before);
    }

    pub(super) fn handle_mouse(&mut self, mouse: MouseEvent) {
        if !self.overlays.is_empty() {
            let area = tui_rect(self.frame_geometry.full);
            let action = self.overlays.handle_mouse(mouse, area);
            self.handle_overlay_action(action);
            return;
        }

        if self.cmdline.is_some() {
            let Some(area) = self.frame_geometry.command_line else {
                return;
            };
            if !area.contains(mouse.column, mouse.row) {
                return;
            }
            let action = self
                .cmdline
                .as_mut()
                .map_or(CmdlineAction::Ignored, |cmdline| {
                    cmdline.handle_mouse(mouse, area)
                });
            match action {
                CmdlineAction::Menu(menu) => self.open_context_menu(menu, ContextTarget::Cmdline),
                CmdlineAction::Clipboard(action) => self.apply_cmdline_clipboard(action),
                CmdlineAction::Cancel => self.cmdline = None,
                CmdlineAction::Changed | CmdlineAction::Ignored | CmdlineAction::Submit { .. } => {}
            }
            return;
        }

        // 即使指针跨越面板边界，拖动仍由最初的面板拥有。
        if let Some(capture) = self.mouse_capture {
            match capture {
                MouseCapture::Divider(divider) => {
                    if matches!(
                        mouse.kind,
                        MouseEventKind::Drag(MouseButton::Left)
                            | MouseEventKind::Up(MouseButton::Left)
                    ) {
                        let _ = self.windows.resize_divider(
                            divider,
                            self.frame_geometry.content,
                            mouse.column,
                            mouse.row,
                            12,
                            4,
                        );
                    }
                    if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
                        self.mouse_capture = None;
                    }
                    return;
                }
                MouseCapture::Panel(panel) => {
                    if matches!(
                        mouse.kind,
                        MouseEventKind::Drag(MouseButton::Left)
                            | MouseEventKind::Up(MouseButton::Left)
                    ) {
                        if let Some(geometry) = self.frame_geometry.panel(panel) {
                            let before = self.editor_snapshot(panel);
                            let action = self
                                .panels
                                .get_mut(panel)
                                .map_or(WidgetAction::Ignored, |target| {
                                    target.handle_mouse(mouse, tui_rect(geometry.body))
                                });
                            self.apply_panel_action(panel, action);
                            self.journal_if_changed(panel, before);
                        }
                        if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
                            self.mouse_capture = None;
                        }
                        return;
                    }
                }
                MouseCapture::WindowMove { window, origin } => {
                    if matches!(
                        mouse.kind,
                        MouseEventKind::Drag(MouseButton::Left)
                            | MouseEventKind::Up(MouseButton::Left)
                    ) {
                        let dx = i32::from(mouse.column) - i32::from(origin.0);
                        let dy = i32::from(mouse.row) - i32::from(origin.1);
                        let _ = self.windows.move_floating(
                            window,
                            dx,
                            dy,
                            self.frame_geometry.content,
                            WINDOW_MINIMUM,
                        );
                    }
                    if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
                        self.mouse_capture = None;
                    }
                    return;
                }
                MouseCapture::WindowResize {
                    window,
                    anchor,
                    origin,
                } => {
                    if matches!(
                        mouse.kind,
                        MouseEventKind::Drag(MouseButton::Left)
                            | MouseEventKind::Up(MouseButton::Left)
                    ) {
                        let dx = i32::from(mouse.column) - i32::from(origin.0);
                        let dy = i32::from(mouse.row) - i32::from(origin.1);
                        let _ = self.windows.resize_floating_anchored(
                            window,
                            anchor,
                            dx,
                            dy,
                            self.frame_geometry.content,
                            WINDOW_MINIMUM,
                        );
                    }
                    if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
                        self.mouse_capture = None;
                    }
                    return;
                }
            }
        }

        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.select_tab(mouse.column, mouse.row)
        {
            return;
        }

        // 浮动窗口的标题栏与缩放手柄先于面板命中；被动窗口不接收焦点。
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some((window, part)) =
                self.frame_geometry.window_frame_at(mouse.column, mouse.row)
        {
            if self
                .windows
                .get(window)
                .is_some_and(|target| target.is_draggable())
            {
                let _ = self.windows.select(window);
                let origin = (mouse.column, mouse.row);
                self.mouse_capture = Some(match part {
                    WindowFramePart::Chrome => MouseCapture::WindowMove { window, origin },
                    WindowFramePart::Resize(anchor) => MouseCapture::WindowResize {
                        window,
                        anchor,
                        origin,
                    },
                });
            }
            return;
        }

        // 点击浮动窗口正文把它提到最上层并聚焦。
        if matches!(
            mouse.kind,
            MouseEventKind::Down(MouseButton::Left | MouseButton::Right | MouseButton::Middle)
        ) && let Some(geometry) = self.frame_geometry.window_at(mouse.column, mouse.row)
        {
            let _ = self.windows.select(geometry.window);
        }

        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some(divider) =
                self.windows
                    .divider_at(self.frame_geometry.content, mouse.column, mouse.row)
        {
            self.mouse_capture = Some(MouseCapture::Divider(divider));
            return;
        }

        let Some(geometry) = self.frame_geometry.allocation_at(mouse.column, mouse.row) else {
            return;
        };
        if matches!(
            mouse.kind,
            MouseEventKind::Down(MouseButton::Left | MouseButton::Right | MouseButton::Middle)
        ) {
            let _ = self.windows.focus(geometry.panel);
        }
        if !geometry.body.contains(mouse.column, mouse.row) {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Right)) {
                self.open_context_menu(
                    ContextMenu {
                        x: mouse.column,
                        y: mouse.row,
                        items: Vec::new(),
                    },
                    ContextTarget::Panel(geometry.panel),
                );
            }
            return;
        }
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            self.mouse_capture = Some(MouseCapture::Panel(geometry.panel));
        }
        let before = self.editor_snapshot(geometry.panel);
        let action = self
            .panels
            .get_mut(geometry.panel)
            .map_or(WidgetAction::Ignored, |panel| {
                panel.handle_mouse(mouse, tui_rect(geometry.body))
            });
        self.apply_panel_action(geometry.panel, action);
        self.journal_if_changed(geometry.panel, before);
    }

    pub(super) fn apply_panel_action(&mut self, panel: PanelId, action: WidgetAction) {
        match action {
            WidgetAction::CustomEvent(event) => self.queue_panel_event(panel, event),
            WidgetAction::Commands(commands) => {
                for command in commands {
                    self.dispatch(command);
                }
            }
            WidgetAction::Menu(menu) => {
                self.open_context_menu(menu, ContextTarget::Panel(panel));
            }
            WidgetAction::Clipboard(action) => self.apply_panel_clipboard(panel, action),
            WidgetAction::WriteClipboard(text) => {
                let _ = self.write_clipboard(&text);
            }
            WidgetAction::Consumed | WidgetAction::Ignored => {}
        }
    }

    pub(super) fn open_context_menu(&mut self, mut menu: ContextMenu, target: ContextTarget) {
        if let ContextTarget::Panel(panel) = target {
            menu.items.extend([
                MenuItem::enabled("panel.split_horizontal", "Split horizontally"),
                MenuItem::enabled("panel.split_vertical", "Split vertically"),
                MenuItem::enabled("panel.close", "Close panel"),
            ]);
            let _ = self.windows.focus(panel);
        }
        self.overlays.push(ContextMenuOverlay::new(menu));
        self.pending_overlay = Some(PendingOverlay::Context(target));
    }

    pub(super) fn handle_overlay_action(&mut self, action: OverlayAction) {
        match action {
            OverlayAction::Accepted(value) => match self.pending_overlay.take() {
                Some(PendingOverlay::DurableQuestion { store, request }) => {
                    self.answer_durable_question(store, request, Some(value));
                }
                Some(PendingOverlay::Lsp(picker)) => {
                    if let Ok(index) = value.parse::<usize>() {
                        self.accept_lsp_choice(&picker, index);
                    }
                }
                Some(PendingOverlay::ChangeSet(request)) => {
                    self.finish_change_set_review(*request, Some(&value));
                }
                Some(PendingOverlay::Confirmation(confirmation)) => {
                    if let PendingConfirmation::Agent(prompt) = confirmation {
                        let response = match value.as_str() {
                            "once" => {
                                teal_agent::ApprovalResponse::Allow(teal_agent::ApprovalScope::Once)
                            }
                            "task" => {
                                teal_agent::ApprovalResponse::Allow(teal_agent::ApprovalScope::Task)
                            }
                            "workspace" => teal_agent::ApprovalResponse::Allow(
                                teal_agent::ApprovalScope::Workspace,
                            ),
                            _ => teal_agent::ApprovalResponse::Deny,
                        };
                        self.finish_agent_approval(*prompt, response);
                    } else {
                        self.finish_confirmation(confirmation, value == "yes");
                    }
                }
                Some(PendingOverlay::BufferPicker(panels)) => {
                    if let Ok(index) = value.parse::<usize>()
                        && let Some(panel) = panels.get(index)
                    {
                        let _ = self.windows.focus(*panel);
                    }
                }
                Some(PendingOverlay::ScriptProfile { name, profiles }) => {
                    if let Ok(index) = value.parse::<usize>()
                        && let Some(profile) = profiles.get(index)
                    {
                        self.apply_script_tier(&name, profile);
                    }
                }
                Some(PendingOverlay::Context(target)) => {
                    self.execute_context_action(target, &value);
                }
                Some(PendingOverlay::AgentPicker) => self.select_agent(&value),
                Some(PendingOverlay::ModelProviderPicker { pending_request }) => {
                    self.select_model_provider(&value, pending_request);
                }
                Some(PendingOverlay::ModelAuthProfilePicker {
                    pending_request,
                    provider,
                }) => self.select_model_auth_profile(&provider, &value, pending_request),
                Some(PendingOverlay::ModelPicker {
                    pending_request,
                    provider,
                    auth_profile,
                }) => self.select_model_model(&provider, &auth_profile, &value, pending_request),
                Some(PendingOverlay::ModelProfilePicker {
                    pending_request,
                    selection,
                }) => self.select_model_profile(&selection, &value, pending_request),
                None => {}
            },
            OverlayAction::Cancelled => match self.pending_overlay.take() {
                Some(PendingOverlay::DurableQuestion { store, request }) => {
                    self.answer_durable_question(store, request, None);
                }
                Some(PendingOverlay::ChangeSet(request)) => {
                    self.finish_change_set_review(*request, None);
                }
                Some(PendingOverlay::Confirmation(confirmation)) => {
                    if let PendingConfirmation::Agent(prompt) = confirmation {
                        self.finish_agent_approval(*prompt, teal_agent::ApprovalResponse::Deny);
                    } else {
                        self.finish_confirmation(confirmation, false);
                    }
                }
                Some(
                    PendingOverlay::ModelProviderPicker { .. }
                    | PendingOverlay::ModelAuthProfilePicker { .. }
                    | PendingOverlay::ModelPicker { .. }
                    | PendingOverlay::ModelProfilePicker { .. },
                ) => {
                    self.pending_inline_completion = None;
                    self.message(Severity::Info, "agent", "Model selection cancelled");
                }
                _ => {}
            },
            OverlayAction::SensitiveAccepted(_)
            | OverlayAction::Consumed
            | OverlayAction::Ignored => {}
        }
    }

    pub(super) fn execute_context_action(&mut self, target: ContextTarget, id: &str) {
        let clipboard = match id {
            "clipboard.copy" => Some(ClipboardAction::Copy),
            "clipboard.cut" => Some(ClipboardAction::Cut),
            "clipboard.paste" => Some(ClipboardAction::Paste),
            "clipboard.select_all" => Some(ClipboardAction::SelectAll),
            _ => None,
        };
        if let Some(action) = clipboard {
            match target {
                ContextTarget::Panel(panel) => self.apply_panel_clipboard(panel, action),
                ContextTarget::Cmdline => self.apply_cmdline_clipboard(action),
            }
            return;
        }
        let ContextTarget::Panel(panel) = target else {
            return;
        };
        let _ = self.windows.focus(panel);
        match id {
            "panel.split_horizontal" => self.dispatch(AppCommand::Split { vertical: false }),
            "panel.split_vertical" => self.dispatch(AppCommand::Split { vertical: true }),
            "panel.close" => {
                self.dispatch(AppCommand::Close {
                    panel,
                    force: false,
                });
            }
            "explorer.rename" => {
                self.start_explorer_prompt(panel, PendingPrompt::Rename(panel));
            }
            "explorer.copy_to" => {
                self.start_explorer_prompt(panel, PendingPrompt::CopyTo(panel));
            }
            "explorer.move_to" => {
                self.start_explorer_prompt(panel, PendingPrompt::MoveTo(panel));
            }
            "explorer.trash" => self.confirm_explorer_action(panel, false),
            "explorer.delete" => self.confirm_explorer_action(panel, true),
            _ => {
                let before = self.editor_snapshot(panel);
                let action = self
                    .panels
                    .get_mut(panel)
                    .map_or(WidgetAction::Ignored, |target| target.handle_menu(id));
                self.apply_panel_action(panel, action);
                self.journal_if_changed(panel, before);
            }
        }
    }

    pub(super) fn apply_panel_clipboard(&mut self, panel: PanelId, action: ClipboardAction) {
        if action == ClipboardAction::Cut {
            // 成功复制后再删除文本，避免剪贴板失败造成选择内容丢失。
            let text = self
                .panels
                .get_mut(panel)
                .and_then(|target| target.handle_clipboard(ClipboardAction::Copy, None).write);
            let Some(text) = text else {
                return;
            };
            if !self.write_clipboard(&text) {
                return;
            }

            let before = self.editor_snapshot(panel);
            if let Some(target) = self.panels.get_mut(panel) {
                let _ = target.handle_clipboard(ClipboardAction::Cut, None);
            }
            self.journal_if_changed(panel, before);
            return;
        }

        let paste = if action == ClipboardAction::Paste {
            match self.clipboard.read_text() {
                Ok(text) => Some(text),
                Err(error) => {
                    self.message(Severity::Error, "clipboard", error.to_string());
                    return;
                }
            }
        } else {
            None
        };

        let before = self.editor_snapshot(panel);
        let outcome = self
            .panels
            .get_mut(panel)
            .map(|target| target.handle_clipboard(action, paste.as_deref()));
        if let Some(outcome) = outcome {
            if let Some(text) = outcome.write {
                let _ = self.write_clipboard(&text);
            }
            if outcome.consumed {
                self.journal_if_changed(panel, before);
            }
        }
    }

    pub(super) fn apply_cmdline_clipboard(&mut self, action: ClipboardAction) {
        if action == ClipboardAction::Cut {
            let text = self
                .cmdline
                .as_mut()
                .and_then(|cmdline| cmdline.handle_clipboard(ClipboardAction::Copy, None).write);
            let Some(text) = text else {
                return;
            };
            if self.write_clipboard(&text)
                && let Some(cmdline) = &mut self.cmdline
            {
                let _ = cmdline.handle_clipboard(ClipboardAction::Cut, None);
            }
            return;
        }
        let paste = if action == ClipboardAction::Paste {
            match self.clipboard.read_text() {
                Ok(text) => Some(text),
                Err(error) => {
                    self.message(Severity::Error, "clipboard", error.to_string());
                    return;
                }
            }
        } else {
            None
        };
        let outcome = self
            .cmdline
            .as_mut()
            .map(|cmdline| cmdline.handle_clipboard(action, paste.as_deref()));
        if let Some(Some(text)) = outcome.map(|outcome| outcome.write) {
            let _ = self.write_clipboard(&text);
        }
    }

    pub(super) fn write_clipboard(&mut self, text: &str) -> bool {
        if let Err(error) = self.clipboard.write_text(text) {
            self.message(Severity::Error, "clipboard", error.to_string());
            false
        } else {
            true
        }
    }
}
