//! 从 tracing 跨度生成脱敏本地与 OTLP 记录及有界操作指标。

use super::{
    config::LogFormat,
    privacy::{Policy, application, operation},
};
use opentelemetry::{
    Value,
    logs::{LogRecord as _, Logger as _, Severity},
    metrics::{Counter, Histogram, Meter, UpDownCounter},
    trace::TraceContextExt as _,
};
use opentelemetry_sdk::logs::SdkLogger;
use std::{
    collections::BTreeMap,
    fmt,
    io::Write,
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::{EnvFilter, Layer, layer::Context, registry::LookupSpan};

#[derive(Default)]
struct Fields(BTreeMap<String, Value>);

impl Visit for Fields {
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().into(), value.into());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if let Ok(value) = i64::try_from(value) {
            self.record_i64(field, value);
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().into(), value.into());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if value.is_finite() {
            self.0.insert(field.name().into(), value.into());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if super::privacy::allowed(field.name()) {
            self.0.insert(field.name().into(), value.to_owned().into());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        // 只格式化具有明确策略的字段；绝不格式化正文或载荷。
        if matches!(
            field.name(),
            "file.path" | "teal.source.name" | "error.type"
        ) {
            self.record_str(field, &format!("{value:?}"));
        }
    }
}

struct SpanRecord {
    fields: BTreeMap<String, Value>,
    started: Instant,
    operation: &'static str,
}

pub(crate) struct Records {
    pub writer: tracing_appender::non_blocking::NonBlocking,
    pub format: LogFormat,
    pub policy: Policy,
    pub local_filter: EnvFilter,
    pub log_filter: EnvFilter,
    pub logger: Option<SdkLogger>,
    pub metrics: Option<OperationMetrics>,
    pub last_diagnostic: Arc<Mutex<Option<Instant>>>,
    pub health: Arc<super::Health>,
}

pub(crate) struct OperationMetrics {
    count: Counter<u64>,
    duration: Histogram<f64>,
    active: UpDownCounter<i64>,
}

impl OperationMetrics {
    pub fn new(meter: &Meter) -> Self {
        Self {
            count: meter.u64_counter("teal.bench.operation.count").build(),
            duration: meter
                .f64_histogram("teal.bench.operation.duration")
                .with_unit("ms")
                .build(),
            active: meter
                .i64_up_down_counter("teal.bench.operation.active")
                .build(),
        }
    }
}

impl Records {
    fn safe(&self, fields: Fields) -> BTreeMap<String, Value> {
        fields
            .0
            .into_iter()
            .filter_map(|(k, v)| self.policy.field(&k, &v))
            .map(|kv| (kv.key.as_str().to_owned(), kv.value))
            .collect()
    }
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Records {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        let operation = operation(attrs.metadata().name());
        span.extensions_mut().insert(SpanRecord {
            fields: self.safe(fields),
            started: Instant::now(),
            operation,
        });
        if operation.starts_with("teal.bench.")
            && let Some(metrics) = &self.metrics
        {
            metrics
                .active
                .add(1, &[opentelemetry::KeyValue::new("operation", operation)]);
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut fields = Fields::default();
        values.record(&mut fields);
        if let Some(record) = span.extensions_mut().get_mut::<SpanRecord>() {
            record.fields.extend(self.safe(fields));
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let meta = event.metadata();
        if meta.target().starts_with("opentelemetry") && *meta.level() <= tracing::Level::WARN {
            self.health
                .export_failures
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let local = self.local_filter.enabled(meta, ctx.clone());
        let remote = self.logger.is_some()
            && application(meta.target())
            && self.log_filter.enabled(meta, ctx.clone());
        if !local && !remote {
            return;
        }
        if !application(meta.target()) {
            let Ok(mut last) = self.last_diagnostic.lock() else {
                return;
            };
            if last.is_some_and(|time| time.elapsed() < std::time::Duration::from_secs(30)) {
                return;
            }
            *last = Some(Instant::now());
        }
        let mut fields = BTreeMap::new();
        let mut name = "operation";
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                if let Some(record) = span.extensions().get::<SpanRecord>() {
                    fields.extend(record.fields.clone());
                    name = record.operation;
                }
            }
        }
        let mut event_fields = Fields::default();
        event.record(&mut event_fields);
        fields.extend(self.safe(event_fields));
        let otel = tracing::Span::current().context();
        let span = otel.span();
        let context = span.span_context();
        let now = SystemTime::now();
        let timestamp = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let trace_id = context.is_valid().then(|| context.trace_id().to_string());
        let span_id = context.is_valid().then(|| context.span_id().to_string());
        let safe_json: BTreeMap<_, _> = fields
            .iter()
            .map(|(key, value)| (key, json_value(value)))
            .collect();
        if local {
            let value = serde_json::json!({"timestamp_ms":timestamp,"level":meta.level().as_str(),"target":meta.target(),"operation":name,"trace_id":trace_id,"span_id":span_id,"fields":safe_json});
            let line = match self.format {
                LogFormat::Json => format!("{value}\n"),
                LogFormat::Text => format!(
                    "{timestamp} {} {} {name} trace={} span={} {}\n",
                    meta.level(),
                    meta.target(),
                    trace_id.as_deref().unwrap_or("-"),
                    span_id.as_deref().unwrap_or("-"),
                    value["fields"]
                ),
            };
            let _ = self.writer.clone().write_all(line.as_bytes());
        }
        if remote && let Some(logger) = &self.logger {
            let mut record = logger.create_log_record();
            record.set_timestamp(now);
            record.set_target(meta.target());
            record.set_body(name.into());
            record.set_severity_text(meta.level().as_str());
            record.set_severity_number(match *meta.level() {
                tracing::Level::ERROR => Severity::Error,
                tracing::Level::WARN => Severity::Warn,
                tracing::Level::INFO => Severity::Info,
                tracing::Level::DEBUG => Severity::Debug,
                tracing::Level::TRACE => Severity::Trace,
            });
            if context.is_valid() {
                record.set_trace_context(
                    context.trace_id(),
                    context.span_id(),
                    Some(context.trace_flags()),
                );
            }
            for (key, value) in fields {
                record.add_attribute(key, any_value(value));
            }
            logger.emit(record);
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else { return };
        let ext = span.extensions();
        let Some(record) = ext.get::<SpanRecord>() else {
            return;
        };
        if !record.operation.starts_with("teal.bench.") {
            return;
        }
        if let Some(metrics) = &self.metrics {
            let result = record
                .fields
                .get("operation.result")
                .map_or("cancelled", |v| match v {
                    Value::String(v) => v.as_str(),
                    _ => "cancelled",
                });
            let attributes = [
                opentelemetry::KeyValue::new("operation", record.operation),
                opentelemetry::KeyValue::new("result", result.to_owned()),
            ];
            metrics.count.add(1, &attributes);
            metrics
                .duration
                .record(record.started.elapsed().as_secs_f64() * 1000.0, &attributes);
            metrics.active.add(
                -1,
                &[opentelemetry::KeyValue::new("operation", record.operation)],
            );
        }
    }
}

fn json_value(value: &Value) -> serde_json::Value {
    match value {
        Value::Bool(v) => (*v).into(),
        Value::I64(v) => (*v).into(),
        Value::F64(v) => serde_json::json!(v),
        Value::String(v) => v.as_str().into(),
        _ => serde_json::Value::Null,
    }
}

fn any_value(value: Value) -> opentelemetry::logs::AnyValue {
    match value {
        Value::Bool(v) => v.into(),
        Value::I64(v) => v.into(),
        Value::F64(v) => v.into(),
        Value::String(v) => v.into(),
        _ => "[redacted]".into(),
    }
}
