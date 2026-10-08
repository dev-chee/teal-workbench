//! 工作目录内的子进程执行与输出收集。
//!
//! 平台命令解释方式在服务内明确区分；敏感命令的用户批准必须在调用服务之前完成。

use std::{ffi::OsString, path::Path, process::Stdio};

use thiserror::Error;
use tokio::io::AsyncWriteExt as _;
use tokio::process::Command;

/// 捕获的子进程结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessOutput {
    /// 进程风格状态码；不可用时为 `-1`。
    pub status: i32,

    /// 标准输出后接标准错误的合并内容。
    pub output: String,
}

/// 启动或等待子进程失败。
#[derive(Debug, Error)]
pub enum ProcessError {
    /// 子进程启动前的持久审计写入失败。
    #[error("Process audit failed: {0}")]
    Audit(#[from] teal_security::AuditError),
    /// 子进程可能已执行，但审计结果无法确认。
    #[error("Process audit result unavailable; effect may be unknown: {0}")]
    AuditUnknown(teal_security::AuditError),
    /// 操作系统进程错误。
    #[error("Child process failed: {0}")]
    Io(#[from] std::io::Error),
}

/// 通过平台 shell 运行一条用户输入命令。
pub async fn run_command(command: &str, cwd: &Path) -> Result<ProcessOutput, ProcessError> {
    run_command_with_audit(command, cwd, &teal_security::process_context()).await
}

/// 使用宿主接收器通过平台 shell 启动子进程。
///
/// # Errors
/// 审计写入、进程启动或等待失败时返回错误。
#[tracing::instrument(name = "teal.process.run", skip_all)]
pub async fn run_command_with_audit(
    command: &str,
    cwd: &Path,
    context: &teal_security::AuditContext,
) -> Result<ProcessOutput, ProcessError> {
    let audit = begin_process(context)?;
    let result = run_command_inner(command, cwd).await;
    finish_process(audit, &result)?;
    result
}

async fn run_command_inner(command: &str, cwd: &Path) -> Result<ProcessOutput, ProcessError> {
    #[cfg(windows)]
    let mut child = {
        let mut child = Command::new("cmd.exe");
        child.args(["/D", "/S", "/C", command]);
        child
    };
    #[cfg(not(windows))]
    let mut child = {
        let mut child = Command::new("sh");
        child.args(["-lc", command]);
        child
    };
    let output = child.current_dir(cwd).kill_on_drop(true).output().await?;
    Ok(process_output(output))
}

/// 不经过 shell，以独立参数启动一个程序。
///
/// # Errors
/// 程序无法启动或等待时返回错误。
pub async fn run_program(
    program: &Path,
    arguments: &[String],
    cwd: &Path,
) -> Result<ProcessOutput, ProcessError> {
    run_program_with_audit(program, arguments, cwd, &teal_security::process_context()).await
}

/// 使用宿主接收器不经 shell 启动程序。
///
/// # Errors
/// 审计写入、进程启动或等待失败时返回错误。
#[tracing::instrument(name = "teal.process.run", skip_all)]
pub async fn run_program_with_audit(
    program: &Path,
    arguments: &[String],
    cwd: &Path,
    context: &teal_security::AuditContext,
) -> Result<ProcessOutput, ProcessError> {
    let audit = begin_process(context)?;
    let result = run_program_inner(program, arguments, cwd).await;
    finish_process(audit, &result)?;
    result
}

/// 使用独立参数、指定环境和可选标准输入运行程序，并分别捕获原始 stdout/stderr。
///
/// 供需要隔离 Git index 或向子进程传入已核对字节的工作台操作使用；不经 shell，
/// 输入与环境值不会写入受控审计事件。调用方仍负责本操作的资源批准。
///
/// # Errors
/// 审计意图、进程启动、标准输入写入、等待或结果审计失败时返回错误；结果审计失败可能
/// 表示进程效果未知。
#[tracing::instrument(name = "teal.process.run", skip_all)]
pub async fn run_program_with_input_and_environment(
    program: &Path,
    arguments: &[String],
    cwd: &Path,
    input: Option<&[u8]>,
    environment: &[(String, OsString)],
) -> Result<std::process::Output, ProcessError> {
    let audit = begin_process(&teal_security::process_context())?;
    let result =
        run_program_with_input_and_environment_inner(program, arguments, cwd, input, environment)
            .await;
    finish_process(audit, &result)?;
    result
}

async fn run_program_with_input_and_environment_inner(
    program: &Path,
    arguments: &[String],
    cwd: &Path,
    input: Option<&[u8]>,
    environment: &[(String, OsString)],
) -> Result<std::process::Output, ProcessError> {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(cwd)
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn()?;
    let output = if let Some(bytes) = input {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("child stdin is unavailable"))?;
        let write = async move { stdin.write_all(bytes).await };
        let (_, output) = tokio::try_join!(write, child.wait_with_output())?;
        output
    } else {
        child.wait_with_output().await?
    };
    Ok(output)
}

async fn run_program_inner(
    program: &Path,
    arguments: &[String],
    cwd: &Path,
) -> Result<ProcessOutput, ProcessError> {
    let output = Command::new(program)
        .args(arguments)
        .current_dir(cwd)
        .kill_on_drop(true)
        .output()
        .await?;
    Ok(process_output(output))
}

/// 在当前阻塞工作线程中通过平台 shell 运行命令。
///
/// 该入口与 [`run_command`] 使用相同的 shell、工作目录和输出合并契约，供同步 Host
/// 回调复用；调用方必须确保它不在异步执行器工作线程上直接阻塞。
pub fn run_command_blocking(command: &str, cwd: &Path) -> Result<ProcessOutput, ProcessError> {
    run_command_blocking_with_audit(command, cwd, &teal_security::process_context())
}

/// 使用宿主接收器在阻塞线程中通过平台 shell 启动子进程。
///
/// # Errors
/// 审计写入、进程启动或等待失败时返回错误。
#[tracing::instrument(name = "teal.process.run", skip_all)]
pub fn run_command_blocking_with_audit(
    command: &str,
    cwd: &Path,
    context: &teal_security::AuditContext,
) -> Result<ProcessOutput, ProcessError> {
    let audit = begin_process(context)?;
    let result = run_command_blocking_inner(command, cwd);
    finish_process(audit, &result)?;
    result
}

fn run_command_blocking_inner(command: &str, cwd: &Path) -> Result<ProcessOutput, ProcessError> {
    #[cfg(windows)]
    let mut child = {
        let mut child = std::process::Command::new("cmd.exe");
        child.args(["/D", "/S", "/C", command]);
        child
    };
    #[cfg(not(windows))]
    let mut child = {
        let mut child = std::process::Command::new("sh");
        child.args(["-lc", command]);
        child
    };
    let output = child.current_dir(cwd).output()?;
    Ok(process_output(output))
}

/// 在当前阻塞工作线程中以独立参数启动程序。
///
/// # Errors
/// 程序无法启动或等待时返回错误。
pub fn run_program_blocking(
    program: &Path,
    arguments: &[String],
    cwd: &Path,
) -> Result<ProcessOutput, ProcessError> {
    run_program_blocking_with_audit(program, arguments, cwd, &teal_security::process_context())
}

/// 使用宿主接收器在阻塞线程中不经 shell 启动程序。
///
/// # Errors
/// 审计写入、进程启动或等待失败时返回错误。
#[tracing::instrument(name = "teal.process.run", skip_all)]
pub fn run_program_blocking_with_audit(
    program: &Path,
    arguments: &[String],
    cwd: &Path,
    context: &teal_security::AuditContext,
) -> Result<ProcessOutput, ProcessError> {
    let audit = begin_process(context)?;
    let result = run_program_blocking_inner(program, arguments, cwd);
    finish_process(audit, &result)?;
    result
}

fn run_program_blocking_inner(
    program: &Path,
    arguments: &[String],
    cwd: &Path,
) -> Result<ProcessOutput, ProcessError> {
    let output = std::process::Command::new(program)
        .args(arguments)
        .current_dir(cwd)
        .output()?;
    Ok(process_output(output))
}

fn process_output(output: std::process::Output) -> ProcessOutput {
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    ProcessOutput {
        status: output.status.code().unwrap_or(-1),
        output: combined,
    }
}

fn begin_process(
    context: &teal_security::AuditContext,
) -> Result<teal_security::AuditAttempt, ProcessError> {
    let event = teal_security::Event::new(
        teal_security::Action::Process,
        teal_security::Phase::Intent,
        teal_security::Outcome::Pending,
        teal_security::new_correlation_id(),
    )?
    .with_actor(teal_security::Actor::Host);
    context.begin(event).map_err(ProcessError::Audit)
}

fn finish_process<T>(
    audit: teal_security::AuditAttempt,
    result: &Result<T, ProcessError>,
) -> Result<(), ProcessError> {
    audit
        .finish(if result.is_ok() {
            teal_security::Outcome::Succeeded
        } else {
            teal_security::Outcome::Failed
        })
        .map(|_| ())
        .map_err(ProcessError::AuditUnknown)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{
        run_command_blocking_with_audit, run_command_with_audit, run_program_blocking_with_audit,
        run_program_with_audit,
    };

    #[derive(Default)]
    struct TestAuditSink(Mutex<Vec<teal_security::Event>>);

    impl teal_security::AuditSink for TestAuditSink {
        fn append(
            &self,
            event: &teal_security::Event,
        ) -> Result<teal_security::Receipt, teal_security::AuditError> {
            let mut events = self.0.lock().unwrap();
            events.push(event.clone());
            Ok(teal_security::Receipt {
                sequence: events.len() as u64,
                hash: format!("{:064x}", events.len()),
            })
        }
    }

    fn audit_context() -> teal_security::AuditContext {
        teal_security::AuditContext::new(Arc::new(TestAuditSink::default()))
    }

    #[tokio::test]
    async fn blocking_and_async_commands_keep_the_same_status_and_merged_output() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let command = "echo stdout & echo stderr 1>&2 & exit /b 7";
        #[cfg(not(windows))]
        let command = "printf stdout; printf stderr >&2; exit 7";

        let audit = audit_context();
        let asynchronous = run_command_with_audit(command, directory.path(), &audit)
            .await
            .unwrap();
        let blocking = run_command_blocking_with_audit(command, directory.path(), &audit).unwrap();
        assert_eq!(blocking, asynchronous);
        assert_eq!(blocking.status, 7);
        assert!(blocking.output.contains("stdout"));
        assert!(blocking.output.contains("stderr"));
    }

    #[tokio::test]
    async fn blocking_and_async_programs_keep_the_same_arguments_contract() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let (program, arguments) = (
            std::path::PathBuf::from("cmd.exe"),
            vec!["/D".into(), "/S".into(), "/C".into(), "echo stable".into()],
        );
        #[cfg(not(windows))]
        let (program, arguments) = (
            std::path::PathBuf::from("sh"),
            vec!["-c".into(), "printf stable".into()],
        );

        let audit = audit_context();
        let asynchronous = run_program_with_audit(&program, &arguments, directory.path(), &audit)
            .await
            .unwrap();
        let blocking =
            run_program_blocking_with_audit(&program, &arguments, directory.path(), &audit)
                .unwrap();
        assert_eq!(blocking, asynchronous);
        assert_eq!(blocking.status, 0);
        assert!(blocking.output.contains("stable"));
    }

    #[test]
    fn missing_audit_sink_prevents_process_start() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let command = "echo started>marker.txt";
        #[cfg(not(windows))]
        let command = "printf started > marker.txt";

        let error = run_command_blocking_with_audit(
            command,
            directory.path(),
            &teal_security::AuditContext::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("audit sink is required"));
        assert!(!directory.path().join("marker.txt").exists());
    }
}
