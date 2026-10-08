//! 工作区源码快照查询与结构化结果展示。

use super::*;

impl App {
    /// 捕获同一事件循环中的所有命名缓冲区，服务层不持有界面句柄。
    pub(super) fn agent_source_snapshot(
        &self,
    ) -> Result<teal_workbench_services::tools::SourceQuerySnapshot, String> {
        teal_workbench_services::tools::SourceQuerySnapshot::new(
            &self.workspace,
            self.buffers
                .iter()
                .filter_map(|buffer| buffer.path().map(|path| (path, buffer.to_string()))),
        )
    }

    /// 将工作区执行失败追加到当前 Agent buffer 的诊断块。
    pub(in crate::app) fn append_agent_diagnostic(
        &mut self,
        summary: impl Into<String>,
        detail: impl Into<String>,
    ) {
        let panel = self.ensure_agent_panel();
        if let Some(output) = self
            .panels
            .get_mut(panel)
            .and_then(|panel| panel.as_text_mut())
        {
            output.push_agent_block(AgentBlock::Diagnostic {
                summary: summary.into(),
                detail: detail.into(),
            });
        }
    }

    pub(in crate::app) fn load_agent_references(&mut self, requested: &str) {
        let requested = requested.trim();
        if requested.is_empty() {
            self.append_agent_diagnostic(
                "Teal references require a source path",
                "Usage: :agent-references <path>",
            );
            return;
        }
        let path = PathBuf::from(requested);
        let path = if path.is_absolute() {
            path
        } else {
            self.workspace.join(path)
        };
        if let Err(error) = crate::app::ensure_workspace_path(&self.workspace, &path) {
            self.append_agent_diagnostic("Teal references path is invalid", error);
            return;
        }
        let path = path.canonicalize().unwrap_or(path);
        let analysis = match self
            .agent_source_snapshot()
            .and_then(|snapshot| snapshot.analyze(&path))
        {
            Ok(source) => source,
            Err(error) => {
                self.append_agent_diagnostic(
                    "Teal references could not read source",
                    error.to_string(),
                );
                return;
            }
        };
        let refs: Vec<String> = analysis
            .references()
            .iter()
            .map(|r| {
                let role = format!("{:?}", r.role);
                let name = analysis
                    .symbols()
                    .get(r.symbol.as_usize())
                    .map(|s| s.name.as_str())
                    .unwrap_or("<unknown>");
                format!(
                    "{}..{} {role} -> {name}",
                    r.span.start,
                    r.span.start + r.span.size
                )
            })
            .collect();
        let display_source = path
            .strip_prefix(&self.workspace)
            .unwrap_or(&path)
            .display()
            .to_string();
        if let Some(panel) = self
            .agent_panel
            .and_then(|id| self.panels.get_mut(id))
            .and_then(AppPanel::as_text_mut)
        {
            panel.upsert_agent_references(AgentBlock::References {
                source: display_source,
                items: refs,
            });
        } else {
            self.message(Severity::Warning, "agent", "The Agent buffer is not open");
        }
    }

    pub(in crate::app) fn load_agent_tokens(&mut self, requested: &str) {
        let requested = requested.trim();
        if requested.is_empty() {
            self.append_agent_diagnostic(
                "Teal tokens require a source path",
                "Usage: :agent-tokens <path>",
            );
            return;
        }
        let path = PathBuf::from(requested);
        let path = if path.is_absolute() {
            path
        } else {
            self.workspace.join(path)
        };
        if let Err(error) = crate::app::ensure_workspace_path(&self.workspace, &path) {
            self.append_agent_diagnostic("Teal tokens path is invalid", error);
            return;
        }
        let path = path.canonicalize().unwrap_or(path);
        let analysis = match self
            .agent_source_snapshot()
            .and_then(|snapshot| snapshot.analyze(&path))
        {
            Ok(source) => source,
            Err(error) => {
                self.append_agent_diagnostic(
                    "Teal tokens could not read source",
                    error.to_string(),
                );
                return;
            }
        };
        let tokens: Vec<String> = analysis
            .tokens()
            .iter()
            .map(|token| {
                format!(
                    "{:?} {}..{} `{}`",
                    token.kind,
                    token.span.start,
                    token.span.start + token.span.size,
                    token.text
                )
            })
            .collect();
        let display_source = path
            .strip_prefix(&self.workspace)
            .unwrap_or(&path)
            .display()
            .to_string();
        if let Some(panel) = self
            .agent_panel
            .and_then(|id| self.panels.get_mut(id))
            .and_then(AppPanel::as_text_mut)
        {
            panel.upsert_agent_tokens(AgentBlock::Tokens {
                source: display_source,
                items: tokens,
            });
        } else {
            self.message(Severity::Warning, "agent", "The Agent buffer is not open");
        }
    }

    /// 追加带可选源位置的诊断，用于测试失败自动定位。
    pub(in crate::app) fn append_agent_source_diagnostic(
        &mut self,
        summary: impl Into<String>,
        detail: impl Into<String>,
        source: Option<String>,
        position: Option<u32>,
    ) {
        let panel = self.ensure_agent_panel();
        if let Some(output) = self
            .panels
            .get_mut(panel)
            .and_then(|panel| panel.as_text_mut())
        {
            output.push_agent_block(AgentBlock::SourceDiagnostic {
                summary: summary.into(),
                detail: detail.into(),
                source,
                position,
            });
        }
    }

    /// 运行 cargo check 并将编译器错误解析为带源位置的诊断块。
    pub(in crate::app) fn run_diagnose(&mut self, requested: &str) {
        let args: Vec<&str> = requested.split_whitespace().collect();
        let (mode, crate_name) = match args.as_slice() {
            ["test", rest @ ..] if !rest.is_empty() => ("test", rest.join(" ")),
            _ => ("check", requested.trim().to_owned()),
        };
        let crate_name = crate_name.trim();
        if crate_name.is_empty() {
            self.append_agent_diagnostic(
                "Diagnose requires a crate name",
                "Usage: :diagnose [test] <crate-name>",
            );
            return;
        }

        let mut cmd = std::process::Command::new("cargo");
        if mode == "test" {
            cmd.args(["test", "-p", crate_name, "--message-format=json"]);
        } else {
            cmd.args(["check", "-p", crate_name, "--message-format=json"]);
        }
        cmd.current_dir(&self.workspace)
            .env("CARGO_TERM_COLOR", "never");

        let output = match cmd.output() {
            Ok(output) => output,
            Err(error) => {
                self.append_agent_diagnostic("cargo could not start", error.to_string());
                return;
            }
        };

        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{stderr}\n{stdout}");

        let diagnostics = teal_workbench_services::parse_cargo_diagnostics(&stdout, 50);
        if diagnostics.is_empty() {
            if output.status.success() {
                self.append_agent_diagnostic(
                    format!("cargo {mode} -p {crate_name} passed"),
                    String::new(),
                );
            } else {
                let fallback = combined
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .take(10)
                    .collect::<Vec<_>>()
                    .join("\n");
                self.append_agent_diagnostic(
                    format!("cargo {mode} -p {crate_name} failed"),
                    fallback,
                );
            }
        } else {
            for diagnostic in &diagnostics {
                let summary = diagnostic.code.as_deref().map_or_else(
                    || format!("{}: {}", diagnostic.level, diagnostic.message),
                    |code| format!("{}[{code}]: {}", diagnostic.level, diagnostic.message),
                );
                self.append_agent_source_diagnostic(
                    summary,
                    diagnostic.children.join("\n"),
                    diagnostic.primary.as_ref().map(|span| span.file.clone()),
                    diagnostic.primary.as_ref().map(|span| span.line),
                );
                let Some(span) = diagnostic.primary.as_ref() else {
                    continue;
                };
                let requested = PathBuf::from(&span.file);
                let source_path = if requested.is_absolute() {
                    requested
                } else {
                    self.workspace.join(requested)
                };
                let overlay = self
                    .buffers
                    .find(&source_path)
                    .map(|buffer| buffer.to_string());
                let Ok(context) = teal_workbench_services::diagnostic_context(
                    &self.workspace,
                    span,
                    overlay.as_deref(),
                ) else {
                    continue;
                };
                let symbol = source_path
                    .extension()
                    .and_then(|value| value.to_str())
                    .filter(|extension| *extension == "tl")
                    .and_then(|_| {
                        let source = overlay
                            .clone()
                            .or_else(|| std::fs::read_to_string(&source_path).ok())?;
                        teal_compiler::analyze_source(&teal_compiler::Source::new(
                            context.path.clone(),
                            source,
                        ))
                        .symbol_at(span.byte_start)
                        .map(|symbol| symbol.name.clone())
                    });
                let related = diagnostic
                    .related
                    .iter()
                    .map(|span| format!("{}:{}:{}", span.file, span.line, span.column))
                    .collect();
                let panel = self.ensure_agent_panel();
                if let Some(output) = self
                    .panels
                    .get_mut(panel)
                    .and_then(|panel| panel.as_text_mut())
                {
                    output.push_agent_block(AgentBlock::DiagnosticContext {
                        source: context.path,
                        line: context.line,
                        column: span.column,
                        symbol,
                        commit: context.commit,
                        commit_summary: context.commit_summary,
                        context: context.context,
                        related,
                    });
                }
            }
            if output.status.success() {
                self.append_agent_diagnostic(
                    format!("cargo {mode} -p {crate_name} passed"),
                    String::new(),
                );
            }
        }
    }

    /// 打开编号从一开始的诊断上下文源码位置。
    pub(in crate::app) fn open_agent_diagnostic_context(&mut self, requested: &str) {
        let Ok(index) = requested.trim().parse::<usize>() else {
            self.message(Severity::Warning, "agent", "Usage: :agent-context <number>");
            return;
        };
        let Some((source, line, column)) = self.agent_panel.and_then(|panel| {
            self.panels
                .get(panel)?
                .as_text()?
                .agent_blocks()
                .iter()
                .filter_map(|block| {
                    if let AgentBlock::DiagnosticContext {
                        source,
                        line,
                        column,
                        ..
                    } = block
                    {
                        Some((source.clone(), *line, *column))
                    } else {
                        None
                    }
                })
                .nth(index.saturating_sub(1))
        }) else {
            self.message(
                Severity::Warning,
                "agent",
                "Diagnostic context was not found",
            );
            return;
        };
        self.queue_open(
            self.workspace.join(source),
            true,
            Some(serde_json::json!({
                "line": line.saturating_sub(1),
                "character": column.saturating_sub(1),
            })),
        );
    }

    /// 编译 Teal 源码并投影一个有界工程分析图。
    pub(in crate::app) fn load_agent_analysis_view(&mut self, arguments: &[String]) {
        let [view, requested] = arguments else {
            self.append_agent_diagnostic(
                "Teal analysis view requires a view and source",
                "Usage: :agent-view <ast|types|data|effects> <path>",
            );
            return;
        };
        if !matches!(view.as_str(), "ast" | "types" | "data" | "effects") {
            self.append_agent_diagnostic(
                "Unknown Teal analysis view",
                "Expected ast, types, data or effects",
            );
            return;
        }
        let path = PathBuf::from(requested);
        let path = if path.is_absolute() {
            path
        } else {
            self.workspace.join(path)
        };
        if let Err(error) = crate::app::ensure_workspace_path(&self.workspace, &path) {
            self.append_agent_diagnostic("Teal analysis path is invalid", error);
            return;
        }
        let path = path.canonicalize().unwrap_or(path);
        let inspected = self
            .agent_source_snapshot()
            .and_then(|snapshot| snapshot.inspect(&path, teal::CompileOptions::default()));
        let graph = match inspected {
            Ok(inspected) => inspected.graph,
            Err(error) => {
                self.append_agent_diagnostic("Teal analysis failed", error.to_string());
                return;
            }
        };
        let include_node = |kind: teal_compiler::AnalysisNodeKind| match view.as_str() {
            "ast" => matches!(
                kind,
                teal_compiler::AnalysisNodeKind::Module
                    | teal_compiler::AnalysisNodeKind::Declaration
                    | teal_compiler::AnalysisNodeKind::Reference
            ),
            "types" => kind == teal_compiler::AnalysisNodeKind::Type,
            "data" => kind == teal_compiler::AnalysisNodeKind::Reference,
            "effects" => kind == teal_compiler::AnalysisNodeKind::Effect,
            _ => false,
        };
        let nodes = graph
            .nodes
            .iter()
            .filter(|node| include_node(node.kind))
            .take(4096)
            .map(|node| format!("{} {:?} {}", node.id, node.kind, node.label))
            .collect::<Vec<_>>();
        let edge_kind = |kind: teal_compiler::AnalysisEdgeKind| match view.as_str() {
            "ast" => kind == teal_compiler::AnalysisEdgeKind::Contains,
            "types" => kind == teal_compiler::AnalysisEdgeKind::TypeOf,
            "data" => matches!(
                kind,
                teal_compiler::AnalysisEdgeKind::ValueFlow | teal_compiler::AnalysisEdgeKind::Calls
            ),
            "effects" => kind == teal_compiler::AnalysisEdgeKind::Effect,
            _ => false,
        };
        let edges = graph
            .edges
            .iter()
            .filter(|edge| edge_kind(edge.kind))
            .take(4096)
            .map(|edge| format!("{} -{:?}-> {}", edge.source, edge.kind, edge.target))
            .collect::<Vec<_>>();
        let display = path
            .strip_prefix(&self.workspace)
            .unwrap_or(&path)
            .display()
            .to_string();
        let panel = self.ensure_agent_panel();
        if let Some(output) = self
            .panels
            .get_mut(panel)
            .and_then(|panel| panel.as_text_mut())
        {
            output.push_agent_block(AgentBlock::AnalysisGraph {
                source: display,
                view: view.clone(),
                nodes,
                edges,
            });
        }
    }

    /// 异步读取指定持久 Graph run，避免在终端事件线程阻塞 SQLite。
    pub(in crate::app) fn load_agent_graph(&mut self, run: &str) {
        let requested = run.trim().to_owned();
        let run = if requested.is_empty() {
            self.extensions
                .loaded
                .values()
                .flat_map(|loaded| loaded.instance.tasks.values())
                .filter_map(|task| task.run_id.clone().map(|run| (task.key.clone(), run)))
                .max_by(|left, right| left.0.cmp(&right.0))
                .map_or_else(String::new, |(_, run)| run)
        } else {
            requested
        };
        if run.is_empty() {
            self.message(
                Severity::Warning,
                "agent",
                "No persisted Graph run is available. Usage: :agent-graph <run-id>",
            );
            return;
        }
        let Some(store) = self.agent_session_store.clone() else {
            self.message(
                Severity::Warning,
                "agent",
                "Agent RuntimeStore is not available",
            );
            return;
        };
        let output = self.runtime_tx.clone();
        self.tasks.spawn(async move {
            let query_run = run.clone();
            let result = crate::runtime_bridge::spawn_blocking(move || {
                store
                    .graph_snapshot(&query_run)
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(std::convert::identity);
            let _ = output
                .send(RuntimeEvent::AgentGraphLoaded { run, result })
                .await;
        });
    }

    /// 将指定 Teal 文件的直接导入投影到 Agent buffer，供模块依赖审查使用。
    pub(in crate::app) fn load_agent_imports(&mut self, requested: &str) {
        let requested = requested.trim();
        if requested.is_empty() {
            self.append_agent_diagnostic(
                "Teal module imports require a source path",
                "Usage: :agent-imports <path>",
            );
            return;
        }
        let path = PathBuf::from(requested);
        let path = if path.is_absolute() {
            path
        } else {
            self.workspace.join(path)
        };
        if let Err(error) = crate::app::ensure_workspace_path(&self.workspace, &path) {
            self.append_agent_diagnostic("Teal module imports path is invalid", error);
            return;
        }
        let path = path.canonicalize().unwrap_or(path);
        let analysis = match self
            .agent_source_snapshot()
            .and_then(|snapshot| snapshot.analyze(&path))
        {
            Ok(source) => source,
            Err(error) => {
                self.append_agent_diagnostic(
                    "Teal module imports could not read source",
                    error.to_string(),
                );
                return;
            }
        };
        let imports = analysis
            .imports()
            .iter()
            .map(|import| {
                import.alias.as_deref().map_or_else(
                    || import.path.clone(),
                    |alias| format!("{} as {alias}", import.path),
                )
            })
            .collect();
        if let Some(panel) = self
            .agent_panel
            .and_then(|id| self.panels.get_mut(id))
            .and_then(AppPanel::as_text_mut)
        {
            panel.upsert_agent_module_imports(AgentBlock::ModuleImports {
                source: path
                    .strip_prefix(&self.workspace)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
                imports,
            });
        } else {
            self.message(Severity::Warning, "agent", "The Agent buffer is not open");
        }
    }

    pub(in crate::app) fn load_agent_callers(&mut self, requested: &str) {
        let requested = requested.trim();
        if requested.is_empty() {
            self.append_agent_diagnostic(
                "Teal callers require a source path and a symbol name",
                "Usage: :agent-callers <path> <symbol>",
            );
            return;
        }
        let (path_str, symbol_name) = match requested.split_once(char::is_whitespace) {
            Some(parts) => (parts.0, parts.1.trim()),
            None => {
                self.append_agent_diagnostic(
                    "Teal callers require both a source path and a symbol name",
                    "Usage: :agent-callers <path> <symbol>",
                );
                return;
            }
        };
        if symbol_name.is_empty() {
            self.append_agent_diagnostic(
                "Teal callers require a non-empty symbol name",
                "Usage: :agent-callers <path> <symbol>",
            );
            return;
        }
        let path = PathBuf::from(path_str);
        let path = if path.is_absolute() {
            path
        } else {
            self.workspace.join(path)
        };
        if let Err(error) = crate::app::ensure_workspace_path(&self.workspace, &path) {
            self.append_agent_diagnostic("Teal callers path is invalid", error);
            return;
        }
        let path = path.canonicalize().unwrap_or(path);
        let analysis = match self
            .agent_source_snapshot()
            .and_then(|snapshot| snapshot.analyze(&path))
        {
            Ok(source) => source,
            Err(error) => {
                self.append_agent_diagnostic(
                    "Teal callers could not read source",
                    error.to_string(),
                );
                return;
            }
        };
        let symbol = analysis.symbols().iter().find(|s| s.name == symbol_name);
        if symbol.is_none() {
            self.append_agent_diagnostic(
                "Teal callers symbol not found",
                format!("symbol=`{symbol_name}`"),
            );
            return;
        }
        let symbol = symbol.unwrap();
        let callers: Vec<String> = analysis
            .callers_of(symbol.id)
            .filter_map(|(_, caller_id)| {
                analysis
                    .symbols()
                    .get(caller_id.as_usize())
                    .map(|s| s.name.clone())
            })
            .collect();
        if let Some(panel) = self
            .agent_panel
            .and_then(|id| self.panels.get_mut(id))
            .and_then(AppPanel::as_text_mut)
        {
            panel.upsert_agent_callers(AgentBlock::Callers {
                source: path
                    .strip_prefix(&self.workspace)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
                symbol: symbol_name.to_owned(),
                callers,
            });
        } else {
            self.message(Severity::Warning, "agent", "The Agent buffer is not open");
        }
    }

    pub(in crate::app) fn load_agent_callees(&mut self, requested: &str) {
        let requested = requested.trim();
        if requested.is_empty() {
            self.append_agent_diagnostic(
                "Teal callees require a source path and a symbol name",
                "Usage: :agent-callees <path> <symbol>",
            );
            return;
        }
        let (path_str, symbol_name) = match requested.split_once(char::is_whitespace) {
            Some(parts) => (parts.0, parts.1.trim()),
            None => {
                self.append_agent_diagnostic(
                    "Teal callees require both a source path and a symbol name",
                    "Usage: :agent-callees <path> <symbol>",
                );
                return;
            }
        };
        if symbol_name.is_empty() {
            self.append_agent_diagnostic(
                "Teal callees require a non-empty symbol name",
                "Usage: :agent-callees <path> <symbol>",
            );
            return;
        }
        let path = PathBuf::from(path_str);
        let path = if path.is_absolute() {
            path
        } else {
            self.workspace.join(path)
        };
        if let Err(error) = crate::app::ensure_workspace_path(&self.workspace, &path) {
            self.append_agent_diagnostic("Teal callees path is invalid", error);
            return;
        }
        let path = path.canonicalize().unwrap_or(path);
        let analysis = match self
            .agent_source_snapshot()
            .and_then(|snapshot| snapshot.analyze(&path))
        {
            Ok(source) => source,
            Err(error) => {
                self.append_agent_diagnostic(
                    "Teal callees could not read source",
                    error.to_string(),
                );
                return;
            }
        };
        let symbol = analysis.symbols().iter().find(|s| s.name == symbol_name);
        if symbol.is_none() {
            self.append_agent_diagnostic(
                "Teal callees symbol not found",
                format!("symbol=`{symbol_name}`"),
            );
            return;
        }
        let symbol = symbol.unwrap();
        let callees: Vec<String> = analysis
            .callees_of(symbol.id)
            .filter_map(|(_, callee_id)| {
                analysis
                    .symbols()
                    .get(callee_id.as_usize())
                    .map(|s| s.name.clone())
            })
            .collect();
        if let Some(panel) = self
            .agent_panel
            .and_then(|id| self.panels.get_mut(id))
            .and_then(AppPanel::as_text_mut)
        {
            panel.upsert_agent_callees(AgentBlock::Callees {
                source: path
                    .strip_prefix(&self.workspace)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
                symbol: symbol_name.to_owned(),
                callees,
            });
        } else {
            self.message(Severity::Warning, "agent", "The Agent buffer is not open");
        }
    }

    pub(in crate::app) fn load_agent_symbols(&mut self, requested: &str) {
        let requested = requested.trim();
        if requested.is_empty() {
            self.append_agent_diagnostic(
                "Teal symbols require a source path",
                "Usage: :agent-symbols <path>",
            );
            return;
        }
        let path = PathBuf::from(requested);
        let path = if path.is_absolute() {
            path
        } else {
            self.workspace.join(path)
        };
        if let Err(error) = crate::app::ensure_workspace_path(&self.workspace, &path) {
            self.append_agent_diagnostic("Teal symbols path is invalid", error);
            return;
        }
        let path = path.canonicalize().unwrap_or(path);
        let analysis = match self
            .agent_source_snapshot()
            .and_then(|snapshot| snapshot.analyze(&path))
        {
            Ok(source) => source,
            Err(error) => {
                self.append_agent_diagnostic(
                    "Teal symbols could not read source",
                    error.to_string(),
                );
                return;
            }
        };
        let symbols: Vec<String> = analysis
            .symbols()
            .iter()
            .map(|symbol| {
                let kind = format!("{:?}", symbol.kind);
                match &symbol.detail {
                    Some(detail) => format!("{} ({kind}) - {detail}", symbol.name),
                    None => format!("{} ({kind})", symbol.name),
                }
            })
            .collect();
        let display_source = path
            .strip_prefix(&self.workspace)
            .unwrap_or(&path)
            .display()
            .to_string();
        if let Some(panel) = self
            .agent_panel
            .and_then(|id| self.panels.get_mut(id))
            .and_then(AppPanel::as_text_mut)
        {
            panel.upsert_agent_symbols(AgentBlock::Symbols {
                source: display_source,
                symbols,
            });
        } else {
            self.message(Severity::Warning, "agent", "The Agent buffer is not open");
        }
    }

    pub(in crate::app) fn load_agent_occurrences(&mut self, requested: &str) {
        let requested = requested.trim();
        if requested.is_empty() {
            self.append_agent_diagnostic(
                "Teal occurrences require a source path and a symbol name",
                "Usage: :agent-occurrences <path> <symbol>",
            );
            return;
        }
        let (path_str, symbol_name) = match requested.split_once(char::is_whitespace) {
            Some(parts) => (parts.0, parts.1.trim()),
            None => {
                self.append_agent_diagnostic(
                    "Teal occurrences require both a source path and a symbol name",
                    "Usage: :agent-occurrences <path> <symbol>",
                );
                return;
            }
        };
        if symbol_name.is_empty() {
            self.append_agent_diagnostic(
                "Teal occurrences require a non-empty symbol name",
                "Usage: :agent-occurrences <path> <symbol>",
            );
            return;
        }
        let path = PathBuf::from(path_str);
        let path = if path.is_absolute() {
            path
        } else {
            self.workspace.join(path)
        };
        if let Err(error) = crate::app::ensure_workspace_path(&self.workspace, &path) {
            self.append_agent_diagnostic("Teal occurrences path is invalid", error);
            return;
        }
        let path = path.canonicalize().unwrap_or(path);
        let analysis = match self
            .agent_source_snapshot()
            .and_then(|snapshot| snapshot.analyze(&path))
        {
            Ok(source) => source,
            Err(error) => {
                self.append_agent_diagnostic(
                    "Teal occurrences could not read source",
                    error.to_string(),
                );
                return;
            }
        };
        let symbol = analysis.symbols().iter().find(|s| s.name == symbol_name);
        if symbol.is_none() {
            self.append_agent_diagnostic(
                "Teal occurrences symbol not found",
                format!("symbol=`{symbol_name}`"),
            );
            return;
        }
        let symbol = symbol.unwrap();
        let spans: Vec<String> = analysis
            .occurrences(symbol.id, true)
            .map(|span| format!("{}..{}", span.start, span.start + span.size))
            .collect();
        if let Some(panel) = self
            .agent_panel
            .and_then(|id| self.panels.get_mut(id))
            .and_then(AppPanel::as_text_mut)
        {
            panel.upsert_agent_occurrences(AgentBlock::Occurrences {
                source: path
                    .strip_prefix(&self.workspace)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
                symbol: symbol_name.to_owned(),
                spans,
            });
        } else {
            self.message(Severity::Warning, "agent", "The Agent buffer is not open");
        }
    }

    /// 将 Graph 查询结果转换成只读摘要或诊断。
    pub(in crate::app) fn load_agent_visible(&mut self, requested: &str) {
        let requested = requested.trim();
        if requested.is_empty() {
            self.append_agent_diagnostic(
                "Teal visible symbols require a source path and a byte position",
                "Usage: :agent-visible <path> <byte_offset>",
            );
            return;
        }
        let (path_str, pos_str) = match requested.split_once(char::is_whitespace) {
            Some(parts) => (parts.0, parts.1.trim()),
            None => {
                self.append_agent_diagnostic(
                    "Teal visible symbols require both a source path and a byte position",
                    "Usage: :agent-visible <path> <byte_offset>",
                );
                return;
            }
        };
        let Ok(position) = pos_str.parse::<u32>() else {
            self.append_agent_diagnostic(
                "Teal visible symbols require a valid byte position",
                format!("position=`{pos_str}`"),
            );
            return;
        };
        let path = PathBuf::from(path_str);
        let path = if path.is_absolute() {
            path
        } else {
            self.workspace.join(path)
        };
        if let Err(error) = crate::app::ensure_workspace_path(&self.workspace, &path) {
            self.append_agent_diagnostic("Teal visible symbols path is invalid", error);
            return;
        }
        let path = path.canonicalize().unwrap_or(path);
        let analysis = match self
            .agent_source_snapshot()
            .and_then(|snapshot| snapshot.analyze(&path))
        {
            Ok(source) => source,
            Err(error) => {
                self.append_agent_diagnostic(
                    "Teal visible symbols could not read source",
                    error.to_string(),
                );
                return;
            }
        };
        let symbols: Vec<String> = analysis
            .visible_symbols(position)
            .map(|symbol| {
                let kind = format!("{:?}", symbol.kind);
                match &symbol.detail {
                    Some(detail) => format!("{} ({kind}) - {detail}", symbol.name),
                    None => format!("{} ({kind})", symbol.name),
                }
            })
            .collect();
        if let Some(panel) = self
            .agent_panel
            .and_then(|id| self.panels.get_mut(id))
            .and_then(AppPanel::as_text_mut)
        {
            panel.upsert_agent_visible(AgentBlock::Visible {
                source: path
                    .strip_prefix(&self.workspace)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
                position,
                symbols,
            });
        } else {
            self.message(Severity::Warning, "agent", "The Agent buffer is not open");
        }
    }

    pub(in crate::app) fn load_agent_type(&mut self, requested: &str) {
        let requested = requested.trim();
        if requested.is_empty() {
            self.append_agent_diagnostic(
                "Teal type requires a source path and a byte position",
                "Usage: :agent-type <path> <byte_offset>",
            );
            return;
        }
        let (path_str, pos_str) = match requested.split_once(char::is_whitespace) {
            Some(parts) => (parts.0, parts.1.trim()),
            None => {
                self.append_agent_diagnostic(
                    "Teal type requires both a source path and a byte position",
                    "Usage: :agent-type <path> <byte_offset>",
                );
                return;
            }
        };
        let Ok(position) = pos_str.parse::<u32>() else {
            self.append_agent_diagnostic(
                "Teal type requires a valid byte position",
                format!("position=`{pos_str}`"),
            );
            return;
        };
        let path = PathBuf::from(path_str);
        let path = if path.is_absolute() {
            path
        } else {
            self.workspace.join(path)
        };
        if let Err(error) = crate::app::ensure_workspace_path(&self.workspace, &path) {
            self.append_agent_diagnostic("Teal type path is invalid", error);
            return;
        }
        let path = path.canonicalize().unwrap_or(path);
        let analysis = match self
            .agent_source_snapshot()
            .and_then(|snapshot| snapshot.analyze(&path))
        {
            Ok(source) => source,
            Err(error) => {
                self.append_agent_diagnostic("Teal type could not read source", error.to_string());
                return;
            }
        };
        let symbol = analysis.symbol_at(position);
        let (name, kind, detail) = match symbol {
            Some(symbol) => (
                symbol.name.clone(),
                format!("{:?}", symbol.kind),
                symbol.detail.clone(),
            ),
            None => {
                self.append_agent_diagnostic(
                    "Teal type no symbol at position",
                    format!("position={position}"),
                );
                return;
            }
        };
        if let Some(panel) = self
            .agent_panel
            .and_then(|id| self.panels.get_mut(id))
            .and_then(AppPanel::as_text_mut)
        {
            panel.upsert_agent_type(AgentBlock::Type {
                source: path
                    .strip_prefix(&self.workspace)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
                position,
                name,
                kind,
                detail,
            });
        } else {
            self.message(Severity::Warning, "agent", "The Agent buffer is not open");
        }
    }

    pub(in crate::app) fn finish_agent_graph_load(
        &mut self,
        run: String,
        result: Result<Option<teal_agent::GraphSnapshot>, String>,
    ) {
        match result {
            Ok(Some(snapshot)) => self.append_agent_graph(run, snapshot),
            Ok(None) => {
                self.append_agent_diagnostic("Agent Graph was not found", format!("run={run}"))
            }
            Err(error) => self.append_agent_diagnostic("Agent Graph load failed", error),
        }
    }
}
