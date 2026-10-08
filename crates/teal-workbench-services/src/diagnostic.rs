//! Cargo JSON 诊断与只读源码/Git 上下文。

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};

/// Cargo 编译消息中的源码范围。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticSpan {
    /// Cargo 返回的源码路径。
    pub file: String,
    /// UTF-8 起始字节。
    pub byte_start: u32,
    /// UTF-8 结束字节。
    pub byte_end: u32,
    /// 从一开始的行号。
    pub line: u32,
    /// 从一开始的列号。
    pub column: u32,
    /// 该范围的可选说明。
    pub label: Option<String>,
    /// 是否为主范围。
    pub primary: bool,
}

/// 一条结构化 Cargo/rustc 诊断。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CargoDiagnostic {
    /// `error`、`warning` 等稳定级别。
    pub level: String,
    /// 可选 rustc 错误码。
    pub code: Option<String>,
    /// 主消息。
    pub message: String,
    /// 主范围。
    pub primary: Option<DiagnosticSpan>,
    /// 其余相关范围。
    pub related: Vec<DiagnosticSpan>,
    /// 有界附属说明。
    pub children: Vec<String>,
}

/// 一条诊断附近的工作区和 Git 来源。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticContext {
    /// 规范工作区相对路径。
    pub path: String,
    /// 诊断行号。
    pub line: u32,
    /// 上下文第一行。
    pub context_start_line: u32,
    /// 最多七行源码上下文。
    pub context: Vec<String>,
    /// `git blame` 返回的提交；未提交行为空。
    pub commit: Option<String>,
    /// 提交主题。
    pub commit_summary: Option<String>,
}

/// 解析 Cargo `--message-format=json` 的逐行输出，忽略非诊断与损坏行。
#[must_use]
pub fn parse_cargo_diagnostics(output: &str, limit: usize) -> Vec<CargoDiagnostic> {
    output
        .lines()
        .filter_map(parse_message)
        .take(limit)
        .collect()
}

/// 读取诊断附近源码并查询该行的 Git 来源，不修改仓库。
///
/// `overlay` 用于传入共享缓冲区中的未保存文本；省略时读取磁盘。
pub fn diagnostic_context(
    workspace: &Path,
    span: &DiagnosticSpan,
    overlay: Option<&str>,
) -> Result<DiagnosticContext, String> {
    let root = workspace
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let requested = PathBuf::from(&span.file);
    let path = if requested.is_absolute() {
        requested
    } else {
        root.join(requested)
    };
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err("Diagnostic source is outside the workspace".into());
    }
    let source = overlay.map(str::to_owned).map_or_else(
        || std::fs::read_to_string(&path).map_err(|error| error.to_string()),
        Ok,
    )?;
    let lines = source.lines().collect::<Vec<_>>();
    let index = usize::try_from(span.line.saturating_sub(1)).unwrap_or(usize::MAX);
    let start = index.saturating_sub(3).min(lines.len());
    let end = index.saturating_add(4).min(lines.len());
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| "Diagnostic source is outside the workspace".to_owned())?;
    let blame = Command::new("git")
        .args([
            "blame",
            "--line-porcelain",
            "-L",
            &format!("{},{}", span.line, span.line),
            "--",
        ])
        .arg(relative)
        .current_dir(&root)
        .output()
        .ok()
        .filter(|output| output.status.success());
    let (commit, commit_summary) = blame.map_or((None, None), |output| {
        let text = String::from_utf8_lossy(&output.stdout);
        let mut rows = text.lines();
        let hash = rows
            .next()
            .and_then(|line| line.split_whitespace().next())
            .filter(|hash| !hash.bytes().all(|byte| byte == b'0'))
            .map(str::to_owned);
        let summary = rows.find_map(|line| line.strip_prefix("summary ").map(str::to_owned));
        (hash, summary)
    });
    Ok(DiagnosticContext {
        path: relative.to_string_lossy().replace('\\', "/"),
        line: span.line,
        context_start_line: u32::try_from(start + 1).unwrap_or(u32::MAX),
        context: lines[start..end]
            .iter()
            .map(|line| (*line).to_owned())
            .collect(),
        commit,
        commit_summary,
    })
}

fn parse_message(line: &str) -> Option<CargoDiagnostic> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("reason")?.as_str()? != "compiler-message" {
        return None;
    }
    let message = value.get("message")?;
    let spans = message
        .get("spans")?
        .as_array()?
        .iter()
        .filter_map(parse_span)
        .collect::<Vec<_>>();
    let primary = spans.iter().find(|span| span.primary).cloned();
    let mut related = spans
        .into_iter()
        .filter(|span| !span.primary)
        .collect::<Vec<_>>();
    let mut children = Vec::new();
    for child in message
        .get("children")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .take(16)
    {
        if let Some(text) = child.get("message").and_then(serde_json::Value::as_str) {
            children.push(text.to_owned());
        }
        related.extend(
            child
                .get("spans")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(parse_span),
        );
    }
    Some(CargoDiagnostic {
        level: message.get("level")?.as_str()?.to_owned(),
        code: message
            .get("code")
            .and_then(|code| code.get("code"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        message: message.get("message")?.as_str()?.to_owned(),
        primary,
        related,
        children,
    })
}

fn parse_span(value: &serde_json::Value) -> Option<DiagnosticSpan> {
    Some(DiagnosticSpan {
        file: value.get("file_name")?.as_str()?.to_owned(),
        byte_start: u32::try_from(value.get("byte_start")?.as_u64()?).ok()?,
        byte_end: u32::try_from(value.get("byte_end")?.as_u64()?).ok()?,
        line: u32::try_from(value.get("line_start")?.as_u64()?).ok()?,
        column: u32::try_from(value.get("column_start")?.as_u64()?).ok()?,
        label: value
            .get("label")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        primary: value.get("is_primary")?.as_bool()?,
    })
}
