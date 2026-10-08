//! 本地记录与导出跨度、日志共享的默认拒绝字段策略。

use opentelemetry::{KeyValue, Value};
use std::{
    path::{Component, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug)]
pub(crate) struct Policy(Arc<Inner>);

struct Inner {
    workspace: PathBuf,
    secrets: Vec<String>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivacyPolicy")
    }
}

impl Policy {
    pub fn new(workspace: PathBuf, mut secrets: Vec<String>) -> Self {
        secrets.extend(
            secrets
                .clone()
                .iter()
                .filter_map(|value| value.strip_prefix("Bearer ").map(str::to_owned)),
        );
        secrets.retain(|secret| !secret.is_empty());
        secrets.sort_by_key(|value| std::cmp::Reverse(value.len()));
        Self(Arc::new(Inner {
            workspace: normalized(workspace),
            secrets,
        }))
    }

    pub fn text(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for secret in &self.0.secrets {
            text = text.replace(secret, "[redacted]");
        }
        text.chars()
            .filter(|ch| !ch.is_control())
            .take(256)
            .collect()
    }

    pub fn path(&self, text: &str) -> String {
        if text.contains("://") {
            return "<external>".into();
        }
        if text.starts_with("<builtin:") {
            return "<builtin>".into();
        }
        let normalized = normalized(PathBuf::from(text));
        let path = normalized.as_path();
        let relative = if path.is_absolute() {
            match path.strip_prefix(&self.0.workspace) {
                Ok(path) => path,
                Err(_) => return "<external>".into(),
            }
        } else {
            path
        };
        if relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        {
            return "<external>".into();
        }
        self.text(&relative.to_string_lossy().replace('\\', "/"))
    }

    pub fn field(&self, key: &str, value: &Value) -> Option<KeyValue> {
        let value = match (key, value) {
            ("file.path" | "teal.source.name", Value::String(value)) => {
                Value::from(self.path(value.as_str()))
            }
            ("operation.result", Value::String(value))
                if matches!(value.as_str(), "ok" | "error" | "cancelled" | "pending") =>
            {
                value.clone().into()
            }
            ("event.name", Value::String(value))
                if matches!(
                    value.as_str(),
                    "started"
                        | "completed"
                        | "failed"
                        | "cancelled"
                        | "shutdown"
                        | "diagnostic"
                        | "queue_full"
                ) =>
            {
                value.clone().into()
            }
            ("error.type", Value::String(value))
                if matches!(
                    value.as_str(),
                    "Compile" | "Load" | "Runtime" | "Io" | "cancelled" | "operation"
                ) =>
            {
                value.clone().into()
            }
            ("model.protocol", Value::String(value))
                if matches!(
                    value.as_str(),
                    "responses" | "chat_completions" | "anthropic"
                ) =>
            {
                value.clone().into()
            }
            ("model.phase", Value::String(value))
                if matches!(value.as_str(), "login" | "catalog" | "commit") =>
            {
                value.clone().into()
            }
            ("model.failure_stage", Value::String(value))
                if matches!(
                    value.as_str(),
                    "selection"
                        | "preflight"
                        | "preparation"
                        | "encoding"
                        | "authentication"
                        | "sse_frame"
                        | "stream_end"
                        | "json_decode"
                        | "provider_event"
                        | "event_decode"
                        | "event_delivery"
                        | "assembly"
                        | "capture"
                        | "http"
                ) =>
            {
                value.clone().into()
            }
            ("model.assembly_error", Value::String(value))
                if matches!(
                    value.as_str(),
                    "too_large" | "protocol" | "invalid_arguments" | "incomplete"
                ) =>
            {
                value.clone().into()
            }
            ("model.stop_reason", Value::String(value))
                if matches!(
                    value.as_str(),
                    "end_turn"
                        | "tool_use"
                        | "continue"
                        | "output_limit"
                        | "context_limit"
                        | "refused"
                        | "cancelled"
                ) =>
            {
                value.clone().into()
            }
            (
                "operation.id"
                | "audit.sequence"
                | "request.id"
                | "buffer.revision"
                | "queue.depth"
                | "duration_ms"
                | "teal.exit_code"
                | "teal.instructions"
                | "teal.gc.reclaimed"
                | "teal.frame.id"
                | "model.models_count"
                | "model.request_bytes"
                | "model.response_bytes"
                | "model.sse_events"
                | "model.events"
                | "model.pending_tools"
                | "http.status_code",
                Value::I64(_) | Value::F64(_) | Value::Bool(_),
            ) => value.clone(),
            _ => return None,
        };
        Some(KeyValue::new(key.to_owned(), value))
    }

    pub fn attributes(&self, values: &mut Vec<KeyValue>) {
        *values = values
            .iter()
            .filter_map(|kv| self.field(kv.key.as_str(), &kv.value))
            .collect();
    }
}

fn normalized(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(native) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(native);
        }
    }
    path
}

pub(crate) fn operation(name: &str) -> &'static str {
    match name {
        "teal.bench.startup" => "teal.bench.startup",
        "teal.bench.script" => "teal.bench.script",
        "teal.bench.file.open" => "teal.bench.file.open",
        "teal.bench.file.mutate" => "teal.bench.file.mutate",
        "teal.bench.syntax" => "teal.bench.syntax",
        "teal.lsp.client.request" => "teal.lsp.client.request",
        "teal.lsp.connection" => "teal.lsp.connection",
        "teal.dap.request" => "teal.dap.request",
        "teal.model.request" => "teal.model.request",
        "teal.model.activate" => "teal.model.activate",
        "teal.model.prepare" => "teal.model.prepare",
        "teal.model.sample" => "teal.model.sample",
        "teal.model.auth.login" => "teal.model.auth.login",
        "teal.model.auth.preflight" => "teal.model.auth.preflight",
        "teal.model.auth.catalog" => "teal.model.auth.catalog",
        "teal.model.auth.refresh" => "teal.model.auth.refresh",
        "teal.agent.model.prepare" => "teal.agent.model.prepare",
        "teal.agent.model.attempt" => "teal.agent.model.attempt",
        "teal.agent.tool" => "teal.agent.tool",
        "teal.cli.action" => "teal.cli.action",
        "teal.browser.start" => "teal.browser.start",
        "teal.browser.open" => "teal.browser.open",
        "teal.browser.click" => "teal.browser.click",
        "teal.browser.fill" => "teal.browser.fill",
        "teal.browser.close" => "teal.browser.close",
        "teal.computer.action" => "teal.computer.action",
        "teal.workspace.mutate" => "teal.workspace.mutate",
        "teal.automation.execute" => "teal.automation.execute",
        "teal.config.commit" => "teal.config.commit",
        "teal.knowledge.write" => "teal.knowledge.write",
        "teal.knowledge.export" => "teal.knowledge.export",
        "teal.host.file.mutate" => "teal.host.file.mutate",
        "teal.process.run" => "teal.process.run",
        "teal.bench.agent.run" => "teal.bench.agent.run",
        "teal.bench.agent.tool" => "teal.bench.agent.tool",
        "teal.bench.agent.provider" => "teal.bench.agent.provider",
        "teal.bench.browser" => "teal.bench.browser",
        "teal.bench.task" => "teal.bench.task",
        "teal.compile" => "teal.compile",
        "teal.load" => "teal.load",
        "teal.execute" => "teal.execute",
        "teal.run" => "teal.run",
        "teal.function" => "teal.function",
        "teal.host.call" => "teal.host.call",
        "teal.dylib.call" => "teal.dylib.call",
        "teal.gc.collect" => "teal.gc.collect",
        _ => "operation",
    }
}

pub(crate) fn application(target: &str) -> bool {
    target == "teal-bench"
        || target.starts_with("teal_bench::")
        || target.starts_with("teal_bench_")
        || target == "teal"
        || target.starts_with("teal::")
        || target.starts_with("teal_")
}

pub(crate) fn allowed(key: &str) -> bool {
    matches!(
        key,
        "file.path"
            | "teal.source.name"
            | "operation.result"
            | "event.name"
            | "error.type"
            | "operation.id"
            | "audit.sequence"
            | "request.id"
            | "buffer.revision"
            | "queue.depth"
            | "duration_ms"
            | "teal.exit_code"
            | "teal.instructions"
            | "teal.gc.reclaimed"
            | "teal.frame.id"
            | "model.protocol"
            | "model.phase"
            | "model.failure_stage"
            | "model.assembly_error"
            | "model.stop_reason"
            | "model.models_count"
            | "model.request_bytes"
            | "model.response_bytes"
            | "model.sse_events"
            | "model.events"
            | "model.pending_tools"
            | "http.status_code"
    )
}
