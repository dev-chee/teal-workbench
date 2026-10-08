//! 仅用于启动配置。凭据不实现 Debug，也不参与序列化。

use std::{collections::HashMap, io, path::PathBuf, time::Duration};
use tracing_subscriber::EnvFilter;

/// 本地日志文件的编码格式。
#[derive(Clone, Copy, Debug, Default)]
pub enum LogFormat {
    /// 人类可读的文本行。
    #[default]
    Text,
    /// 每行一条 JSON 对象，便于机器解析。
    Json,
}

/// 由命令行开关装配的观测输入；全部缺省时只写本地滚动日志。
#[derive(Debug, Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent explicit enablement of command line switches, not states of the same state machine"
)]
pub struct Arguments {
    /// 本地日志编码；绝不将应用日志写入终端。
    pub log_format: LogFormat,

    /// 覆盖本地滚动日志目录。
    pub log_dir: Option<PathBuf>,

    /// 本地目标与级别过滤器；未设置时读取 `RUST_LOG`。
    pub log_filter: Option<String>,

    /// 显式启用 OTLP 日志导出。
    pub otel_logs: bool,

    /// 显式启用 OTLP 跨度导出。
    pub otel_traces: bool,

    /// 显式启用 OTLP 指标导出。
    pub otel_metrics: bool,

    /// 通用 OTLP/HTTP 基础网址；设置此项不会启用导出。
    pub otel_endpoint: Option<String>,

    /// 独立的 OTLP 跨度目标与级别过滤器。
    pub otel_trace_filter: Option<String>,

    /// 独立的 OTLP 日志目标与级别过滤器。
    pub otel_log_filter: Option<String>,

    /// 包含 Teal 函数跨度；需要 --otel-traces。
    pub trace_functions: bool,
}

pub(crate) struct Signal {
    pub endpoint: String,
    pub headers: HashMap<String, String>,
    pub timeout: Duration,
}

/// 已解析的观测配置：输出目录、过滤器、导出信号和脱敏策略。
pub struct Options {
    pub(crate) directory: PathBuf,
    pub(crate) format: LogFormat,
    pub(crate) local_filter: EnvFilter,
    pub(crate) trace_filter: EnvFilter,
    pub(crate) log_filter: EnvFilter,
    pub(crate) traces: Option<Signal>,
    pub(crate) logs: Option<Signal>,
    pub(crate) metrics: Option<Signal>,
    pub(crate) interval: Duration,
    pub(crate) resource: Vec<opentelemetry::KeyValue>,
    pub(crate) policy: super::privacy::Policy,
    pub(crate) function_spans: bool,
}

impl Options {
    /// 仅解析进程启动输入，绝不读取用户或工作区的 Teal 状态。
    pub fn resolve(
        args: Arguments,
        workspace: PathBuf,
        directory: PathBuf,
        env: impl Fn(&str) -> Option<String>,
    ) -> io::Result<Self> {
        let mut secrets = ["OPENAI_API_KEY", "ANTHROPIC_API_KEY"]
            .into_iter()
            .filter_map(&env)
            .collect::<Vec<_>>();
        let traces = signal(
            "TRACES",
            args.otel_traces,
            args.otel_endpoint.as_deref(),
            &env,
        )?;
        let logs = signal("LOGS", args.otel_logs, args.otel_endpoint.as_deref(), &env)?;
        let metrics = signal(
            "METRICS",
            args.otel_metrics,
            args.otel_endpoint.as_deref(),
            &env,
        )?;
        for signal in [&traces, &logs, &metrics].into_iter().flatten() {
            secrets.extend(signal.headers.values().cloned());
        }
        // 对应信号未启用时，仍须脱敏响应头中的凭据。
        for name in [
            "OTEL_EXPORTER_OTLP_HEADERS",
            "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
            "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
            "OTEL_EXPORTER_OTLP_METRICS_HEADERS",
        ] {
            if let Some(value) = env(name) {
                secrets.extend(pairs(&value)?.into_iter().map(|(_, value)| value));
            }
        }
        let policy = super::privacy::Policy::new(workspace, secrets);
        let mut resource = vec![
            opentelemetry::KeyValue::new(
                "service.name",
                policy.text(&env("OTEL_SERVICE_NAME").unwrap_or_else(|| "teal-bench".into())),
            ),
            opentelemetry::KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
        ];
        if let Some(value) = env("OTEL_RESOURCE_ATTRIBUTES") {
            for (key, value) in pairs(&value)? {
                if matches!(
                    key.as_str(),
                    "service.namespace" | "deployment.environment.name"
                ) {
                    resource.push(opentelemetry::KeyValue::new(key, policy.text(&value)));
                }
            }
        }
        Ok(Self {
            directory: args.log_dir.unwrap_or(directory),
            format: args.log_format,
            local_filter: filter(
                args.log_filter.or_else(|| env("RUST_LOG")),
                "warn,teal_bench=info,teal=info",
            )?,
            trace_filter: filter(
                args.otel_trace_filter
                    .or_else(|| env("TEAL_BENCH_OTEL_TRACE_FILTER")),
                if args.trace_functions {
                    "off,teal_bench=info,teal=trace"
                } else {
                    "off,teal_bench=info,teal=info"
                },
            )?,
            log_filter: filter(
                args.otel_log_filter
                    .or_else(|| env("TEAL_BENCH_OTEL_LOG_FILTER")),
                "off,teal_bench=info,teal=info",
            )?,
            interval: duration(env("OTEL_METRIC_EXPORT_INTERVAL"), 60_000)?,
            traces,
            logs,
            metrics,
            resource,
            policy,
            function_spans: args.trace_functions && args.otel_traces,
        })
    }
}

fn filter(value: Option<String>, default: &str) -> io::Result<EnvFilter> {
    let value = value.unwrap_or_else(|| default.to_owned());
    if value.contains(['[', ']']) {
        return Err(invalid(
            "Observation filters support target and level directives, but do not support span field predicates.",
        ));
    }
    EnvFilter::builder()
        .with_regex(false)
        .parse(value)
        .map_err(|_| invalid("Observation target and level filters are invalid"))
}

fn signal(
    name: &str,
    enabled: bool,
    explicit: Option<&str>,
    env: &impl Fn(&str) -> Option<String>,
) -> io::Result<Option<Signal>> {
    if !enabled {
        return Ok(None);
    }
    let specific = env(&format!("OTEL_EXPORTER_OTLP_{name}_ENDPOINT"));
    let endpoint = if let Some(base) = explicit {
        format!(
            "{}/v1/{}",
            base.trim_end_matches('/'),
            name.to_ascii_lowercase()
        )
    } else if let Some(specific) = specific {
        specific
    } else {
        format!(
            "{}/v1/{}",
            env("OTEL_EXPORTER_OTLP_ENDPOINT")
                .unwrap_or_else(|| "http://localhost:4318".into())
                .trim_end_matches('/'),
            name.to_ascii_lowercase()
        )
    };
    let url = url::Url::parse(&endpoint).map_err(|_| invalid("OTLP endpoint is invalid"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "OTLP requires HTTP(S) endpoint without credentials, query parameters or fragments",
        ));
    }
    let header_text = env(&format!("OTEL_EXPORTER_OTLP_{name}_HEADERS"))
        .or_else(|| env("OTEL_EXPORTER_OTLP_HEADERS"));
    let headers = pairs(header_text.as_deref().unwrap_or_default())?
        .into_iter()
        .collect::<HashMap<_, _>>();
    for (key, value) in &headers {
        reqwest::header::HeaderName::from_bytes(key.as_bytes())
            .map_err(|_| invalid("OTLP response header name is invalid"))?;
        reqwest::header::HeaderValue::from_str(value)
            .map_err(|_| invalid("OTLP response header value is invalid"))?;
    }
    let timeout = duration(
        env(&format!("OTEL_EXPORTER_OTLP_{name}_TIMEOUT"))
            .or_else(|| env("OTEL_EXPORTER_OTLP_TIMEOUT")),
        1_000,
    )?;
    Ok(Some(Signal {
        endpoint,
        headers,
        timeout,
    }))
}

fn duration(value: Option<String>, default: u64) -> io::Result<Duration> {
    let millis = value.map_or(Ok(default), |value| {
        value
            .parse::<u64>()
            .map_err(|_| invalid("Observation duration must be a positive number of milliseconds"))
    })?;
    if millis == 0 || millis > 3_600_000 {
        return Err(invalid(
            "Observation duration must be in the range 1..3600000 milliseconds",
        ));
    }
    Ok(Duration::from_millis(millis))
}

fn pairs(text: &str) -> io::Result<Vec<(String, String)>> {
    text.split(',')
        .filter(|part| !part.trim().is_empty())
        .map(|part| {
            let (key, value) = part
                .split_once('=')
                .ok_or_else(|| invalid("Observation attributes and response headers must use key=value key-value pairs"))?;
            Ok((decode(key.trim())?, decode(value.trim())?))
        })
        .collect()
}

fn decode(text: &str) -> io::Result<String> {
    let mut bytes = text.bytes();
    let mut output = Vec::new();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = bytes.next().and_then(|b| char::from(b).to_digit(16));
            let low = bytes.next().and_then(|b| char::from(b).to_digit(16));
            output.push(
                u8::try_from(high.zip(low).map(|(h, l)| h * 16 + l).ok_or_else(|| {
                    invalid("Invalid percent encoding in observation configuration")
                })?)
                .map_err(|_| invalid("Invalid percent encoding"))?,
            );
        } else {
            output.push(byte);
        }
    }
    String::from_utf8(output).map_err(|_| invalid("Observation configuration must be UTF-8"))
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
