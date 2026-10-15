//! Tests 窗口：`:test` 的后台运行、结果提交、失败跳转与单测重跑。
//!
//! 运行本身在阻塞线程上编译并执行，结果经 [`RuntimeEvent::TestsFinished`] 回到事件循环；
//! 窗口只持有显示投影，不保存编译程序或 VM 状态。

use super::*;
use crate::workbench::ui::{TestRow, TestRowStatus, TestsWidget};

/// Tests 窗口拥有的面板身份。
#[derive(Clone, Copy, Debug)]
pub(crate) struct TestsPanels {
    pub(crate) window: WindowId,
    pub(crate) list: PanelId,
    pub(crate) output: PanelId,
}

/// 一次测试运行的完整结果，跨线程传回事件循环。
#[derive(Clone, Debug)]
pub(crate) struct TestsOutcome {
    pub(crate) rows: Vec<TestRow>,
    pub(crate) summary: String,
    pub(crate) error: Option<String>,
}

/// 在阻塞线程上编译并运行一个脚本登记的 `@test`：返回显示结果与摘要。
pub(crate) fn run_tests_blocking(workspace: &Path, path: &Path, filter: &[String]) -> TestsOutcome {
    let profile = match configured_tier(workspace) {
        Ok(tier) => tier,
        Err(error) => {
            return TestsOutcome {
                rows: Vec::new(),
                summary: "Tests could not be run".into(),
                error: Some(error),
            };
        }
    };
    let source_text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            return TestsOutcome {
                rows: Vec::new(),
                summary: "Tests could not be run".to_owned(),
                error: Some(format!("Cannot read {}: {error}", path.display())),
            };
        }
    };
    let source = teal::Source::new(path.to_string_lossy().into_owned(), source_text.clone());
    let program = match teal::compile_source(
        source,
        &teal::FsSourceLoader,
        teal::CompileOptions::default().with_tier(profile),
    ) {
        Ok(program) => program,
        Err(error) => {
            return TestsOutcome {
                rows: Vec::new(),
                summary: "Tests could not be compiled".to_owned(),
                error: Some(format!("{error:?}")),
            };
        }
    };
    if program.tests().is_empty() {
        return TestsOutcome {
            rows: Vec::new(),
            summary: "No `@test` functions are registered in this program".to_owned(),
            error: None,
        };
    }
    let mut host = match teal::NativeHostRuntime::new(Vec::new(), false) {
        Ok(host) => host,
        Err(error) => {
            return TestsOutcome {
                rows: Vec::new(),
                summary: "Tests could not be run".to_owned(),
                error: Some(error.to_string()),
            };
        }
    };
    let report = match teal::run_program_tests(
        &program,
        &mut host,
        teal::VmOptions::for_tier(profile),
        teal::VmTestLimits::default(),
        filter,
    ) {
        Ok(report) => report,
        Err(error) => {
            return TestsOutcome {
                rows: Vec::new(),
                summary: "Tests could not be run".to_owned(),
                error: Some(error.to_string()),
            };
        }
    };
    let rows = report
        .results
        .iter()
        .map(|result| row_for(result, path, &source_text))
        .collect::<Vec<_>>();
    let summary = format!(
        "{} passed; {} failed; {} selected",
        report.passed(),
        report.failed(),
        report.results.len()
    );
    TestsOutcome {
        rows,
        summary,
        error: None,
    }
}

/// 把一次测试结果映射为窗口行，并用字节偏移换算声明行号。
fn row_for(result: &teal::TestResult, path: &Path, source: &str) -> TestRow {
    let start = (result.span_start as usize).min(source.len());
    let line = source[..start].matches('\n').count() as u32 + 1;
    let (status, message) = match &result.status {
        teal::TestStatus::Passed => (TestRowStatus::Passed, String::new()),
        teal::TestStatus::ExitCode(code) => {
            (TestRowStatus::ExitCode(*code), format!("exit code {code}"))
        }
        teal::TestStatus::TimedOut => (
            TestRowStatus::TimedOut,
            "exceeded the instruction budget".to_owned(),
        ),
        teal::TestStatus::Failed { kind, message } => {
            (TestRowStatus::Failed, failure_text(kind, message))
        }
    };
    TestRow {
        name: result.name.clone(),
        source: result
            .source
            .clone()
            .unwrap_or_else(|| path.to_string_lossy().into_owned()),
        path: Some(path.to_path_buf()),
        line,
        status,
        message,
        output: result.output.clone(),
    }
}

/// 抛出错误的类别就是消息本身时只显示一次。
fn failure_text(kind: &str, message: &str) -> String {
    if kind == message {
        message.to_owned()
    } else {
        format!("{kind}: {message}")
    }
}

/// 返回工作台脚本使用的运行时配置档名称。
fn configured_tier(workspace: &Path) -> Result<teal::RuntimeTier, String> {
    let effective =
        teal_security::load_declarations(Some(workspace)).map_err(|error| error.to_string())?;
    Ok(effective
        .category::<teal::RuntimeDocument>()
        .map_err(|error| error.to_string())?
        .document()
        .tier)
}

impl App {
    /// 处理 `:test [FILTER]`：编译聚焦脚本或显式路径登记的 `@test` 测试。
    pub(in crate::app) fn tests_command(&mut self, arguments: &[String]) {
        let (path, filter) = match arguments {
            [] => (None, Vec::new()),
            [filter] if !filter.ends_with(".tl") => (None, vec![filter.clone()]),
            [path] => (Some(path.clone()), Vec::new()),
            [path, filter] => (Some(path.clone()), vec![filter.clone()]),
            _ => {
                self.message(Severity::Error, "test", "Usage: :test [PATH] [FILTER]");
                return;
            }
        };
        let Some(path) = self.tests_target(path.as_deref()) else {
            return;
        };
        self.run_tests(path, filter);
    }

    /// 处理 `test-jump <INDEX>`：打开选中测试的声明位置。
    pub(in crate::app) fn tests_jump(&mut self, requested: &str) {
        let Some(row) = self.selected_test_row(requested) else {
            return;
        };
        let Some(path) = row.path.clone() else {
            self.message(Severity::Warning, "test", "The test has no source path");
            return;
        };
        self.queue_open(
            path,
            true,
            Some(serde_json::json!({
                "line": row.line.saturating_sub(1),
                "character": 0,
            })),
        );
    }

    /// 处理 `test-select <INDEX>`：选中一行并刷新输出面板。
    pub(in crate::app) fn tests_select(&mut self, requested: &str) {
        if requested.trim().parse::<usize>().is_ok() {
            let _ = self.selected_test_row(requested);
        }
        self.refresh_tests_output();
    }

    /// 处理 `test-rerun <INDEX>`：只重跑选中的测试。
    pub(in crate::app) fn tests_rerun(&mut self, requested: &str) {
        let Some(row) = self.selected_test_row(requested) else {
            return;
        };
        if self.tests_run_in_flight {
            self.message(
                Severity::Warning,
                "test",
                "A test run is already in progress",
            );
            return;
        }
        let Some(path) = self.tests_source.clone() else {
            self.message(Severity::Warning, "test", "No test source is selected");
            return;
        };
        self.run_tests(path, vec![row.name.clone()]);
    }

    /// 返回请求序号对应的测试行；省略序号时使用选中行。
    fn selected_test_row(&mut self, requested: &str) -> Option<TestRow> {
        let panels = self.tests_panels?;
        let widget = self
            .panels
            .get_mut(panels.list)
            .and_then(AppPanel::as_tests_mut)?;
        let index = requested.trim().parse::<usize>().ok();
        if let Some(index) = index {
            widget.select(index);
        }
        widget.selected_row().cloned()
    }

    /// 解析测试目标：显式路径优先，否则使用聚焦编辑器的文件。
    fn tests_target(&mut self, requested: Option<&str>) -> Option<PathBuf> {
        let candidate = match requested {
            Some(requested) => {
                let requested = PathBuf::from(requested);
                if requested.is_absolute() {
                    requested
                } else {
                    self.workspace.join(requested)
                }
            }
            None => {
                let panel = self.windows.focused();
                match self
                    .panels
                    .get(panel)
                    .and_then(AppPanel::as_editor)
                    .and_then(|editor| editor.document().path())
                {
                    Some(path) => path,
                    None => {
                        self.message(Severity::Warning, "test", "Usage: :test [PATH] [FILTER]");
                        return None;
                    }
                }
            }
        };
        let path = match candidate.canonicalize() {
            Ok(path) if path.starts_with(&self.workspace) => path,
            _ => {
                self.message(
                    Severity::Error,
                    "test",
                    "The test path does not exist or is outside the workspace",
                );
                return None;
            }
        };
        if path.extension().and_then(|extension| extension.to_str()) != Some("tl") {
            self.message(
                Severity::Error,
                "test",
                "Only .tl source files can be tested",
            );
            return None;
        }
        Some(path)
    }

    /// 打开或复用 Tests 窗口，并把运行中的状态写入列表与输出面板。
    fn run_tests(&mut self, path: PathBuf, filter: Vec<String>) {
        if self.tests_run_in_flight {
            self.message(
                Severity::Warning,
                "test",
                "A test run is already in progress",
            );
            return;
        }
        let panels = self.tests_window();
        self.tests_run_in_flight = true;
        self.tests_source = Some(path.clone());
        if let Some(widget) = self
            .panels
            .get_mut(panels.list)
            .and_then(AppPanel::as_tests_mut)
        {
            widget.replace(
                Vec::new(),
                filter.join(", "),
                format!("Running tests from {}", path.display()),
            );
        }
        if let Some(output) = self
            .panels
            .get_mut(panels.output)
            .and_then(AppPanel::as_text_mut)
        {
            output.replace_plain("Running tests…");
        }
        self.windows.select(panels.window);
        let output = self.runtime_tx.clone();
        let requested = filter.clone();
        let worker_path = path.clone();
        let workspace = self.workspace.clone();
        self.tasks.spawn(async move {
            let outcome = crate::runtime_bridge::spawn_blocking(move || {
                run_tests_blocking(&workspace, &worker_path, &requested)
            })
            .await
            .unwrap_or_else(|error| TestsOutcome {
                rows: Vec::new(),
                summary: "Tests could not be scheduled".to_owned(),
                error: Some(error.to_string()),
            });
            let _ = output
                .send(RuntimeEvent::TestsFinished {
                    path,
                    filter,
                    outcome,
                })
                .await;
        });
        self.message(
            Severity::Info,
            "test",
            format!("Running tests from {}", self.tests_source_label()),
        );
    }

    fn tests_source_label(&self) -> String {
        self.tests_source
            .as_ref()
            .map(|path| {
                path.strip_prefix(&self.workspace)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_default()
    }

    /// 打开或复用 Tests 窗口：列表在上，输出在下。
    pub(super) fn tests_window(&mut self) -> TestsPanels {
        if let Some(panels) = self.tests_panels
            && self.windows.get(panels.window).is_some()
        {
            return panels;
        }
        // 会话恢复或 `:window new tests` 可能已经存在 Tests 窗口；接管它而不是叠加重复窗口。
        if let Some(panels) = self.adopt_tests_window() {
            self.tests_panels = Some(panels);
            return panels;
        }
        let list = self.panels.insert(AppPanel::from(TestsWidget::new()));
        self.ensure_recovery_identity(list);
        let output = self.panels.insert(AppPanel::from(TextPanel::plain(
            PanelKind::Output,
            "Output",
            "",
        )));
        self.ensure_recovery_identity(output);
        let spec = LayoutSpec {
            focused: list,
            root: NodeSpec::Split {
                axis: SplitAxis::Rows,
                children: vec![
                    (PanelSize::fill(1).with_min(4), NodeSpec::Leaf(list)),
                    (PanelSize::percent(40).with_min(4), NodeSpec::Leaf(output)),
                ],
            },
        };
        let window = Window::from_spec(WindowKind::Tests, "Tests", spec)
            .expect("the Tests window template is valid");
        let window = self.windows.insert(window);
        let panels = TestsPanels {
            window,
            list,
            output,
        };
        self.tests_panels = Some(panels);
        panels
    }

    /// 接管已存在的 Tests 窗口；同窗口的文本面板作为输出面板。
    fn adopt_tests_window(&mut self) -> Option<TestsPanels> {
        let (window, list) = self.windows.iter().find_map(|(id, window)| {
            let panels = window.validate().ok()?;
            let list = panels.iter().copied().find(|panel| {
                self.panels
                    .get(*panel)
                    .is_some_and(|p| p.as_tests().is_some())
            })?;
            Some((id, list))
        })?;
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
        Some(TestsPanels {
            window,
            list,
            output,
        })
    }

    /// 把一次运行结果写入 Tests 窗口与输出面板。
    pub(crate) fn finish_tests(
        &mut self,
        path: PathBuf,
        filter: Vec<String>,
        outcome: TestsOutcome,
    ) {
        self.tests_run_in_flight = false;
        self.tests_source = Some(path.clone());
        let panels = self.tests_window();
        let source_label = path
            .strip_prefix(&self.workspace)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        if let Some(widget) = self
            .panels
            .get_mut(panels.list)
            .and_then(AppPanel::as_tests_mut)
        {
            widget.replace(
                outcome.rows.clone(),
                filter.join(", "),
                outcome.summary.clone(),
            );
        }
        if let Some(error) = &outcome.error {
            self.write_tests_output(format!("{source_label}: {error}"));
        } else {
            self.refresh_tests_output();
        }
        self.windows.select(panels.window);
        match &outcome.error {
            Some(error) => self.message(Severity::Error, "test", error.clone()),
            None => self.message(Severity::Info, "test", outcome.summary.clone()),
        }
    }

    /// 选中行的输出与失败说明；供 Tests 窗口输出面板使用。
    pub(crate) fn tests_output_for_selected(&mut self) -> Option<String> {
        let panels = self.tests_panels?;
        let row = self
            .panels
            .get_mut(panels.list)
            .and_then(AppPanel::as_tests_mut)
            .and_then(|widget| widget.selected_row().cloned())?;
        Some(render_test_output(&row))
    }

    /// 按当前选中行刷新输出面板；没有选中行时说明没有匹配的测试。
    pub(crate) fn refresh_tests_output(&mut self) {
        let text = self
            .tests_output_for_selected()
            .unwrap_or_else(|| "no matching tests".to_owned());
        self.write_tests_output(text);
    }

    fn write_tests_output(&mut self, text: String) {
        let Some(panels) = self.tests_panels else {
            return;
        };
        if let Some(output) = self
            .panels
            .get_mut(panels.output)
            .and_then(AppPanel::as_text_mut)
        {
            output.replace_plain(&text);
        }
    }
}

/// 渲染一个测试的输出面板：状态、说明、源码位置与捕获输出。
fn render_test_output(row: &TestRow) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "{}  {}", row.status.label(), row.name);
    if !row.message.is_empty() {
        let _ = writeln!(text, "{}", row.message);
    }
    if let Some(path) = &row.path {
        let _ = writeln!(text, "{}:{}", path.display(), row.line);
    }
    if row.output.trim().is_empty() {
        text.push_str("(no console output)\n");
    } else {
        text.push_str(&row.output);
    }
    text
}
