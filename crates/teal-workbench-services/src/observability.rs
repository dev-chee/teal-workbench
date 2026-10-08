//! 进程拥有的观测：有界本地日志、显式启用的 OTLP 与明确关闭流程。

mod config;
#[cfg(test)]
mod tests;
mod export;
mod privacy;
mod records;

pub use config::{Arguments, LogFormat, Options};
use opentelemetry::{
    logs::LoggerProvider as _,
    metrics::{Meter, MeterProvider as _},
    trace::TracerProvider as _,
};
use opentelemetry_otlp::{Protocol, WithExportConfig as _, WithHttpConfig as _};
use opentelemetry_sdk::{
    Resource,
    logs::SdkLoggerProvider,
    metrics::{PeriodicReader, SdkMeterProvider, Stream},
    trace::SdkTracerProvider,
};
use std::{
    io::{self, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tracing_appender::{
    non_blocking::{NonBlockingBuilder, WorkerGuard},
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{
    filter::{FilterExt as _, filter_fn},
    prelude::*,
};

type Shutdown = Box<dyn FnOnce() -> opentelemetry_sdk::error::OTelSdkResult + Send>;

/// 仅由 main 拥有。库中的指标表和日志器副本不拥有此生命周期。
pub struct Guard {
    traces: Option<SdkTracerProvider>,
    logs: Option<SdkLoggerProvider>,
    metrics: Option<SdkMeterProvider>,
    worker: Option<WorkerGuard>,
    /// 是否为 Teal 函数调用生成跨度；缺省只导出应用自身的跨度。
    pub function_spans: bool,
    health: Arc<Health>,
    dropped: tracing_appender::non_blocking::ErrorCounter,
}

/// 观测写入与导出的累计失败计数，供调用方提示观测降级。
#[derive(Default)]
pub struct Health {
    /// 本地日志写入失败次数。
    pub write_failures: AtomicU64,
    /// OTLP 导出失败次数。
    pub export_failures: AtomicU64,
}

struct CheckedWriter<W> {
    inner: W,
    health: Arc<Health>,
}

impl<W: Write> Write for CheckedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let result = self.inner.write(bytes);
        if result.is_err() {
            self.health.write_failures.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    fn flush(&mut self) -> io::Result<()> {
        let result = self.inner.flush();
        if result.is_err() {
            self.health.write_failures.fetch_add(1, Ordering::Relaxed);
        }
        result
    }
}

impl Guard {
    /// 构建观测资源并安装为进程的全局 tracing 订阅者。
    ///
    /// # Errors
    /// 已存在全局订阅者或本地日志目录不可用时返回错误，此时不安装任何全局状态。
    pub fn initialize(options: Options) -> io::Result<Self> {
        let (guard, subscriber) = Self::build(options)?;
        tracing::subscriber::set_global_default(subscriber)
            .map_err(|_| io::Error::other("tracing subscriber is already installed"))?;
        Ok(guard)
    }

    /// 构建观测资源，并把订阅者交回调用方自行安装。
    ///
    /// 供需要组合多个订阅者或验证导出行为的调用方使用；返回的 `Guard` 持有观测
    /// 资源，丢弃后本地日志工作线程随之关闭。
    ///
    /// # Errors
    /// 本地日志目录不可用时返回错误。
    pub fn build(options: Options) -> io::Result<(Self, impl tracing::Subscriber + Send + Sync)> {
        let file = RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix("teal.bench.log")
            .build(&options.directory)
            .map_err(|_| io::Error::other("Unable to create Teal Bench rolling log file"))?;
        let health = Arc::new(Health::default());
        let (writer, worker) = NonBlockingBuilder::default()
            .buffered_lines_limit(8192)
            .lossy(true)
            .finish(CheckedWriter {
                inner: file,
                health: health.clone(),
            });
        let dropped = writer.error_counter();

        // 空资源构建器不会隐式导入任意环境属性。
        let resource = Resource::builder_empty()
            .with_attributes(options.resource)
            .build();
        let mut traces = None;
        let mut logs = None;
        let mut metrics = None;
        let mut warnings = Vec::new();

        // 即使禁用跨度导出，日志关联仍需追踪提供器。
        if options.traces.is_some() || options.logs.is_some() {
            let mut builder = SdkTracerProvider::builder().with_resource(resource.clone());
            if let Some(signal) = options.traces {
                let exporter = blocking_client(signal.timeout).and_then(|client| {
                    opentelemetry_otlp::SpanExporter::builder()
                        .with_http()
                        .with_http_client(client)
                        .with_protocol(Protocol::HttpBinary)
                        .with_endpoint(signal.endpoint)
                        .with_timeout(signal.timeout)
                        .with_headers(signal.headers)
                        .build()
                        .map_err(|_| io::Error::other("Span exporter initialization failed"))
                });
                match exporter {
                    Ok(inner) => {
                        builder = builder.with_batch_exporter(export::Sanitized {
                            inner,
                            policy: options.policy.clone(),
                        });
                    }
                    Err(_) => warnings.push("traces"),
                }
            }
            traces = Some(builder.build());
        }

        if let Some(signal) = options.logs {
            let exporter = blocking_client(signal.timeout).and_then(|client| {
                opentelemetry_otlp::LogExporter::builder()
                    .with_http()
                    .with_http_client(client)
                    .with_protocol(Protocol::HttpBinary)
                    .with_endpoint(signal.endpoint)
                    .with_timeout(signal.timeout)
                    .with_headers(signal.headers)
                    .build()
                    .map_err(|_| io::Error::other("Log exporter initialization failed"))
            });
            match exporter {
                Ok(exporter) => {
                    logs = Some(
                        SdkLoggerProvider::builder()
                            .with_resource(resource.clone())
                            .with_batch_exporter(exporter)
                            .build(),
                    );
                }
                Err(_) => warnings.push("logs"),
            }
        }

        if let Some(signal) = options.metrics {
            let exporter = blocking_client(signal.timeout).and_then(|client| {
                opentelemetry_otlp::MetricExporter::builder()
                    .with_http()
                    .with_http_client(client)
                    .with_protocol(Protocol::HttpBinary)
                    .with_endpoint(signal.endpoint)
                    .with_timeout(signal.timeout)
                    .with_headers(signal.headers)
                    .build()
                    .map_err(|_| io::Error::other("Metric exporter initialization failed"))
            });
            match exporter {
                Ok(exporter) => {
                    metrics = Some(
                        SdkMeterProvider::builder()
                            .with_resource(resource)
                            .with_reader(
                                PeriodicReader::builder(exporter)
                                    .with_interval(options.interval)
                                    .build(),
                            )
                            .with_view(|_| {
                                Stream::builder().with_cardinality_limit(64).build().ok()
                            })
                            .build(),
                    );
                }
                Err(_) => warnings.push("metrics"),
            }
        }

        health
            .export_failures
            .store(warnings.len() as u64, Ordering::Relaxed);
        let meter = metrics
            .as_ref()
            .map(|provider| provider.meter("teal-bench"));
        if let Some(meter) = &meter {
            let health = health.clone();
            let dropped = dropped.clone();
            meter
                .u64_observable_counter("teal.bench.log.dropped")
                .with_callback(move |observer| {
                    observer.observe(dropped.dropped_lines() as u64, &[]);
                })
                .build();
            meter
                .u64_observable_counter("teal.bench.log.write_failures")
                .with_callback(move |observer| {
                    observer.observe(health.write_failures.load(Ordering::Relaxed), &[]);
                })
                .build();
        }

        let trace_layer = traces.as_ref().map(|provider| {
            tracing_opentelemetry::layer()
                .with_tracer(provider.tracer("teal-bench"))
                .with_filter(options.trace_filter.and(filter_fn(|metadata| {
                    privacy::application(metadata.target())
                })))
        });
        let records = records::Records {
            writer,
            format: options.format,
            policy: options.policy,
            local_filter: options.local_filter,
            log_filter: options.log_filter,
            logger: logs.as_ref().map(|provider| provider.logger("teal-bench")),
            metrics: meter.as_ref().map(records::OperationMetrics::new),
            last_diagnostic: Arc::new(Mutex::new(None)),
            health: health.clone(),
        };
        let subscriber = tracing_subscriber::registry()
            .with(trace_layer)
            .with(records);
        let guard = Self {
            traces,
            logs,
            metrics,
            worker: Some(worker),
            function_spans: options.function_spans,
            health,
            dropped,
        };

        Ok((guard, subscriber))
    }

    /// 返回 Teal 指标表；未启用指标导出时为 `None`。
    pub fn meter(&self) -> Option<Meter> {
        self.metrics.as_ref().map(|provider| provider.meter("teal"))
    }

    /// 返回可跨线程读取的观测失败计数。
    pub fn health(&self) -> Arc<Health> {
        self.health.clone()
    }

    /// 以五秒预算关闭导出器与日志工作线程，超时后不再等待剩余任务。
    pub fn shutdown(mut self) {
        self.close(Duration::from_secs(5));
    }

    fn close(&mut self, budget: Duration) {
        if self.worker.is_none() {
            return;
        }

        let deadline = Instant::now() + budget;

        // 某些 SDK 读取器忽略 shutdown_with_timeout。不要在应用线程等待这些任务；
        // 先启动全部关闭操作，再用统一的外部期限约束它们。
        let mut jobs: Vec<Shutdown> = Vec::new();
        if let Some(provider) = self.traces.take() {
            jobs.push(Box::new(move || {
                provider.shutdown_with_timeout(deadline.saturating_duration_since(Instant::now()))
            }));
        }
        if let Some(provider) = self.metrics.take() {
            jobs.push(Box::new(move || {
                provider.shutdown_with_timeout(deadline.saturating_duration_since(Instant::now()))
            }));
        }
        if let Some(provider) = self.logs.take() {
            jobs.push(Box::new(move || {
                provider.shutdown_with_timeout(deadline.saturating_duration_since(Instant::now()))
            }));
        }

        let (sender, receiver) = std::sync::mpsc::channel();
        let mut remaining = jobs.len();
        for job in jobs {
            let sender = sender.clone();
            if std::thread::Builder::new()
                .name("teal-bench-otel-shutdown".into())
                .spawn(move || {
                    let _ = sender.send(job().is_ok());
                })
                .is_err()
            {
                remaining -= 1;
                self.health.export_failures.fetch_add(1, Ordering::Relaxed);
            }
        }
        drop(sender);

        while remaining > 0 {
            match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(success) => {
                    remaining -= 1;
                    if !success {
                        self.health.export_failures.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(_) => break,
            }
        }

        self.health
            .export_failures
            .fetch_add(remaining as u64, Ordering::Relaxed);
        // 仅在终端恢复后调用，包括栈展开期间。
        let writes = self.health.write_failures.load(Ordering::Relaxed);
        let exports = self.health.export_failures.load(Ordering::Relaxed);
        let dropped = self.dropped.dropped_lines();
        if writes > 0 || exports > 0 || dropped > 0 {
            eprintln!(
                "Teal Bench observations: {writes} log write failures, {exports} export or initialization failures, and {dropped} log records dropped"
            );
        }
        self.worker.take();
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.close(Duration::from_secs(5));
    }
}

fn blocking_client(timeout: Duration) -> io::Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|_| io::Error::other("OTLP HTTP client initialization failed"))
}

/// 只记录有限的结果类别，绝不格式化错误载荷。
pub fn outcome<T, E>(result: &Result<T, E>) {
    let span = tracing::Span::current();
    span.record(
        "operation.result",
        if result.is_ok() { "ok" } else { "error" },
    );
    if result.is_err() {
        span.record("otel.status_code", "ERROR");
        tracing::warn!(event.name = "failed");
    } else {
        tracing::info!(event.name = "completed");
    }
}

/// 返回进程内单调递增的运算标识，用于把跨度关联到同一次后台操作。
pub fn operation_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
