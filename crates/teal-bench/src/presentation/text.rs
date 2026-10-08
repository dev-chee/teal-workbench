//! Markdown、搜索、消息和进程输出的只读文本视图。
//!
//! 面板保存滚动和选择状态，外部操作通过应用动作发出；显示文本不替代类型化智能体会话。

use super::{
    Block, Borders, ClipboardAction, ClipboardOutcome, Color, Frame, InputEvent, Key, Line,
    MarkdownDocument, MarkdownStyle, MouseEvent, Widget, WidgetAction, PanelKind, PanelStatus,
    Paragraph, Rect, SearchMatch, Style, TextViewState, plain_lines, render_lines_with_selection,
};
use serde::{Deserialize, Serialize};

/// Agent buffer 中可独立渲染、筛选和操作的内容块。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentBlock {
    /// Markdown 或普通对话文本。
    Markdown(String),

    /// 带语言标识的代码片段。
    Code { language: String, source: String },

    /// Agent 发起的工具调用。
    ToolCall { name: String, arguments: String },

    /// 工具返回的结构化结果。
    ToolResult {
        name: String,
        text: String,
        checkpoint: Option<String>,
    },

    /// 编译器、语言服务或测试产生的诊断。
    Diagnostic { summary: String, detail: String },

    /// 所有符号引用关系。
    References { source: String, items: Vec<String> },

    /// 词法 Token 流。
    Tokens { source: String, items: Vec<String> },

    /// 带可选源位置引用的诊断，用于测试失败自动定位和诊断跳转。
    SourceDiagnostic {
        summary: String,
        detail: String,
        source: Option<String>,
        position: Option<u32>,
    },

    /// Cargo 诊断关联的源码、符号、Git 和相邻文件上下文。
    DiagnosticContext {
        /// 工作区相对源码路径。
        source: String,
        /// 从一开始的行列。
        line: u32,
        column: u32,
        /// Teal 源码中可解析时的符号。
        symbol: Option<String>,
        /// 该行最近一次已提交来源。
        commit: Option<String>,
        /// 提交主题。
        commit_summary: Option<String>,
        /// 有界源码上下文。
        context: Vec<String>,
        /// 同一 Cargo 诊断中的其他源码范围。
        related: Vec<String>,
    },

    /// 带来源、版本和独立 hunk 身份的完整变更集。
    ChangeSetReview(teal_workbench_core::ChangeSet),

    /// 等待用户决定或已经完成的审批请求。
    Approval {
        id: String,
        action: String,
        resources: Vec<String>,
        decision: Option<String>,
    },

    /// 不属于对话正文的生命周期状态。
    Status(String),

    /// Task run 的阶段和终态时间线事件。
    Task {
        id: String,
        state: String,
        phase: Option<String>,
        detail: Option<String>,
    },

    /// Task 交付使用的脱敏工具证据摘要。
    Evidence { task: String, items: Vec<String> },

    /// GraphRuntime 的只读任务摘要。
    Graph {
        run: String,
        tasks: Vec<String>,
        events: usize,
    },

    /// GraphRuntime 的只读有向边摘要。
    GraphEdges { run: String, edges: Vec<String> },

    /// GraphRuntime 的 Agent 所有权树摘要。
    GraphAgents { run: String, agents: Vec<String> },

    /// Teal 源文件的直接模块导入摘要。
    ModuleImports {
        source: String,
        imports: Vec<String>,
    },

    /// 按源码顺序的调用者列表。
    Callers {
        source: String,
        symbol: String,
        callers: Vec<String>,
    },

    /// 按源码顺序的被调用者列表。
    Callees {
        source: String,
        symbol: String,
        callees: Vec<String>,
    },

    /// 指定字节位置可见的符号。
    Visible {
        source: String,
        position: u32,
        symbols: Vec<String>,
    },

    /// 指定字节位置的类型信息。
    Type {
        source: String,
        position: u32,
        name: String,
        kind: String,
        detail: Option<String>,
    },

    /// 一个符号的声明与全部引用位置。
    Occurrences {
        source: String,
        symbol: String,
        spans: Vec<String>,
    },

    /// 源文件全部声明符号的摘要索引。
    Symbols {
        source: String,
        symbols: Vec<String>,
    },

    /// 从录制品文件加载的任务回放摘要。
    TaskReplay {
        id: String,
        agent: String,
        state: String,
        history_count: usize,
        recorded_at: String,
    },

    /// 与工作区及 Git 内容快照绑定的对话分支。
    ConversationBranch {
        /// 工作区内唯一的分支名。
        id: String,
        /// 可选来源分支。
        parent: Option<String>,
        /// 创建时的 Git 分支。
        git_branch: String,
        /// 创建时的 Git HEAD。
        git_head: String,
        /// 当前基线状态。
        state: String,
        /// 分支中保存的模型消息数。
        history_count: usize,
    },

    /// 一次三方合并预览及其结构化冲突摘要。
    Merge {
        /// 解决结果准备写入的工作区相对路径。
        target: String,
        /// 仍需人工处理的冲突数。
        conflicts: usize,
        /// 三份输入绑定摘要。
        input_digest: String,
    },

    /// Teal 编译器生成的结构、类型、值流或副作用关系图。
    AnalysisGraph {
        /// 工作区相对源码路径。
        source: String,
        /// `ast`、`types`、`data` 或 `effects`。
        view: String,
        /// 有界节点摘要。
        nodes: Vec<String>,
        /// 有界边摘要。
        edges: Vec<String>,
    },
}

impl AgentBlock {
    fn lines(&self) -> Vec<String> {
        match self {
            Self::Markdown(text) | Self::Status(text) => {
                if text.is_empty() {
                    vec![String::new()]
                } else {
                    text.lines().map(str::to_owned).collect()
                }
            }
            Self::Code { language, source } => {
                let mut lines = vec![format!("```{language}")];
                lines.extend(source.lines().map(str::to_owned));
                lines.push("```".to_owned());
                lines
            }
            Self::ToolCall { name, arguments } => vec![format!("[Tool] {name} {arguments}")],
            Self::ToolResult {
                name,
                text,
                checkpoint,
            } => {
                let suffix = checkpoint
                    .as_deref()
                    .map_or(String::new(), |value| format!(" checkpoint={value}"));
                vec![format!("[Tool: {name}] {text}{suffix}")]
            }
            Self::References { source, items } => {
                let mut lines = vec![format!("[References] {source} count={}", items.len())];
                lines.extend(items.iter().map(|r| format!("- {r}")));
                lines
            }
            Self::Tokens { source, items } => {
                let mut lines = vec![format!("[Tokens] {source} count={}", items.len())];
                lines.extend(items.iter().map(|t| format!("- {t}")));
                lines
            }
            Self::SourceDiagnostic {
                summary,
                detail,
                source,
                position,
            } => {
                let location = match (&source, position) {
                    (Some(path), Some(pos)) => format!(" {path}:{pos}"),
                    (Some(path), None) => format!(" {path}"),
                    _ => String::new(),
                };
                let mut lines = vec![format!("[Diagnostic{location}] {summary}")];
                if !detail.is_empty() {
                    lines.extend(detail.lines().map(str::to_owned));
                }
                lines
            }
            Self::Diagnostic { summary, detail } => {
                let mut lines = vec![format!("[Diagnostic] {summary}")];
                if !detail.is_empty() {
                    lines.extend(detail.lines().map(str::to_owned));
                }
                lines
            }
            Self::DiagnosticContext {
                source,
                line,
                column,
                symbol,
                commit,
                commit_summary,
                context,
                related,
            } => {
                let mut lines = vec![format!("[Diagnostic context] {source}:{line}:{column}")];
                if let Some(symbol) = symbol {
                    lines.push(format!("- symbol: {symbol}"));
                }
                if let Some(commit) = commit {
                    lines.push(format!(
                        "- commit: {} {}",
                        &commit[..commit.len().min(12)],
                        commit_summary.as_deref().unwrap_or("")
                    ));
                }
                lines.extend(context.iter().map(|line| format!("  {line}")));
                lines.extend(related.iter().map(|item| format!("- related: {item}")));
                lines
            }
            Self::ChangeSetReview(change_set) => {
                let mut lines = vec![format!(
                    "[ChangeSet] {} files={} generation={} source={}",
                    change_set.id,
                    change_set.files.len(),
                    change_set.generation,
                    change_set.source_task.as_deref().unwrap_or("interactive")
                )];
                for file in &change_set.files {
                    lines.push(format!(
                        "- {} {} -> {} hunks={}",
                        file.path.display(),
                        file.original_sha256,
                        file.new_sha256,
                        file.hunks.len()
                    ));
                    lines.extend(file.diff.lines().map(str::to_owned));
                }
                lines
            }
            Self::Approval {
                id,
                action,
                resources,
                decision,
            } => {
                let state = decision.as_deref().unwrap_or("pending");
                let mut lines = vec![format!("[Approval] {action} state={state} id={id}")];
                lines.extend(resources.iter().map(|resource| format!("- {resource}")));
                lines
            }
            Self::Task {
                id,
                state,
                phase,
                detail,
            } => {
                let mut line = format!("[Task] {id} state={state}");
                if let Some(phase) = phase {
                    line.push_str(&format!(" phase={phase}"));
                }
                if let Some(detail) = detail {
                    line.push_str(&format!(" {detail}"));
                }
                vec![line]
            }
            Self::Evidence { task, items } => {
                let mut lines = vec![format!("[Evidence] {task}")];
                lines.extend(items.iter().map(|item| format!("- {item}")));
                lines
            }
            Self::Graph { run, tasks, events } => {
                let mut lines = vec![format!(
                    "[Graph] {run} tasks={} events={events}",
                    tasks.len()
                )];
                lines.extend(tasks.iter().map(|task| format!("- {task}")));
                lines
            }
            Self::GraphEdges { run, edges } => {
                let mut lines = vec![format!("[Graph edges] {run} count={}", edges.len())];
                lines.extend(edges.iter().map(|edge| format!("- {edge}")));
                lines
            }
            Self::GraphAgents { run, agents } => {
                let mut lines = vec![format!("[Graph agents] {run} count={}", agents.len())];
                lines.extend(agents.iter().map(|agent| format!("- {agent}")));
                lines
            }
            Self::ModuleImports { source, imports } => {
                let mut lines = vec![format!("[Module imports] {source} count={}", imports.len())];
                lines.extend(imports.iter().map(|import| format!("- {import}")));
                lines
            }
            Self::Callers {
                source,
                symbol,
                callers,
            } => {
                let mut lines = vec![format!(
                    "[Callers] {symbol} source={source} count={}",
                    callers.len()
                )];
                lines.extend(callers.iter().map(|c| format!("- {c}")));
                lines
            }
            Self::Callees {
                source,
                symbol,
                callees,
            } => {
                let mut lines = vec![format!(
                    "[Callees] {symbol} source={source} count={}",
                    callees.len()
                )];
                lines.extend(callees.iter().map(|c| format!("- {c}")));
                lines
            }
            Self::Visible {
                source,
                position,
                symbols,
            } => {
                let mut lines = vec![format!(
                    "[Visible] pos={position} source={source} count={}",
                    symbols.len()
                )];
                lines.extend(symbols.iter().map(|s| format!("- {s}")));
                lines
            }
            Self::Type {
                source,
                position,
                name,
                kind,
                detail,
            } => {
                let detail = detail.as_deref().unwrap_or("");
                vec![format!(
                    "[Type] {name} ({kind}) pos={position} source={source} detail={detail}"
                )]
            }
            Self::Occurrences {
                source,
                symbol,
                spans,
            } => {
                let mut lines = vec![format!(
                    "[Occurrences] {symbol} source={source} count={}",
                    spans.len()
                )];
                lines.extend(spans.iter().map(|s| format!("- {s}")));
                lines
            }
            Self::Symbols { source, symbols } => {
                let mut lines = vec![format!("[Symbols] {source} count={}", symbols.len())];
                lines.extend(symbols.iter().map(|s| format!("- {s}")));
                lines
            }
            Self::TaskReplay {
                id,
                agent,
                state,
                history_count,
                recorded_at,
            } => {
                vec![format!(
                    "[Task Replay] {id} agent={agent} state={state} history={history_count} recorded={recorded_at}"
                )]
            }
            Self::ConversationBranch {
                id,
                parent,
                git_branch,
                git_head,
                state,
                history_count,
            } => vec![format!(
                "[Conversation Branch] {id} parent={} git={git_branch}@{} state={state} history={history_count}",
                parent.as_deref().unwrap_or("root"),
                &git_head[..git_head.len().min(12)]
            )],
            Self::Merge {
                target,
                conflicts,
                input_digest,
            } => vec![format!(
                "[Three-way Merge] target={target} conflicts={conflicts} input={}",
                &input_digest[..input_digest.len().min(12)]
            )],
            Self::AnalysisGraph {
                source,
                view,
                nodes,
                edges,
            } => {
                let mut lines = vec![format!(
                    "[Analysis {view}] {source} nodes={} edges={}",
                    nodes.len(),
                    edges.len()
                )];
                lines.extend(nodes.iter().map(|node| format!("- node {node}")));
                lines.extend(edges.iter().map(|edge| format!("- edge {edge}")));
                lines
            }
        }
    }
}

/// 用于 Markdown、搜索、日志和输出的可滚动文本类面板。
pub struct TextPanel {
    kind: PanelKind,
    title: String,
    lines: Vec<Line<'static>>,
    view: TextViewState,
    resource: String,
    agent_blocks: Vec<AgentBlock>,
}

impl TextPanel {
    pub(crate) fn session_state(&self) -> teal_workbench_services::PanelSessionState {
        teal_workbench_services::PanelSessionState::Text {
            category: self.kind.as_str().into(),
            title: self.title.clone(),
            content: plain_lines(&self.lines).join("\n"),
            blocks: (self.kind == PanelKind::Agent)
                .then(|| serde_json::to_value(self.session_agent_blocks()).ok())
                .flatten(),
            resource: self.resource().to_owned(),
            scroll: self.view.vertical,
            column: self.view.horizontal,
        }
    }

    pub(crate) fn restore_view(&mut self, resource: String, scroll: usize, column: usize) {
        self.resource = resource;
        self.view.vertical = scroll.min(self.lines.len().saturating_sub(1));
        self.view.horizontal = column;
    }

    /// 用新的纯文本替换正文，并清空结构化 Agent 块与滚动位置。
    pub(crate) fn replace_plain(&mut self, text: &str) {
        self.lines = text
            .lines()
            .map(|line| Line::from(line.to_owned()))
            .collect();
        self.agent_blocks.clear();
        self.resource.clear();
        self.view.vertical = 0;
        self.view.horizontal = 0;
    }

    /// 恢复经过会话边界保存的 Agent block；无效负载保持当前纯文本后备视图。
    pub(crate) fn restore_agent_blocks(&mut self, blocks: Option<&serde_json::Value>) -> bool {
        if self.kind != PanelKind::Agent {
            return false;
        }
        let Some(blocks) = blocks else {
            return false;
        };
        let Ok(blocks) = serde_json::from_value::<Vec<AgentBlock>>(blocks.clone()) else {
            return false;
        };
        self.agent_blocks = blocks;
        self.rebuild_agent_lines();
        true
    }

    fn session_agent_blocks(&self) -> Vec<AgentBlock> {
        self.agent_blocks
            .iter()
            .cloned()
            .map(|mut block| {
                match &mut block {
                    AgentBlock::ToolCall { arguments, .. } => redact_json_text(arguments),
                    AgentBlock::ToolResult { text, .. } => redact_json_text(text),
                    AgentBlock::Approval { decision, .. } if decision.is_none() => {
                        *decision = Some("interrupted".into());
                    }
                    _ => {}
                }
                block
            })
            .collect()
    }

    /// 创建纯文本内容。
    pub fn plain(kind: PanelKind, title: impl Into<String>, text: impl AsRef<str>) -> Self {
        let text = text.as_ref();
        Self {
            kind,
            title: title.into(),
            lines: text.lines().map(|line| diff_line(kind, line)).collect(),
            view: TextViewState::default(),
            resource: String::new(),
            agent_blocks: if kind == PanelKind::Agent && !text.is_empty() {
                vec![AgentBlock::Markdown(text.to_owned())]
            } else {
                Vec::new()
            },
        }
    }

    /// 创建带样式的 Markdown 内容。
    pub fn markdown(title: impl Into<String>, document: &MarkdownDocument) -> Self {
        let lines = document
            .lines
            .iter()
            .map(|line| {
                let style = match line.style {
                    MarkdownStyle::Heading(_) => Style::default().fg(Color::Cyan),
                    MarkdownStyle::Code => Style::default().fg(Color::Green),
                    MarkdownStyle::Quote => Style::default().fg(Color::DarkGray),
                    MarkdownStyle::ListItem | MarkdownStyle::Paragraph | MarkdownStyle::Table => {
                        Style::default()
                    }
                };
                Line::styled(line.text.clone(), style)
            })
            .collect();
        Self {
            kind: PanelKind::Markdown,
            title: title.into(),
            lines,
            view: TextViewState::default(),
            resource: String::new(),
            agent_blocks: Vec::new(),
        }
    }

    /// 创建工作区搜索结果。
    pub fn search(query: &str, matches: &[SearchMatch]) -> Self {
        let lines = matches
            .iter()
            .map(|found| {
                Line::from(format!(
                    "{}:{}:{}: {}",
                    found
                        .path
                        .as_ref()
                        .map_or("<buffer>".into(), |path| path.to_string_lossy()),
                    found.line,
                    found.column + 1,
                    found.line_text
                ))
            })
            .collect();
        Self {
            kind: PanelKind::Search,
            title: format!("Search: {query}"),
            lines,
            view: TextViewState::default(),
            resource: query.to_owned(),
            agent_blocks: Vec::new(),
        }
    }

    /// 追加一行活动记录。
    pub fn push(&mut self, line: impl Into<String>) {
        self.lines.push(Line::from(line.into()));
    }

    /// 追加一个 Agent buffer 结构化内容块并同步其兼容文本视图。
    pub fn push_agent_block(&mut self, block: AgentBlock) {
        let lines = block.lines();
        self.lines.extend(lines.into_iter().map(Line::from));
        if self.kind == PanelKind::Agent {
            self.agent_blocks.push(block);
        }
    }

    /// 更新同一 Graph run 的最新摘要，避免每个工具事件重复堆叠快照。
    pub fn upsert_agent_graph(&mut self, block: AgentBlock) {
        if self.kind != PanelKind::Agent {
            return;
        }
        let replace = match (&self.agent_blocks.last(), &block) {
            (Some(AgentBlock::Graph { run: existing, .. }), AgentBlock::Graph { run, .. }) => {
                existing == run
            }
            _ => false,
        };
        if replace {
            *self.agent_blocks.last_mut().expect("last block exists") = block;
            self.rebuild_agent_lines();
        } else {
            self.push_agent_block(block);
        }
    }

    /// 更新同一 Graph run 的边摘要，避免工具事件重复堆叠拓扑信息。
    pub fn upsert_agent_graph_edges(&mut self, block: AgentBlock) {
        if self.kind != PanelKind::Agent {
            return;
        }
        let replace = match (&self.agent_blocks.last(), &block) {
            (
                Some(AgentBlock::GraphEdges { run: existing, .. }),
                AgentBlock::GraphEdges { run, .. },
            ) => existing == run,
            _ => false,
        };
        if replace {
            *self.agent_blocks.last_mut().expect("last block exists") = block;
            self.rebuild_agent_lines();
        } else {
            self.push_agent_block(block);
        }
    }

    /// 更新同一 Graph run 的 Agent 所有权摘要。
    pub fn upsert_agent_graph_agents(&mut self, block: AgentBlock) {
        if self.kind != PanelKind::Agent {
            return;
        }
        let replace = match (&self.agent_blocks.last(), &block) {
            (
                Some(AgentBlock::GraphAgents { run: existing, .. }),
                AgentBlock::GraphAgents { run, .. },
            ) => existing == run,
            _ => false,
        };
        if replace {
            *self.agent_blocks.last_mut().expect("last block exists") = block;
            self.rebuild_agent_lines();
        } else {
            self.push_agent_block(block);
        }
    }

    /// 更新同一源文件的模块导入摘要，避免重复堆叠分析结果。
    pub fn upsert_agent_module_imports(&mut self, block: AgentBlock) {
        if self.kind != PanelKind::Agent {
            return;
        }
        let replace = match (&self.agent_blocks.last(), &block) {
            (
                Some(AgentBlock::ModuleImports {
                    source: existing, ..
                }),
                AgentBlock::ModuleImports { source, .. },
            ) => existing == source,
            _ => false,
        };
        if replace {
            *self.agent_blocks.last_mut().expect("last block exists") = block;
            self.rebuild_agent_lines();
        } else {
            self.push_agent_block(block);
        }
    }

    /// 更新同一源文件与符号的调用者摘要。
    pub fn upsert_agent_callers(&mut self, block: AgentBlock) {
        if self.kind != PanelKind::Agent {
            return;
        }
        let replace = match (&self.agent_blocks.last(), &block) {
            (
                Some(AgentBlock::Callers {
                    source: existing, ..
                }),
                AgentBlock::Callers { source, .. },
            ) => existing == source,
            _ => false,
        };
        if replace {
            *self.agent_blocks.last_mut().expect("last block exists") = block;
            self.rebuild_agent_lines();
        } else {
            self.push_agent_block(block);
        }
    }

    /// 更新同一源文件与符号的被调用者摘要。
    pub fn upsert_agent_callees(&mut self, block: AgentBlock) {
        if self.kind != PanelKind::Agent {
            return;
        }
        let replace = match (&self.agent_blocks.last(), &block) {
            (
                Some(AgentBlock::Callees {
                    source: existing, ..
                }),
                AgentBlock::Callees { source, .. },
            ) => existing == source,
            _ => false,
        };
        if replace {
            *self.agent_blocks.last_mut().expect("last block exists") = block;
            self.rebuild_agent_lines();
        } else {
            self.push_agent_block(block);
        }
    }

    /// 更新同一源文件与位置的可见符号摘要。
    pub fn upsert_agent_visible(&mut self, block: AgentBlock) {
        if self.kind != PanelKind::Agent {
            return;
        }
        let replace = match (&self.agent_blocks.last(), &block) {
            (
                Some(AgentBlock::Visible {
                    source: existing, ..
                }),
                AgentBlock::Visible { source, .. },
            ) => existing == source,
            _ => false,
        };
        if replace {
            *self.agent_blocks.last_mut().expect("last block exists") = block;
            self.rebuild_agent_lines();
        } else {
            self.push_agent_block(block);
        }
    }

    /// 更新同一源文件与位置的类型查询结果。
    pub fn upsert_agent_type(&mut self, block: AgentBlock) {
        if self.kind != PanelKind::Agent {
            return;
        }
        let replace = match (&self.agent_blocks.last(), &block) {
            (
                Some(AgentBlock::Type {
                    source: existing, ..
                }),
                AgentBlock::Type { source, .. },
            ) => existing == source,
            _ => false,
        };
        if replace {
            *self.agent_blocks.last_mut().expect("last block exists") = block;
            self.rebuild_agent_lines();
        } else {
            self.push_agent_block(block);
        }
    }

    /// 更新同一源文件与符号的发生位置摘要。
    pub fn upsert_agent_occurrences(&mut self, block: AgentBlock) {
        if self.kind != PanelKind::Agent {
            return;
        }
        let replace = match (&self.agent_blocks.last(), &block) {
            (
                Some(AgentBlock::Occurrences {
                    source: existing, ..
                }),
                AgentBlock::Occurrences { source, .. },
            ) => existing == source,
            _ => false,
        };
        if replace {
            *self.agent_blocks.last_mut().expect("last block exists") = block;
            self.rebuild_agent_lines();
        } else {
            self.push_agent_block(block);
        }
    }

    /// 更新同一源文件的符号摘要。
    pub fn upsert_agent_symbols(&mut self, block: AgentBlock) {
        if self.kind != PanelKind::Agent {
            return;
        }
        let replace = match (&self.agent_blocks.last(), &block) {
            (
                Some(AgentBlock::Symbols {
                    source: existing, ..
                }),
                AgentBlock::Symbols { source, .. },
            ) => existing == source,
            _ => false,
        };
        if replace {
            *self.agent_blocks.last_mut().expect("last block exists") = block;
            self.rebuild_agent_lines();
        } else {
            self.push_agent_block(block);
        }
    }
    /// 更新同一源文件的引用关系摘要。
    pub fn upsert_agent_references(&mut self, block: AgentBlock) {
        if self.kind != PanelKind::Agent {
            return;
        }
        let replace = match (&self.agent_blocks.last(), &block) {
            (
                Some(AgentBlock::References {
                    source: existing, ..
                }),
                AgentBlock::References { source, .. },
            ) => existing == source,
            _ => false,
        };
        if replace {
            *self.agent_blocks.last_mut().expect("last block exists") = block;
            self.rebuild_agent_lines();
        } else {
            self.push_agent_block(block);
        }
    }

    /// 更新同一源文件的 Token 流摘要。
    pub fn upsert_agent_tokens(&mut self, block: AgentBlock) {
        if self.kind != PanelKind::Agent {
            return;
        }
        let replace = match (&self.agent_blocks.last(), &block) {
            (
                Some(AgentBlock::Tokens {
                    source: existing, ..
                }),
                AgentBlock::Tokens { source, .. },
            ) => existing == source,
            _ => false,
        };
        if replace {
            *self.agent_blocks.last_mut().expect("last block exists") = block;
            self.rebuild_agent_lines();
        } else {
            self.push_agent_block(block);
        }
    }

    /// 插入审批请求，或用同一审批标识的最终决定替换待定状态。
    pub fn upsert_agent_approval(&mut self, block: AgentBlock) {
        if self.kind != PanelKind::Agent {
            return;
        }
        let AgentBlock::Approval { id, .. } = &block else {
            return;
        };
        let existing = self.agent_blocks.iter().rposition(
            |candidate| matches!(candidate, AgentBlock::Approval { id: candidate, .. } if candidate == id),
        );
        if let Some(index) = existing {
            self.agent_blocks[index] = block;
            self.rebuild_agent_lines();
        } else {
            self.push_agent_block(block);
        }
    }

    /// 将流式 Markdown 追加到最后一个 Agent 文本块。
    pub fn append_agent_markdown(&mut self, text: &str) {
        if self.kind != PanelKind::Agent {
            self.append_stream(text);
            return;
        }
        if let Some(AgentBlock::Markdown(existing)) = self.agent_blocks.last_mut() {
            existing.push_str(text);
        } else {
            self.agent_blocks
                .push(AgentBlock::Markdown(text.to_owned()));
            self.lines.push(Line::from(String::new()));
        }
        self.append_stream(text);
    }

    /// 将最后一段 Markdown 中所有完整 Rust/Teal fenced code block 拆成可操作块。
    pub fn finalize_agent_markdown(&mut self) {
        if self.kind != PanelKind::Agent {
            return;
        }
        let Some(AgentBlock::Markdown(text)) = self.agent_blocks.last() else {
            return;
        };
        let Some(blocks) = split_fenced_code(text) else {
            return;
        };
        self.agent_blocks.pop();
        self.agent_blocks.extend(blocks);
        self.rebuild_agent_lines();
    }

    /// 返回指定的 Agent 代码块，编号从 1 开始。
    #[must_use]
    pub fn agent_code(&self, index: usize) -> Option<(String, String)> {
        self.agent_blocks
            .iter()
            .filter_map(|block| match block {
                AgentBlock::Code { language, source } => Some((language, source)),
                _ => None,
            })
            .nth(index.saturating_sub(1))
            .map(|(language, source)| (language.clone(), source.clone()))
    }

    fn rebuild_agent_lines(&mut self) {
        self.lines = self
            .agent_blocks
            .iter()
            .flat_map(AgentBlock::lines)
            .map(Line::from)
            .collect();
    }

    /// 返回 Agent buffer 的结构化内容，只读给面板动作和测试使用。
    #[must_use]
    pub fn agent_blocks(&self) -> &[AgentBlock] {
        &self.agent_blocks
    }

    /// 返回会话面板的 gutter 与装饰视图。
    ///
    /// 会话把工具调用/结果、诊断与审批统一投影为 [`Decorations`]，与编辑器共用同一
    /// 套行首标记语义；每个结构化 block 占一行，标记落在它对应的行上。
    #[must_use]
    pub fn decorations(&self) -> crate::workbench::ui::Decorations {
        use crate::workbench::ui::{Decorations, GutterType};

        let mut decorations = Decorations::default();
        for (line, block) in self.agent_blocks.iter().enumerate() {
            let marker = match block {
                AgentBlock::Diagnostic { .. } | AgentBlock::SourceDiagnostic { .. } => {
                    Some(GutterType::Warning)
                }
                AgentBlock::ToolResult { .. } => Some(GutterType::Tool),
                AgentBlock::ToolCall { .. } => Some(GutterType::Vcs),
                AgentBlock::Approval { .. } => Some(GutterType::Breakpoint),
                _ => None,
            };
            if let Some(marker) = marker {
                decorations.push_gutter(line, marker);
            }
        }
        decorations
    }

    /// 将流式文本追加到最后一行，必要时创建该行。
    pub fn append_stream(&mut self, text: &str) {
        for (index, part) in text.split('\n').enumerate() {
            if index == 0 {
                if let Some(last) = self.lines.last_mut() {
                    *last = Line::from(format!("{last}{part}"));
                } else {
                    self.lines.push(Line::from(part.to_owned()));
                }
            } else {
                self.lines.push(Line::from(part.to_owned()));
            }
        }
    }

    /// 返回面板的序列化资源字符串。
    pub fn resource(&self) -> &str {
        &self.resource
    }
}

fn redact_json_text(text: &mut String) {
    let Ok(mut value) = serde_json::from_str(text) else {
        return;
    };
    redact_sensitive_fields(&mut value);
    *text = value.to_string();
}

fn redact_sensitive_fields(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => values.iter_mut().for_each(redact_sensitive_fields),
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                if matches!(
                    key.to_ascii_lowercase().as_str(),
                    "authorization" | "api_key" | "apikey" | "password" | "token" | "secret"
                ) {
                    *value = serde_json::Value::String("[REDACTED]".into());
                } else {
                    redact_sensitive_fields(value);
                }
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

fn diff_line(kind: PanelKind, text: &str) -> Line<'static> {
    if kind != PanelKind::Diff {
        return Line::from(text.to_owned());
    }
    let style = if text.starts_with("+++") || text.starts_with("---") {
        Style::default().fg(Color::DarkGray)
    } else if text.starts_with('+') {
        Style::default().fg(Color::Green)
    } else if text.starts_with('-') {
        Style::default().fg(Color::Red)
    } else if text.starts_with("@@") {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };
    Line::styled(text.to_owned(), style)
}

fn split_fenced_code(text: &str) -> Option<Vec<AgentBlock>> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut blocks = Vec::new();
    let mut markdown = Vec::new();
    let mut found = false;
    let mut index = 0;

    while index < lines.len() {
        let Some((fence_width, language)) = opening_fence(lines[index]) else {
            markdown.push(lines[index]);
            index += 1;
            continue;
        };
        let Some(closing) = ((index + 1)..lines.len())
            .find(|candidate| is_closing_fence(lines[*candidate], fence_width))
        else {
            markdown.extend_from_slice(&lines[index..]);
            break;
        };

        if !markdown.is_empty() {
            blocks.push(AgentBlock::Markdown(markdown.join("\n")));
            markdown.clear();
        }
        blocks.push(AgentBlock::Code {
            language,
            source: lines[index + 1..closing].join("\n"),
        });
        found = true;
        index = closing + 1;
    }

    if !markdown.is_empty() {
        blocks.push(AgentBlock::Markdown(markdown.join("\n")));
    }
    found.then_some(blocks)
}

fn opening_fence(line: &str) -> Option<(usize, String)> {
    let line = line.trim_end_matches('\r').trim_start();
    let width = line.bytes().take_while(|byte| *byte == b'`').count();
    if width < 3 {
        return None;
    }
    let language = line[width..]
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(language.as_str(), "teal" | "tl" | "rust" | "rs").then_some((width, language))
}

fn is_closing_fence(line: &str, opening_width: usize) -> bool {
    let line = line.trim();
    let width = line.bytes().take_while(|byte| *byte == b'`').count();
    width >= opening_width && width == line.len()
}

impl Widget for TextPanel {
    fn kind(&self) -> PanelKind {
        self.kind
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn status(&self) -> Option<PanelStatus> {
        Some(PanelStatus {
            mode: "view".to_owned(),
            resource: self.resource.clone(),
            position: format!(
                "{}/{} Horizontal: {}",
                self.view.vertical.saturating_add(1),
                self.lines.len(),
                self.view.horizontal
            ),
        })
    }

    fn handle_input(&mut self, event: InputEvent) -> WidgetAction {
        match event.key {
            Key::Char('j') | Key::Down => {
                self.view.vertical =
                    (self.view.vertical + 1).min(self.lines.len().saturating_sub(1));
            }
            Key::Char('k') | Key::Up => {
                self.view.vertical = self.view.vertical.saturating_sub(1);
            }
            Key::PageDown => {
                self.view.vertical =
                    (self.view.vertical + 20).min(self.lines.len().saturating_sub(1));
            }
            Key::PageUp => self.view.vertical = self.view.vertical.saturating_sub(20),
            Key::Char('g') | Key::Home => self.view.vertical = 0,
            Key::Char('G') | Key::End => {
                self.view.vertical = self.lines.len().saturating_sub(1);
            }
            _ => return WidgetAction::Ignored,
        }
        WidgetAction::Consumed
    }

    fn handle_mouse(&mut self, event: MouseEvent, area: Rect) -> WidgetAction {
        let lines = plain_lines(&self.lines);
        self.view.handle_mouse(&lines, event, area)
    }

    fn handle_clipboard(
        &mut self,
        action: ClipboardAction,
        _paste: Option<&str>,
    ) -> ClipboardOutcome {
        let lines = plain_lines(&self.lines);
        self.view.handle_clipboard(&lines, action)
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let block = Block::default()
            .title(self.title.clone())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused {
                Color::Cyan
            } else {
                Color::DarkGray
            }));
        let lines = render_lines_with_selection(&self.lines, self.view.selection);
        frame.render_widget(
            Paragraph::new(lines)
                .scroll((
                    u16::try_from(self.view.vertical).unwrap_or(u16::MAX),
                    u16::try_from(self.view.horizontal).unwrap_or(u16::MAX),
                ))
                .block(block),
            area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个可在会话中往返的完整变更集块。
    fn review_block(path: &str) -> AgentBlock {
        AgentBlock::ChangeSetReview(
            teal_workbench_core::ChangeSet::new(
                Some("task-1".into()),
                1,
                vec![
                    teal_workbench_core::ChangeSetFile::new(
                        std::path::PathBuf::from(path),
                        "a".repeat(64),
                        "b".repeat(64),
                        format!("--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-old\n+new"),
                    )
                    .expect("change set file should be valid"),
                ],
            )
            .expect("change set should be valid"),
        )
    }

    #[test]
    fn agent_blocks_preserve_structured_types_and_stream_markdown() {
        let mut panel = TextPanel::plain(PanelKind::Agent, "Agent", "");
        panel.append_agent_markdown("hello");
        panel.append_agent_markdown(" world");
        panel.push_agent_block(AgentBlock::Code {
            language: "teal".into(),
            source: "fn main() {}".into(),
        });
        panel.push_agent_block(AgentBlock::ToolCall {
            name: "workspace.write".into(),
            arguments: "{}".into(),
        });
        panel.push_agent_block(AgentBlock::ToolResult {
            name: "workspace.write".into(),
            text: "ok".into(),
            checkpoint: Some("cp-1".into()),
        });
        panel.push_agent_block(AgentBlock::Diagnostic {
            summary: "failed".into(),
            detail: "details".into(),
        });
        panel.push_agent_block(review_block("src/main.tl"));
        panel.push_agent_block(AgentBlock::Status("completed".into()));

        assert_eq!(
            panel.agent_blocks(),
            &[
                AgentBlock::Markdown("hello world".into()),
                AgentBlock::Code {
                    language: "teal".into(),
                    source: "fn main() {}".into(),
                },
                AgentBlock::ToolCall {
                    name: "workspace.write".into(),
                    arguments: "{}".into(),
                },
                AgentBlock::ToolResult {
                    name: "workspace.write".into(),
                    text: "ok".into(),
                    checkpoint: Some("cp-1".into()),
                },
                AgentBlock::Diagnostic {
                    summary: "failed".into(),
                    detail: "details".into(),
                },
                review_block("src/main.tl"),
                AgentBlock::Status("completed".into()),
            ]
        );
    }

    #[test]
    fn agent_blocks_project_tool_and_diagnostic_markers_into_gutter() {
        use crate::workbench::ui::GutterType;

        let mut panel = TextPanel::plain(PanelKind::Agent, "Agent", "");
        panel.push_agent_block(AgentBlock::ToolCall {
            name: "teal.check".into(),
            arguments: "{}".into(),
        });
        panel.push_agent_block(AgentBlock::ToolResult {
            name: "teal.check".into(),
            text: "ok".into(),
            checkpoint: None,
        });
        panel.push_agent_block(AgentBlock::Diagnostic {
            summary: "failed".into(),
            detail: "details".into(),
        });

        let decorations = panel.decorations();
        assert!(decorations.gutter(0).contains(&GutterType::Vcs));
        assert!(decorations.gutter(1).contains(&GutterType::Tool));
        assert!(decorations.gutter(2).contains(&GutterType::Warning));
        // 普通文本轮次不产生行首标记。
        assert!(decorations.gutter(3).is_empty());
    }

    #[test]
    fn finalize_agent_markdown_promotes_fenced_code_for_numbered_insertion() {
        let mut panel = TextPanel::plain(PanelKind::Agent, "Agent", "");
        panel.append_agent_markdown("```teal\nfn main() {}\n```");
        panel.finalize_agent_markdown();

        assert_eq!(
            panel.agent_blocks(),
            &[AgentBlock::Code {
                language: "teal".into(),
                source: "fn main() {}".into(),
            }]
        );
        assert_eq!(
            panel.agent_code(1),
            Some(("teal".into(), "fn main() {}".into()))
        );
        assert_eq!(panel.agent_code(2), None);
    }

    #[test]
    fn finalize_agent_markdown_splits_multiple_fences_and_preserves_surrounding_markdown() {
        let mut panel = TextPanel::plain(PanelKind::Agent, "Agent", "");
        panel.append_agent_markdown(
            "before\n```rust\nfn rust() {}\n```\nbetween\n```teal\nfn teal() {}\n```\nafter",
        );
        panel.finalize_agent_markdown();

        assert_eq!(
            panel.agent_blocks(),
            &[
                AgentBlock::Markdown("before".into()),
                AgentBlock::Code {
                    language: "rust".into(),
                    source: "fn rust() {}".into(),
                },
                AgentBlock::Markdown("between".into()),
                AgentBlock::Code {
                    language: "teal".into(),
                    source: "fn teal() {}".into(),
                },
                AgentBlock::Markdown("after".into()),
            ]
        );
        assert_eq!(
            panel.agent_code(1),
            Some(("rust".into(), "fn rust() {}".into()))
        );
        assert_eq!(
            panel.agent_code(2),
            Some(("teal".into(), "fn teal() {}".into()))
        );
    }

    #[test]
    fn finalize_agent_markdown_accepts_crlf_indentation_and_longer_closing_fences() {
        let mut panel = TextPanel::plain(PanelKind::Agent, "Agent", "");
        panel
            .append_agent_markdown("before\r\n    ````Rust\r\n  fn main() {}\r\n    ````\r\nafter");
        panel.finalize_agent_markdown();

        assert_eq!(
            panel.agent_blocks(),
            &[
                AgentBlock::Markdown("before".into()),
                AgentBlock::Code {
                    language: "rust".into(),
                    source: "  fn main() {}".into(),
                },
                AgentBlock::Markdown("after".into()),
            ]
        );
    }

    #[test]
    fn finalize_agent_markdown_keeps_non_target_and_unclosed_fences_as_markdown() {
        for text in [
            "before\n```python\nprint('no')\n```\nafter",
            "before\n```teal\nfn main() {}",
        ] {
            let mut panel = TextPanel::plain(PanelKind::Agent, "Agent", "");
            panel.append_agent_markdown(text);
            panel.finalize_agent_markdown();
            assert_eq!(panel.agent_blocks(), &[AgentBlock::Markdown(text.into())]);
        }
    }

    #[test]
    fn task_block_preserves_timeline_fields_and_renders_compactly() {
        let block = AgentBlock::Task {
            id: "task-1".into(),
            state: "running".into(),
            phase: Some("tools".into()),
            detail: Some("workspace.write".into()),
        };
        assert_eq!(
            block.lines(),
            vec!["[Task] task-1 state=running phase=tools workspace.write"]
        );

        let mut panel = TextPanel::plain(PanelKind::Agent, "Agent", "");
        panel.push_agent_block(block.clone());
        assert_eq!(panel.agent_blocks(), &[block]);
    }

    #[test]
    fn evidence_block_renders_ordered_summary_items_without_raw_output() {
        let block = AgentBlock::Evidence {
            task: "task-1".into(),
            items: vec![
                "#1 workspace.write success=true exit=Some(0) path=src/main.tl".into(),
                "#2 teal.run success=false exit=Some(1) path=<workspace>".into(),
            ],
        };
        assert_eq!(
            block.lines(),
            vec![
                "[Evidence] task-1",
                "- #1 workspace.write success=true exit=Some(0) path=src/main.tl",
                "- #2 teal.run success=false exit=Some(1) path=<workspace>",
            ]
        );
    }

    #[test]
    fn graph_block_renders_task_summary_without_payloads() {
        let block = AgentBlock::Graph {
            run: "run-1".into(),
            tasks: vec!["task=t1 agent=worker status=completed event=build".into()],
            events: 2,
        };
        assert_eq!(
            block.lines(),
            vec![
                "[Graph] run-1 tasks=1 events=2",
                "- task=t1 agent=worker status=completed event=build",
            ]
        );
    }

    #[test]
    fn graph_edges_block_renders_and_upserts_same_run() {
        let block = AgentBlock::GraphEdges {
            run: "run-1".into(),
            edges: vec!["1 -delegation-> 2".into()],
        };
        assert_eq!(
            block.lines(),
            vec!["[Graph edges] run-1 count=1", "- 1 -delegation-> 2"]
        );
        let mut panel = TextPanel::plain(PanelKind::Agent, "Agent", "");
        panel.upsert_agent_graph_edges(block);
        panel.upsert_agent_graph_edges(AgentBlock::GraphEdges {
            run: "run-1".into(),
            edges: vec!["1 -wait-> 3".into()],
        });
        assert_eq!(panel.agent_blocks().len(), 1);
        assert!(matches!(
            panel.agent_blocks().first(),
            Some(AgentBlock::GraphEdges { edges, .. }) if edges == &["1 -wait-> 3"]
        ));
    }

    #[test]
    fn graph_agents_block_renders_and_upserts_same_run() {
        let mut panel = TextPanel::plain(PanelKind::Agent, "Agent", "");
        panel.upsert_agent_graph_agents(AgentBlock::GraphAgents {
            run: "run-1".into(),
            agents: vec!["/root worker status=idle active_task=none".into()],
        });
        panel.upsert_agent_graph_agents(AgentBlock::GraphAgents {
            run: "run-1".into(),
            agents: vec!["/root worker status=running active_task=7".into()],
        });
        assert_eq!(panel.agent_blocks().len(), 1);
        assert!(matches!(
            panel.agent_blocks().first(),
            Some(AgentBlock::GraphAgents { agents, .. }) if agents[0].contains("status=running")
        ));
    }

    #[test]
    fn upsert_agent_graph_replaces_same_run_and_appends_different_runs() {
        let mut panel = TextPanel::plain(PanelKind::Agent, "Agent", "");
        panel.upsert_agent_graph(AgentBlock::Graph {
            run: "run-1".into(),
            tasks: vec!["task=t1".into()],
            events: 1,
        });
        panel.upsert_agent_graph(AgentBlock::Graph {
            run: "run-1".into(),
            tasks: vec!["task=t1 status=completed".into()],
            events: 2,
        });

        assert_eq!(
            panel.agent_blocks(),
            &[AgentBlock::Graph {
                run: "run-1".into(),
                tasks: vec!["task=t1 status=completed".into()],
                events: 2,
            }]
        );

        panel.upsert_agent_graph(AgentBlock::Graph {
            run: "run-2".into(),
            tasks: vec!["task=t2".into()],
            events: 1,
        });
        assert_eq!(panel.agent_blocks().len(), 2);
        assert!(matches!(
            panel.agent_blocks().last(),
            Some(AgentBlock::Graph { run, .. }) if run == "run-2"
        ));
    }

    #[test]
    fn upsert_agent_approval_replaces_pending_state_for_the_same_id() {
        let mut panel = TextPanel::plain(PanelKind::Agent, "Agent", "");
        panel.upsert_agent_approval(AgentBlock::Approval {
            id: "call-1".into(),
            action: "workspace.write_file".into(),
            resources: vec!["file:///workspace/main.tl".into()],
            decision: None,
        });
        panel.upsert_agent_approval(AgentBlock::Approval {
            id: "call-1".into(),
            action: "workspace.write_file".into(),
            resources: vec!["file:///workspace/main.tl".into()],
            decision: Some("allowed-task".into()),
        });
        panel.upsert_agent_approval(AgentBlock::Approval {
            id: "call-2".into(),
            action: "workspace.read_file".into(),
            resources: vec!["file:///workspace/lib.tl".into()],
            decision: Some("denied".into()),
        });

        assert_eq!(
            panel.agent_blocks(),
            &[
                AgentBlock::Approval {
                    id: "call-1".into(),
                    action: "workspace.write_file".into(),
                    resources: vec!["file:///workspace/main.tl".into()],
                    decision: Some("allowed-task".into()),
                },
                AgentBlock::Approval {
                    id: "call-2".into(),
                    action: "workspace.read_file".into(),
                    resources: vec!["file:///workspace/lib.tl".into()],
                    decision: Some("denied".into()),
                },
            ]
        );
        assert_eq!(
            plain_lines(&panel.lines),
            vec![
                "[Approval] workspace.write_file state=allowed-task id=call-1",
                "- file:///workspace/main.tl",
                "[Approval] workspace.read_file state=denied id=call-2",
                "- file:///workspace/lib.tl",
            ]
        );
    }

    #[test]
    fn agent_session_blocks_round_trip_and_interrupt_pending_approvals() {
        let mut panel = TextPanel::plain(PanelKind::Agent, "Agent", "");
        panel.push_agent_block(AgentBlock::Code {
            language: "teal".into(),
            source: "fn main() {}".into(),
        });
        panel.push_agent_block(review_block("src/main.tl"));
        panel.push_agent_block(AgentBlock::Approval {
            id: "call-1".into(),
            action: "workspace.write_file".into(),
            resources: vec!["src/main.tl".into()],
            decision: None,
        });
        panel.push_agent_block(AgentBlock::Evidence {
            task: "task-1".into(),
            items: vec!["#1 workspace.write_file success=true".into()],
        });

        let teal_workbench_services::PanelSessionState::Text { blocks, .. } = panel.session_state()
        else {
            panic!("Agent panel must serialize as Text state");
        };
        let blocks = blocks.expect("Agent panel must persist structured blocks");
        let expected = vec![
            AgentBlock::Code {
                language: "teal".into(),
                source: "fn main() {}".into(),
            },
            review_block("src/main.tl"),
            AgentBlock::Approval {
                id: "call-1".into(),
                action: "workspace.write_file".into(),
                resources: vec!["src/main.tl".into()],
                decision: Some("interrupted".into()),
            },
            AgentBlock::Evidence {
                task: "task-1".into(),
                items: vec!["#1 workspace.write_file success=true".into()],
            },
        ];
        assert_eq!(
            serde_json::from_value::<Vec<AgentBlock>>(blocks.clone()).unwrap(),
            expected
        );
        assert!(matches!(
            panel.agent_blocks().get(2),
            Some(AgentBlock::Approval { decision: None, .. })
        ));

        let mut restored = TextPanel::plain(PanelKind::Agent, "Agent", "fallback");
        assert!(restored.restore_agent_blocks(Some(&blocks)));
        assert_eq!(restored.agent_blocks(), expected);
        assert!(!restored.restore_agent_blocks(None));
    }

    #[test]
    fn agent_session_blocks_redact_nested_json_tool_fields() {
        let mut panel = TextPanel::plain(PanelKind::Agent, "Agent", "");
        panel.push_agent_block(AgentBlock::ToolCall {
            name: "workspace.write_file".into(),
            arguments: serde_json::json!({
                "path": "src/main.tl",
                "nested": {"token": "tool-secret"},
                "items": [{"api_key": "nested-secret"}],
            })
            .to_string(),
        });
        panel.push_agent_block(AgentBlock::ToolResult {
            name: "workspace.write_file".into(),
            text: serde_json::json!({
                "authorization": "result-secret",
                "child": {"password": "password-secret"},
                "ok": true,
            })
            .to_string(),
            checkpoint: None,
        });

        let teal_workbench_services::PanelSessionState::Text { blocks, .. } = panel.session_state()
        else {
            panic!("Agent panel must serialize as Text state");
        };
        let blocks = serde_json::from_value::<Vec<AgentBlock>>(blocks.unwrap()).unwrap();
        assert!(matches!(
            &blocks[0],
            AgentBlock::ToolCall { arguments, .. }
                if serde_json::from_str::<serde_json::Value>(arguments).unwrap()
                    == serde_json::json!({
                        "path": "src/main.tl",
                        "nested": {"token": "[REDACTED]"},
                        "items": [{"api_key": "[REDACTED]"}],
                    })
        ));
        assert!(matches!(
            &blocks[1],
            AgentBlock::ToolResult { text, .. }
                if serde_json::from_str::<serde_json::Value>(text).unwrap()
                    == serde_json::json!({
                        "authorization": "[REDACTED]",
                        "child": {"password": "[REDACTED]"},
                        "ok": true,
                    })
        ));
        assert!(matches!(
            panel.agent_blocks()[0],
            AgentBlock::ToolCall { ref arguments, .. } if arguments.contains("tool-secret")
        ));
    }

    #[test]
    fn changeset_review_session_round_trip_preserves_review() {
        let review = teal_workbench_core::ChangeSet::new(
            Some("task-1".into()),
            3,
            vec![
                teal_workbench_core::ChangeSetFile::new(
                    std::path::PathBuf::from("src/main.tl"),
                    "a".repeat(64),
                    "b".repeat(64),
                    "--- a/src/main.tl\n+++ b/src/main.tl\n@@ -1 +1 @@\n-old\n+new".into(),
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let mut panel = TextPanel::plain(PanelKind::Agent, "Agent", "");
        panel.push_agent_block(AgentBlock::ChangeSetReview(review.clone()));
        let teal_workbench_services::PanelSessionState::Text { blocks, .. } = panel.session_state()
        else {
            panic!("Agent panel must serialize as Text state");
        };
        let mut restored = TextPanel::plain(PanelKind::Agent, "Agent", "");
        assert!(restored.restore_agent_blocks(blocks.as_ref()));
        assert_eq!(
            restored.agent_blocks(),
            &[AgentBlock::ChangeSetReview(review)]
        );
    }
}
