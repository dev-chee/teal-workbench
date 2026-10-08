//! 窗口生命周期、标签切换与窗口级关闭确认。
//!
//! 新建窗口时先装配该工作流的初始面板，再把窗口插入集合；关闭窗口时先移除其中的
//! 面板与缓冲区身份，主窗口始终保留。

use super::*;

impl App {
    /// 返回 Agent 文本面板；源码 Agent 与扩展 Graph 事件共用该展示面板。
    pub(super) fn ensure_agent_panel(&mut self) -> PanelId {
        if let Some(panel) = self.agent_panel {
            return panel;
        }
        let panel = self
            .panels
            .insert(TextPanel::plain(PanelKind::Agent, "Session", "").into());
        self.agent_panel = Some(panel);
        panel
    }

    /// 将 RuntimeStore Graph 快照投影到 Agent 面板；只显示任务摘要，不展示 payload。
    pub(in crate::app) fn append_agent_graph(
        &mut self,
        run: String,
        snapshot: teal_agent::GraphSnapshot,
    ) {
        let panel = self.ensure_agent_panel();
        let edge_run = run.clone();
        let tasks = snapshot
            .tasks
            .iter()
            .map(|task| {
                format!(
                    "task={} agent={} status={} event={}",
                    task.id.get(),
                    task.agent.get(),
                    task.status.as_str(),
                    task.event
                )
            })
            .collect();
        let edges = snapshot
            .edges
            .iter()
            .map(|edge| {
                let kind = match edge.kind {
                    teal_agent::GraphEdgeKind::Delegation => "delegation",
                    teal_agent::GraphEdgeKind::Wait => "wait",
                    teal_agent::GraphEdgeKind::Sequence => "sequence",
                    teal_agent::GraphEdgeKind::Message => "message",
                };
                format!("{} -{}-> {}", edge.source.get(), kind, edge.target.get())
            })
            .collect::<Vec<_>>();
        let agents = snapshot
            .agents
            .iter()
            .map(|agent| {
                format!(
                    "id={} path={} definition={} status={} active_task={}",
                    agent.id.get(),
                    agent.path,
                    agent.definition,
                    agent.status.as_str(),
                    agent
                        .active_task
                        .map_or_else(|| "none".to_owned(), |task| task.get().to_string()),
                )
            })
            .collect::<Vec<_>>();
        if let Some(output) = self
            .panels
            .get_mut(panel)
            .and_then(|panel| panel.as_text_mut())
        {
            output.upsert_agent_graph(crate::presentation::AgentBlock::Graph {
                run,
                tasks,
                events: snapshot.events.len(),
            });
            if !edges.is_empty() {
                output.upsert_agent_graph_edges(crate::presentation::AgentBlock::GraphEdges {
                    run: edge_run.clone(),
                    edges,
                });
            }
            if !agents.is_empty() {
                output.upsert_agent_graph_agents(crate::presentation::AgentBlock::GraphAgents {
                    run: edge_run,
                    agents,
                });
            }
        }
    }

    /// 用真正的起始工作流替换启动骨架主窗口。
    ///
    /// 主窗口标题取工作区目录名以便多窗口标签区分；`start_file` 为显式传入的文件，
    /// 此时主窗口是编辑该文件的 Editor 窗口，否则是 Agent 的会话与输入面板。
    pub(super) fn install_primary_window(
        &mut self,
        start_file: Option<&std::path::Path>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let title = self
            .workspace
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| WindowKind::Agent.default_title().to_owned());
        // 骨架主窗口的面板在替换后成为孤儿；会话要求面板存储与窗口布局一一对应。
        let previous = self
            .windows
            .get(self.windows.primary())
            .and_then(|window| window.validate().ok())
            .unwrap_or_default();
        // 显式指定的起始文件在编辑器窗口中打开；目录与默认启动落到 Agent 主窗口。
        if let Some(file) = start_file {
            let layout = LayoutTree::new(self.open_editor_panel(file)?);
            let window = Window::new(WindowKind::Editor, title, layout);
            self.windows.replace_primary(window);
            self.drop_orphan_panels(&previous);
            return Ok(());
        }
        let session = self.agent_session_panel();
        let prompt = self.agent_prompt_panel();
        let spec = agent_layout(session, None, prompt);
        let window = Window::from_spec(WindowKind::Agent, title, spec)
            .expect("the Agent window template is valid");
        self.windows.replace_primary(window);
        self.drop_orphan_panels(&previous);
        Ok(())
    }

    /// 丢弃不再属于任何窗口的面板，保持存储与布局一一对应。
    fn drop_orphan_panels(&mut self, panels: &[PanelId]) {
        for panel in panels {
            if !self.windows.contains_panel(*panel) {
                self.panels.remove(*panel);
            }
        }
    }

    /// 在独立编辑器窗口中打开一个工作区文件。
    pub(crate) fn open_editor_window(
        &mut self,
        path: &std::path::Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let panel = self.open_editor_panel(path)?;
        let title = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| WindowKind::Editor.default_title().to_owned());
        self.windows.insert(Window::new(
            WindowKind::Editor,
            title,
            LayoutTree::new(panel),
        ));
        Ok(())
    }

    /// 打开一个文件的编辑器面板并登记缓冲区。
    fn open_editor_panel(
        &mut self,
        path: &std::path::Path,
    ) -> Result<PanelId, Box<dyn std::error::Error>> {
        let mut editor = open_editor(path, self.keymap, &self.grammars, &self.grammar_tables)?;
        editor.bind_buffer(&self.buffers.adopt(editor.document().clone()));
        if let Ok(canonical) = std::fs::canonicalize(path) {
            self.path_generations.insert(canonical, 0);
        }
        let panel = self.panels.insert(AppPanel::from(editor));
        self.ensure_recovery_identity(panel);
        Ok(panel)
    }

    /// 新建 Agent 会话面板并登记为当前对话宿主。
    pub(super) fn agent_session_panel(&mut self) -> PanelId {
        let session = self.panels.insert(AppPanel::from(TextPanel::plain(
            PanelKind::Agent,
            "Session",
            "",
        )));
        self.ensure_recovery_identity(session);
        self.agent_panel = Some(session);
        session
    }

    /// 新建 Agent 输入面板并填充上下文 chips。
    pub(super) fn agent_prompt_panel(&mut self) -> PanelId {
        let mut composer = AgentComposerWidget::new("Describe the goal");
        composer.set_chips(self.composer_chips());
        let prompt = self.panels.insert(AppPanel::from(composer));
        self.ensure_recovery_identity(prompt);
        prompt
    }

    /// 返回活动 Agent 窗口中的主输入框。
    pub(super) fn active_agent_composer(&self) -> Option<PanelId> {
        let window = self.windows.active();
        if window.kind() != WindowKind::Agent {
            return None;
        }
        window
            .validate()
            .ok()?
            .into_iter()
            .find(|panel| matches!(self.panels.get(*panel), Some(AppPanel::AgentComposer(_))))
    }

    /// 返回承载调试输出的 Debug 窗口与输出面板；缺失时新建。
    ///
    /// 调试会话绑定到窗口身份：已有 Debug 窗口就地复用，否则新建一个只有输出面板的
    /// Debug 窗口。窗口关闭后调用方清除绑定，事件不再写入已属于其他工作流的面板。
    pub(crate) fn bind_debug_session(&mut self) -> (WindowId, PanelId) {
        if let Some(window) = self.debug_window
            && self.windows.get(window).is_some()
            && let Some(panel) = self.debug_panel
        {
            self.windows.select(window);
            let _ = self.windows.focus(panel);
            return (window, panel);
        }
        let existing = self
            .windows
            .iter()
            .find(|(_, window)| window.kind() == WindowKind::Debug)
            .and_then(|(id, window)| {
                let source = window.validate().ok().and_then(|panels| {
                    panels.into_iter().find(|panel| {
                        self.panels
                            .get(*panel)
                            .is_some_and(|panel| panel.title() == "Source")
                    })
                });
                source.map(|panel| (id, panel))
            });
        if let Some((id, panel)) = existing {
            self.windows.select(id);
            self.debug_window = Some(id);
            self.debug_panel = Some(panel);
            return (id, panel);
        }
        let output = self.panels.insert(AppPanel::from(TextPanel::plain(
            PanelKind::Output,
            "Source",
            "",
        )));
        self.ensure_recovery_identity(output);
        let window = self.windows.insert(Window::new(
            WindowKind::Debug,
            "Debug",
            LayoutTree::new(output),
        ));
        self.debug_window = Some(window);
        self.debug_panel = Some(output);
        (window, output)
    }

    /// 新建窗口并聚焦。
    ///
    /// 每种工作流先装配自己的窗口模板：Agent 是「会话 + 输入框」的上下两面板，
    /// Debug 是「输出 + 控制台」，编辑工作流是一个未命名编辑器。
    pub(super) fn open_window(&mut self, kind: WindowKind) {
        let title = format!("{} {}", kind.default_title(), self.windows.len() + 1);
        let window = match kind {
            WindowKind::Agent => self.agent_window(title),
            WindowKind::Debug => self.debug_window(title),
            WindowKind::Tests => {
                let panels = self.tests_window();
                self.windows.select(panels.window);
                return;
            }
            WindowKind::Plan
            | WindowKind::Scripts
            | WindowKind::Delivery
            | WindowKind::Diagnostics
            | WindowKind::Diff => {
                let panel = self.panels.insert(AppPanel::from(TextPanel::plain(
                    PanelKind::Messages,
                    kind.default_title(),
                    "",
                )));
                self.ensure_recovery_identity(panel);
                Window::new(kind, title, LayoutTree::new(panel))
            }
            WindowKind::Editor => {
                let panel = self.scratch_editor_panel();
                Window::new(kind, title, LayoutTree::new(panel))
            }
        };
        self.windows.insert(window);
        self.message(
            Severity::Info,
            "window",
            format!("Opened {} window", kind.as_str()),
        );
    }

    /// 新建浮动窗口并聚焦。
    ///
    /// 浮动窗口使用内容区居中矩形；终端缩小时由渲染与拖动共同钳制回内容区。
    pub(super) fn open_float_window(&mut self, kind: WindowKind) {
        let rect = self.default_float_rect();
        let title = format!("{} {}", kind.default_title(), self.windows.len() + 1);
        let mut window = match kind {
            WindowKind::Agent => self.agent_window(title),
            WindowKind::Debug => self.debug_window(title),
            WindowKind::Tests => {
                let panels = self.tests_window();
                self.windows.select(panels.window);
                return;
            }
            WindowKind::Plan
            | WindowKind::Scripts
            | WindowKind::Delivery
            | WindowKind::Diagnostics
            | WindowKind::Diff => {
                let panel = self.panels.insert(AppPanel::from(TextPanel::plain(
                    PanelKind::Messages,
                    kind.default_title(),
                    "",
                )));
                self.ensure_recovery_identity(panel);
                Window::new(kind, title, LayoutTree::new(panel))
            }
            WindowKind::Editor => {
                let panel = self.scratch_editor_panel();
                Window::new(kind, title, LayoutTree::new(panel))
            }
        };
        window.set_placement(teal_workbench_core::WindowPlacement::Floating { rect, z: 1 });
        self.windows.insert(window);
        self.message(
            Severity::Info,
            "window",
            format!("Opened floating {} window", kind.as_str()),
        );
    }

    /// 在停靠与浮动之间切换活动窗口。
    pub(super) fn toggle_active_float(&mut self) {
        let id = self.windows.active_id();
        let rect = self.default_float_rect();
        if let Some(floating) = self.windows.toggle_placement(id, rect) {
            self.message(
                Severity::Info,
                "window",
                if floating {
                    "Window is floating"
                } else {
                    "Window is docked"
                },
            );
        }
    }

    /// 按固定步长调整活动浮动窗口的尺寸。
    ///
    /// `Horizontal` 改变宽度、`Vertical` 改变高度；两个方向都从右下锚点调整，并使用与
    /// 拖动相同的钳制与最小尺寸规则。窗口不在浮动状态或命令无实际效果时给出提示。
    pub(super) fn resize_active_window(&mut self, direction: Direction, grow: bool) {
        const STEP: i32 = 2;
        let id = self.windows.active_id();
        if self.windows.get(id).is_none_or(Window::is_docked) {
            self.message(
                Severity::Info,
                "window",
                "Only floating windows can be resized",
            );
            return;
        }
        let delta = if grow { STEP } else { -STEP };
        let (dx, dy) = match direction {
            Direction::Left | Direction::Right => (delta, 0),
            Direction::Up | Direction::Down => (0, delta),
        };
        let bounds = self.frame_geometry.content;
        let resized = self.windows.resize_floating_anchored(
            id,
            ResizeAnchor::BottomRight,
            dx,
            dy,
            bounds,
            super::events::WINDOW_MINIMUM,
        );
        match resized {
            Some(rect) => self.message(
                Severity::Info,
                "window",
                format!("Window resized to {}x{}", rect.width, rect.height),
            ),
            None => self.message(Severity::Info, "window", "Window is docked"),
        }
    }

    /// 把恢复会话的主窗口调整为 Agent 工作流。
    ///
    /// 保留原面板与布局，只改写窗口类型与标题。不注入新面板：会话恢复要求面板存储
    /// 与窗口布局一一对应，新增面板必须同时进入布局。
    /// 用户需要对话面板时用 `:window new agent` 新建窗口。
    pub(super) fn upgrade_primary_window(&mut self) {
        let primary = self.windows.primary();
        let promote = self
            .windows
            .get(primary)
            .is_some_and(|window| window.kind() != WindowKind::Agent);
        if !promote {
            return;
        }
        if let Some(window) = self.windows.get_mut(primary) {
            window.set_kind(WindowKind::Agent);
            window.set_title(WindowKind::Agent.default_title());
        }
    }

    /// 返回内容区内居中的默认浮动矩形。
    pub(super) fn default_float_rect(&self) -> Rect {
        const MARGIN: u16 = 4;
        let area = self.frame_geometry.content;
        let width = area
            .width
            .saturating_sub(MARGIN * 2)
            .max(40)
            .min(area.width);
        let height = area.height.saturating_sub(MARGIN).max(10).min(area.height);
        Rect {
            x: area.x.saturating_add(area.width.saturating_sub(width) / 2),
            y: area
                .y
                .saturating_add(area.height.saturating_sub(height) / 2),
            width,
            height,
        }
    }

    /// 打开或刷新 Plan 窗口：已有窗口就地替换正文，否则新建一个。
    pub(super) fn open_plan_window(&mut self, text: &str) {
        self.open_report_window(WindowKind::Plan, WindowKind::Plan.default_title(), text);
    }

    /// 请求当前源码 Agent 展示交付记录；交付事实由 RuntimeStore 与 Teal 程序提供。
    pub(super) fn open_delivery_window(&mut self) {
        self.submit_source_agent_input(":delivery".into());
    }

    /// 打开或刷新一个只读报告窗口：已有同类型窗口就地替换正文，否则新建一个。
    pub(super) fn open_report_window(&mut self, kind: WindowKind, title: &str, text: &str) {
        let existing = self
            .windows
            .iter()
            .find(|(_, window)| window.kind() == kind)
            .and_then(|(id, window)| {
                window
                    .validate()
                    .ok()
                    .and_then(|panels| panels.first().copied())
                    .map(|panel| (id, panel))
            });
        if let Some((window_id, panel)) = existing {
            if let Some(target) = self.panels.get_mut(panel).and_then(AppPanel::as_text_mut) {
                target.replace_plain(text);
            }
            self.windows.select(window_id);
            return;
        }
        let panel = self.panels.insert(AppPanel::from(TextPanel::plain(
            PanelKind::Messages,
            title,
            text,
        )));
        self.ensure_recovery_identity(panel);
        self.windows
            .insert(Window::new(kind, title, LayoutTree::new(panel)));
    }

    /// 装配 Agent 窗口模板：会话面板在上，输入框在下并随内容增长。
    fn agent_window(&mut self, title: String) -> Window {
        let session = self.agent_session_panel();
        let prompt = self.agent_prompt_panel();
        Window::from_spec(
            WindowKind::Agent,
            title,
            agent_layout(session, None, prompt),
        )
        .expect("the Agent window template is valid")
    }

    /// 装配 Debug 窗口模板：输出在上，控制台在下。
    fn debug_window(&mut self, title: String) -> Window {
        // 工作区上区再横拆为「Source ｜ Inspect」，Inspect 纵拆为「Variables / CallStack」。
        let source = self.debug_text_panel(PanelKind::Output, "Source");
        let variables = self.panels.insert(AppPanel::from(VariablesWidget::new()));
        self.ensure_recovery_identity(variables);
        let call_stack = self.panels.insert(AppPanel::from(CallStackWidget::new()));
        self.ensure_recovery_identity(call_stack);
        let console = self
            .panels
            .insert(AppPanel::from(DebugConsoleWidget::new()));
        self.ensure_recovery_identity(console);
        let inspect = NodeSpec::Split {
            axis: SplitAxis::Rows,
            children: vec![
                (PanelSize::fill(1).with_min(3), NodeSpec::Leaf(variables)),
                (PanelSize::cells(8).with_min(4), NodeSpec::Leaf(call_stack)),
            ],
        };
        let workspace = NodeSpec::Split {
            axis: SplitAxis::Columns,
            children: vec![
                (PanelSize::fill(3).with_min(40), NodeSpec::Leaf(source)),
                (PanelSize::cells(36).with_min(20), inspect),
            ],
        };
        let spec = LayoutSpec {
            focused: console,
            root: NodeSpec::Split {
                axis: SplitAxis::Rows,
                children: vec![
                    (PanelSize::fill(1).with_min(6), workspace),
                    (
                        PanelSize::cells(8).with_min(3).with_max(12),
                        NodeSpec::Leaf(console),
                    ),
                ],
            },
        };
        Window::from_spec(WindowKind::Debug, title, spec)
            .expect("the Debug window template is valid")
    }

    /// 新建一个只读调试文本面板并登记恢复标识。
    fn debug_text_panel(&mut self, kind: PanelKind, title: &str) -> PanelId {
        let panel = self
            .panels
            .insert(AppPanel::from(TextPanel::plain(kind, title, "")));
        self.ensure_recovery_identity(panel);
        panel
    }

    pub(super) fn scratch_editor_panel(&mut self) -> PanelId {
        let mut editor =
            EditorPanel::new(teal_workbench_core::Document::from_text(""), self.keymap);
        if let Some(grammar) = self.grammars.get("txt") {
            let _ = editor.set_grammar(grammar);
        }
        editor.bind_buffer(&self.buffers.adopt(editor.document().clone()));
        let panel = self.panels.insert(AppPanel::from(editor));
        self.ensure_recovery_identity(panel);
        panel
    }

    /// 返回输入框边框上的上下文 chips。
    pub(super) fn composer_chips(&self) -> Vec<String> {
        let agent = self
            .active_agent
            .clone()
            .unwrap_or_else(|| "unset".to_owned());
        let workspace = self
            .workspace
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.workspace.display().to_string());
        let mut chips = vec![format!("agent {agent}"), workspace];
        if let Ok(paths) = teal_config::ConfigPaths::discover(Some(&self.workspace))
            && let Ok(effective) = teal_config::load_declarations_paths(paths)
        {
            chips.push(format!(
                "permissions {:?}",
                effective.workspace_permissions().preset
            ));
        }
        chips
    }

    /// 让输入框的槽位高度跟随内容增长，保持上下限。
    pub(super) fn sync_composer_height(&mut self, panel: PanelId) {
        let Some(rows) = self
            .panels
            .get(panel)
            .and_then(AppPanel::as_composer)
            .map(ComposerWidget::preferred_rows)
        else {
            return;
        };
        let size = PanelSize::cells(rows).with_min(3).with_max(10);
        if self.windows.panel_size(panel) != Some(size) {
            let _ = self.windows.set_panel_size(panel, size);
        }
    }

    /// 关闭活动窗口；主窗口与含未保存缓冲区的窗口先请求确认。
    pub(super) fn close_active_window(&mut self, force: bool) {
        let id = self.windows.active_id();
        if id == self.windows.primary() {
            self.message(
                Severity::Warning,
                "window",
                "The main window cannot be closed",
            );
            return;
        }
        let panels = match self.windows.get(id).map(Window::validate) {
            Some(Ok(panels)) => panels,
            Some(Err(error)) => {
                self.message(Severity::Error, "window", error.to_string());
                return;
            }
            None => return,
        };
        let dirty = panels.iter().any(|panel| {
            self.panels
                .get(*panel)
                .and_then(AppPanel::as_editor)
                .is_some_and(|editor| editor.document().is_dirty())
        });
        if dirty && !force {
            self.overlays
                .push(ConfirmOverlay::new("Unsaved changes", "Close this window?"));
            self.pending_overlay = Some(PendingOverlay::Confirmation(
                PendingConfirmation::CloseWindow(id),
            ));
            return;
        }
        self.finish_close_window(id, &panels);
    }

    pub(super) fn finish_close_window(&mut self, id: WindowId, panels: &[PanelId]) {
        // 调试会话绑定到承载输出的窗口；窗口关闭即结束会话，避免工作线程继续运行并把
        // 事件写入已经属于其他工作流的面板。
        if self.debug_window == Some(id) {
            self.stop_debug_session("Debugging session stopped because its window was closed");
        }
        if self.windows.close(id).is_err() {
            return;
        }
        for panel in panels {
            self.release_panel(*panel);
        }
        self.message(Severity::Info, "window", "Window closed");
    }

    /// 按标签条命中位置聚焦窗口。
    pub(super) fn select_tab(&mut self, column: u16, row: u16) -> bool {
        let Some(id) = self.frame_geometry.tab_at(column, row) else {
            return false;
        };
        self.windows.select(id)
    }
}

/// 构造 Agent 主窗口的布局模板。
///
/// 会话面板占据上方剩余高度；提供编辑器面板时把它放在会话与输入框之间；输入框固定
/// 3–10 行并随内容增长。焦点始终落在输入框。
fn agent_layout(session: PanelId, file: Option<PanelId>, prompt: PanelId) -> LayoutSpec {
    let mut children = vec![(PanelSize::fill(1).with_min(6), NodeSpec::Leaf(session))];
    if let Some(file) = file {
        children.push((PanelSize::fill(2).with_min(4), NodeSpec::Leaf(file)));
    }
    children.push((
        PanelSize::cells(3).with_min(3).with_max(10),
        NodeSpec::Leaf(prompt),
    ));
    LayoutSpec {
        focused: prompt,
        root: NodeSpec::Split {
            axis: SplitAxis::Rows,
            children,
        },
    }
}
