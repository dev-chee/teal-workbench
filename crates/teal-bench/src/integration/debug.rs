//! 使用 `teal-vm` 共享调试会话状态的内嵌调试工作线程。

use std::{
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
};

use teal_compiler::{CompileOptions, OptimizationLevel, RuntimeProfile, Source, compile};
use teal_host::NativeHostRuntime;
use teal_vm::{
    DebugLocation, DebugSession, DebugStopReason, Vm, VmDebugControl, VmDebugException,
    VmDebugState, VmDebugger,
};

use crate::{app::App, presentation::TextPanel, runtime_bridge::RuntimeEvent};
use teal_workbench_core::{SplitAxis, Severity};
use crate::workbench::ui::PanelKind;

/// 工作台调试面板接收的拥有型状态。
pub(crate) enum DebugEvent {
    Stopped {
        reason: DebugStopReason,
        location: DebugLocation,
        frames: Vec<String>,
        variables: Vec<String>,
        /// 当前检查的帧序号；0 是入口帧。
        frame: usize,
    },
    /// 断点集合的校验结果。
    Breakpoints {
        source: String,
        verified: Vec<(u32, bool)>,
    },
    /// 一次表达式求值的结果。
    Evaluated {
        expression: String,
        result: Result<String, String>,
    },
    Finished(Result<i32, String>),
}

#[derive(Clone)]
pub(crate) enum DebugCommand {
    Continue,
    Next,
    StepIn,
    StepOut,
    Stop,
    /// 替换当前源码的完整断点集合。
    SetBreakpoints(Vec<u32>),
    /// 选择要检查的帧序号。
    SelectFrame(usize),
    /// 在当前暂停帧上求值一个纯表达式。
    Evaluate(String),
}

struct BenchDebugger {
    session: DebugSession,
    commands: Receiver<DebugCommand>,
    output: crate::runtime_bridge::RuntimeSender,
    source: String,
    breakpoints: Vec<u32>,
    frame: usize,
}

impl BenchDebugger {
    fn pause(&mut self, state: &mut VmDebugState<'_>, reason: DebugStopReason) -> VmDebugControl {
        self.session.mark_paused(state);
        // 暂停期间持续接收命令：选择帧和求值只刷新面板，不继续执行；只有步进和继续
        // 才结束本次暂停，命令通道关闭等价于终止。
        // 每次停下默认检查最内层帧；`:debug-frame` 只影响当前暂停。
        self.frame = state.frames().len().saturating_sub(1);
        let mut refresh = true;
        loop {
            self.frame = self.frame.min(state.frames().len().saturating_sub(1));
            let location = self.session.location(state);
            if refresh {
                let frames = state
                    .frames()
                    .iter()
                    .enumerate()
                    .rev()
                    .map(|(index, frame)| {
                        let source = frame.source_name.as_deref().unwrap_or("<bytecode>");
                        format!(
                            "{}#{}  {}:{}",
                            frame.name,
                            index,
                            source,
                            self.session.source_line(source, frame.start.unwrap_or(0))
                        )
                    })
                    .collect::<Vec<_>>();
                let variables = state
                    .variables(self.frame)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|variable| {
                        format!(
                            "{}: {} = {}",
                            variable.name, variable.declared_type, variable.display
                        )
                    })
                    .collect();
                if self
                    .output
                    .blocking_send(RuntimeEvent::Debug(DebugEvent::Stopped {
                        reason,
                        location,
                        frames,
                        variables,
                        frame: self.frame,
                    }))
                    .is_err()
                {
                    return VmDebugControl::Terminate;
                }
            }
            refresh = false;
            match self.commands.recv() {
                Ok(DebugCommand::Continue) => {
                    self.session.continue_run();
                    return VmDebugControl::Continue;
                }
                Ok(DebugCommand::Next) => {
                    self.session.next(state);
                    return VmDebugControl::Continue;
                }
                Ok(DebugCommand::StepIn) => {
                    self.session.step_in(state);
                    return VmDebugControl::Continue;
                }
                Ok(DebugCommand::StepOut) => {
                    self.session.step_out(state);
                    return VmDebugControl::Continue;
                }
                Ok(DebugCommand::SetBreakpoints(lines)) => {
                    self.breakpoints = lines;
                    let verified = self
                        .session
                        .set_breakpoints(&self.source, self.breakpoints.iter().copied())
                        .into_iter()
                        .map(|breakpoint| (breakpoint.line, breakpoint.verified))
                        .collect();
                    if self
                        .output
                        .blocking_send(RuntimeEvent::Debug(DebugEvent::Breakpoints {
                            source: self.source.clone(),
                            verified,
                        }))
                        .is_err()
                    {
                        return VmDebugControl::Terminate;
                    }
                }
                Ok(DebugCommand::SelectFrame(index)) => {
                    self.frame = index;
                    refresh = true;
                }
                Ok(DebugCommand::Evaluate(expression)) => {
                    let result = match teal_dap::expression::parse(&expression) {
                        Ok(parsed) => state
                            .evaluate(self.frame, &parsed)
                            .map(|variable| variable.display)
                            .map_err(|error| error.to_string()),
                        Err(error) => Err(error),
                    };
                    if self
                        .output
                        .blocking_send(RuntimeEvent::Debug(DebugEvent::Evaluated {
                            expression,
                            result,
                        }))
                        .is_err()
                    {
                        return VmDebugControl::Terminate;
                    }
                }
                Ok(DebugCommand::Stop) | Err(_) => return VmDebugControl::Terminate,
            }
        }
    }
}

impl VmDebugger for BenchDebugger {
    const ENABLED: bool = true;

    fn before_instruction(&mut self, state: &mut VmDebugState<'_>) -> VmDebugControl {
        self.session
            .before_instruction(state)
            .map_or(VmDebugControl::Continue, |reason| self.pause(state, reason))
    }

    fn exception(
        &mut self,
        state: &mut VmDebugState<'_>,
        exception: &VmDebugException,
    ) -> VmDebugControl {
        self.session
            .exception(exception)
            .map_or(VmDebugControl::Continue, |reason| self.pause(state, reason))
    }
}

impl App {
    pub(crate) fn start_debug(&mut self, requested: &str) {
        if requested.trim().is_empty() {
            self.message(
                Severity::Error,
                "debug",
                "Usage: :debug <.tl file in workspace>",
            );
            return;
        }
        if self.debug_control.is_some() {
            self.message(
                Severity::Warning,
                "debug",
                "There is an active debugging session",
            );
            return;
        }
        let path = self.workspace.join(requested);
        let path = match path.canonicalize() {
            Ok(path) if path.starts_with(&self.workspace) => path,
            _ => {
                self.message(
                    Severity::Error,
                    "debug",
                    "The debug path does not exist or is outside the workspace",
                );
                return;
            }
        };
        if path.extension().and_then(|extension| extension.to_str()) != Some("tl") {
            self.message(
                Severity::Error,
                "debug",
                "Only .tl source files can be debugged",
            );
            return;
        }
        let (commands, received) = mpsc::channel();
        self.debug_control = Some(commands);
        // 调试会话绑定到 Debug 窗口与当前 Task Flow；关闭窗口或 Flow 结束时绑定清除，
        // 面板身份不再复用，避免事件写入已经属于其他工作流的窗口。
        let (window, panel) = self.bind_debug_session();
        self.debug_panel = Some(panel);
        self.debug_window = Some(window);
        let output = self.runtime_tx.clone();
        let worker_output = output.clone();
        self.tasks.spawn(async move {
            let result = crate::runtime_bridge::spawn_blocking(move || {
                run_debug(path, received, worker_output)
            })
            .await;
            let result = result
                .map_err(|error| error.to_string())
                .and_then(|result| result);
            let _ = output
                .send(RuntimeEvent::Debug(DebugEvent::Finished(result)))
                .await;
        });
    }

    pub(crate) fn debug_command(&mut self, command: DebugCommand) {
        let Some(control) = &self.debug_control else {
            self.message(Severity::Warning, "debug", "No active debugging session");
            return;
        };
        if control.send(command).is_err() {
            self.debug_control = None;
            self.message(
                Severity::Error,
                "debug",
                "The debugging worker thread has ended",
            );
        }
    }

    /// 终止当前调试会话并清除窗口与 Flow 绑定；没有活动会话时是空操作。
    ///
    /// 关闭承载调试输出的窗口时使用该入口：工作线程收到停止命令后正常退出，迟到的终态
    /// 事件不会再创建或写入任何面板。
    pub(crate) fn stop_debug_session(&mut self, reason: &str) {
        if let Some(control) = self.debug_control.take() {
            let _ = control.send(DebugCommand::Stop);
        }
        self.debug_window = None;
        self.debug_panel = None;
        self.message(Severity::Info, "debug", reason);
    }

    /// 处理 `:break [LINE...|clear]`：替换当前调试源码的完整断点集合。
    pub(crate) fn debug_breakpoints(&mut self, arguments: &[String]) {
        if self.debug_control.is_none() {
            self.message(Severity::Warning, "debug", "No active debugging session");
            return;
        }
        let lines = match arguments.first().map(String::as_str) {
            Some("clear" | "none") if arguments.len() == 1 => Vec::new(),
            Some(_) => {
                let mut lines = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    match argument.parse::<u32>() {
                        Ok(line) if line > 0 => lines.push(line),
                        _ => {
                            self.message(Severity::Error, "debug", "Usage: :break [LINE...|clear]");
                            return;
                        }
                    }
                }
                lines
            }
            None => {
                self.message(Severity::Error, "debug", "Usage: :break [LINE...|clear]");
                return;
            }
        };
        self.debug_command(DebugCommand::SetBreakpoints(lines));
    }

    /// 处理 `:debug-frame <INDEX>`：选择要检查的帧。
    pub(crate) fn debug_select_frame(&mut self, requested: &str) {
        let Ok(index) = requested.trim().parse::<usize>() else {
            self.message(
                Severity::Error,
                "debug",
                "Usage: :debug-frame <frame index>",
            );
            return;
        };
        self.debug_command(DebugCommand::SelectFrame(index));
    }

    /// 处理 `:debug-eval <EXPRESSION>`：在暂停帧上求值一个无副作用表达式。
    pub(crate) fn debug_evaluate(&mut self, arguments: &[String]) {
        let expression = arguments.join(" ");
        if expression.trim().is_empty() {
            self.message(Severity::Error, "debug", "Usage: :debug-eval <expression>");
            return;
        }
        self.debug_command(DebugCommand::Evaluate(expression));
    }

    pub(crate) fn handle_debug_event(&mut self, event: DebugEvent) {
        // 会话已经终止（例如窗口被关闭）后，迟到的停止或结束事件不得重新创建面板。
        if self.debug_window.is_none() && self.debug_panel.is_none() {
            return;
        }
        let panel = if let Some(panel) = self.debug_panel {
            panel
        } else {
            let panel = self.add_panel(
                TextPanel::plain(PanelKind::Output, "Debugging", ""),
                SplitAxis::Columns,
            );
            self.debug_panel = Some(panel);
            panel
        };
        let Some(output) = self
            .panels
            .get_mut(panel)
            .and_then(|panel| panel.as_text_mut())
        else {
            // 绑定的面板已被释放：结束会话，而不是把事件静默写进不存在的面板。
            self.stop_debug_session("Debugging session stopped because its panel was released");
            return;
        };
        match event {
            DebugEvent::Stopped {
                reason,
                location,
                frames,
                variables,
                frame,
            } => {
                output.push(format!(
                    "[Stop: {}] {}: {} (frame {frame})",
                    reason.name(),
                    location.source,
                    location.line
                ));
                output.push("Call stack:");
                for frame in frames {
                    output.push(format!("  {frame}"));
                }
                output.push("Variables:");
                for variable in variables {
                    output.push(format!("  {variable}"));
                }
            }
            DebugEvent::Breakpoints { source, verified } => {
                output.push(format!("Breakpoints for {source}:"));
                if verified.is_empty() {
                    output.push("  (none; exception stop on entry only)".to_owned());
                }
                for (line, verified) in verified {
                    output.push(format!(
                        "  {}:{line} {}",
                        source,
                        if verified { "verified" } else { "unverified" }
                    ));
                }
            }
            DebugEvent::Evaluated { expression, result } => match result {
                Ok(value) => output.push(format!("{expression} = {value}")),
                Err(error) => output.push(format!("{expression} failed: {error}")),
            },
            DebugEvent::Finished(result) => {
                self.debug_control = None;
                self.debug_window = None;
                match result {
                    Ok(code) => output.push(format!("[End of debugging: exit code {code}]")),
                    Err(error) => output.push(format!("[Debug failed] {error}")),
                }
            }
        }
    }
}

fn run_debug(
    path: PathBuf,
    commands: Receiver<DebugCommand>,
    output: crate::runtime_bridge::RuntimeSender,
) -> Result<i32, String> {
    let contents = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let compiled = compile(
        Source::new(path.to_string_lossy(), contents),
        &teal::FsSourceLoader,
        CompileOptions::default()
            .with_runtime_profile(RuntimeProfile::core())
            .with_optimization(OptimizationLevel::None)
            .with_debug_info(true),
    )
    .map_err(|error| error.message)?;
    let session = DebugSession::new(&compiled.program.source_map, true);
    let mut host = NativeHostRuntime::builder()
        .trusted()
        .trusted()
        .authorization(Arc::new(teal::HostExecutionGrant::core_only(
            "teal-bench/debug/v1",
        )))
        .build()
        .map_err(|error| error.to_string())?;
    let mut vm = Vm::load_with_host_runtime(
        compiled.program,
        teal_vm::VmOptions::for_profile(RuntimeProfile::core()),
        &mut host,
    )
    .map_err(|error| error.to_string())?;
    let mut debugger = BenchDebugger {
        session,
        commands,
        output,
        source: path.to_string_lossy().into_owned(),
        breakpoints: Vec::new(),
        frame: 0,
    };
    vm.execute_debugged(&mut host, &mut debugger)
        .map(|result| result.exit_code)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 读取下一个调试事件，跳过观测信封。
    fn next_debug_event(events: &mut tokio::sync::mpsc::Receiver<RuntimeEvent>) -> DebugEvent {
        let event = events
            .blocking_recv()
            .expect("the debugger should report an event");
        let mut event = event;
        while let RuntimeEvent::Observed { event: inner, .. } = event {
            event = *inner;
        }
        match event {
            RuntimeEvent::Debug(event) => event,
            _ => panic!("expected a debug event"),
        }
    }

    #[test]
    fn embedded_debugger_sets_breakpoints_selects_frames_and_evaluates_expressions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("debug.tl");
        std::fs::write(
            &path,
            "fn compute() -> int { let inner = 2; return inner; }\nfn main() -> int { let answer = compute(); return answer; }\n",
        )
        .unwrap();
        let (runtime, mut events) = tokio::sync::mpsc::channel(16);
        let output = crate::runtime_bridge::RuntimeSender::new(runtime);
        let (commands, received) = mpsc::channel();
        let worker = std::thread::spawn(move || run_debug(path, received, output));

        // 入口暂停：登记断点后继续执行，直到命中 compute 内的断点。
        let DebugEvent::Stopped { reason, .. } = next_debug_event(&mut events) else {
            panic!("entry stop expected");
        };
        assert_eq!(reason, DebugStopReason::Entry);
        commands
            .send(DebugCommand::SetBreakpoints(vec![1]))
            .unwrap();
        let DebugEvent::Breakpoints { verified, .. } = next_debug_event(&mut events) else {
            panic!("breakpoint verification expected");
        };
        assert_eq!(verified, [(1, true)]);
        commands.send(DebugCommand::Continue).unwrap();

        let DebugEvent::Stopped {
            reason,
            frames,
            variables,
            frame,
            ..
        } = next_debug_event(&mut events)
        else {
            panic!("breakpoint stop expected");
        };
        assert_eq!(reason, DebugStopReason::Breakpoint);
        assert!(frames.len() >= 2, "{frames:?}");
        assert_eq!(frame, frames.len() - 1, "the innermost frame is inspected");
        assert!(
            variables.iter().any(|variable| variable.contains("inner")),
            "{variables:?}"
        );

        // 表达式在最内层帧上求值。
        commands
            .send(DebugCommand::Evaluate("inner".to_owned()))
            .unwrap();
        let DebugEvent::Evaluated { expression, result } = next_debug_event(&mut events) else {
            panic!("evaluation result expected");
        };
        assert_eq!(expression, "inner");
        assert_eq!(result.as_deref(), Ok("2"));

        // 选择外层帧后变量集合随之变化，同一表达式在外层帧中不再可见。
        commands.send(DebugCommand::SelectFrame(0)).unwrap();
        let DebugEvent::Stopped { frame, .. } = next_debug_event(&mut events) else {
            panic!("frame selection should refresh the stop state");
        };
        assert_eq!(frame, 0);
        commands
            .send(DebugCommand::Evaluate("inner".to_owned()))
            .unwrap();
        let DebugEvent::Evaluated { result, .. } = next_debug_event(&mut events) else {
            panic!("evaluation result expected");
        };
        assert!(
            result
                .as_deref()
                .is_err_and(|error| error.contains("inner")),
            "{result:?}"
        );

        commands.send(DebugCommand::Stop).unwrap();
        // `Finished` 由应用包装层在 `run_debug` 返回后发出；直接驱动工作线程时
        // 只能断言终止本身以错误结束，会话不再报告退出码。
        assert!(worker.join().unwrap().is_err());
    }

    #[test]
    fn embedded_debugger_stops_on_entry_and_resumes_to_exit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("debug.tl");
        std::fs::write(
            &path,
            " fn main() -> int { let answer = 42; return answer; }",
        )
        .unwrap();
        let (runtime, mut events) = tokio::sync::mpsc::channel(8);
        let output = crate::runtime_bridge::RuntimeSender::new(runtime);
        let (commands, received) = mpsc::channel();
        let worker = std::thread::spawn(move || run_debug(path, received, output));
        let event = events
            .blocking_recv()
            .expect("The debugger should report that entry has stopped");
        let RuntimeEvent::Observed { event, .. } = event else {
            panic!("Debug events should have observation context");
        };
        let RuntimeEvent::Debug(DebugEvent::Stopped {
            reason,
            frames,
            variables,
            ..
        }) = *event
        else {
            panic!("A debug stop event should be received");
        };
        assert_eq!(reason, DebugStopReason::Entry);
        assert!(!frames.is_empty());
        assert!(
            variables
                .iter()
                .all(|variable| !variable.contains("answer = 42"))
        );
        commands.send(DebugCommand::Continue).unwrap();
        assert_eq!(worker.join().unwrap().unwrap(), 42);
    }
}
