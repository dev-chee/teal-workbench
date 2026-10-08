//! 公开启动与输出边界的回归测试。

mod protocol;

use super::*;
use std::collections::HashMap;

fn options(args: Arguments, directory: &std::path::Path, vars: &[(&str, &str)]) -> Options {
    let values: HashMap<_, _> = vars.iter().copied().collect();
    Options::resolve(
        args,
        directory.to_path_buf(),
        directory.join("logs"),
        |name| values.get(name).map(|v| (*v).to_owned()),
    )
    .unwrap()
}

#[test]
fn explicit_flags_are_required_and_invalid_inputs_do_not_echo_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let config = options(
        Arguments::default(),
        dir.path(),
        &[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:1"),
            ("OTEL_TRACES_EXPORTER", "otlp"),
        ],
    );
    assert!(config.logs.is_none() && config.traces.is_none() && config.metrics.is_none());
    for args in [
        Arguments {
            otel_traces: true,
            otel_endpoint: Some("file:///secret-token".into()),
            ..Default::default()
        },
        Arguments {
            log_filter: Some("[secret-token".into()),
            ..Default::default()
        },
    ] {
        let error = Options::resolve(args, dir.path().into(), dir.path().into(), |_| None)
            .err()
            .unwrap();
        assert!(!error.to_string().contains("secret-token"));
    }
    let config = options(
        Arguments {
            otel_logs: true,
            ..Default::default()
        },
        dir.path(),
        &[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://localhost:4318"),
            (
                "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
                "http://localhost:4319/custom",
            ),
            (
                "OTEL_EXPORTER_OTLP_HEADERS",
                "authorization=Bearer%20secret-token",
            ),
        ],
    );
    let logs = config.logs.unwrap();
    assert_eq!(logs.endpoint, "http://localhost:4319/custom");
    assert_eq!(logs.headers["authorization"], "Bearer secret-token");
    assert_eq!(config.policy.text("secret-token"), "[redacted]");
}

#[test]
fn privacy_is_shared_and_fails_closed_for_payloads_paths_and_status() {
    let directory = tempfile::tempdir().unwrap();
    let policy = privacy::Policy::new(
        directory.path().join("workspace"),
        vec!["private-token".into()],
    );
    assert_eq!(
        policy.path(
            &directory
                .path()
                .join("workspace/src/main.rs")
                .to_string_lossy()
        ),
        "src/main.rs"
    );
    assert_eq!(
        policy.path(
            &directory
                .path()
                .join("outside/secret.txt")
                .to_string_lossy()
        ),
        "<external>"
    );
    assert_eq!(policy.path("../secret.txt"), "<external>");
    for key in [
        "message",
        "prompt",
        "arguments",
        "result",
        "otel.status_description",
        "teal.function.name",
        "authorization",
    ] {
        assert!(
            policy
                .field(key, &opentelemetry::Value::from("private-token"))
                .is_none()
        );
    }
    assert_eq!(
        policy.field("audit.sequence", &opentelemetry::Value::from(7_i64)),
        Some(opentelemetry::KeyValue::new("audit.sequence", 7_i64))
    );
    assert!(
        policy
            .field(
                "audit.sequence",
                &opentelemetry::Value::from("private-token")
            )
            .is_none()
    );
    assert_eq!(policy.path("private-token.tl"), "[redacted].tl");
    assert_eq!(
        policy.field(
            "model.failure_stage",
            &opentelemetry::Value::from("sse_frame")
        ),
        Some(opentelemetry::KeyValue::new(
            "model.failure_stage",
            "sse_frame"
        ))
    );
    assert!(
        policy
            .field(
                "model.failure_stage",
                &opentelemetry::Value::from("private-token")
            )
            .is_none()
    );
    assert!(
        policy
            .field(
                "model.request_bytes",
                &opentelemetry::Value::from("private-token")
            )
            .is_none()
    );
}

#[cfg(windows)]
#[test]
fn privacy_normalizes_windows_verbatim_and_unc_prefixes() {
    let policy = privacy::Policy::new(r"\\?\C:\workspace".into(), vec![]);
    assert_eq!(
        policy.path("C:\\workspace\\\u{76ee}\u{5f55}\\file.rs"),
        "\u{76ee}\u{5f55}/file.rs"
    );
    assert_eq!(policy.path(r"\\?\C:\workspace\file.rs"), "file.rs");
    let unc = privacy::Policy::new(r"\\?\UNC\server\share\workspace".into(), vec![]);
    assert_eq!(unc.path(r"\\server\share\workspace\file.rs"), "file.rs");
    assert_eq!(unc.path(r"\\server\other\file.rs"), "<external>");
}

#[test]
fn saturated_and_failing_log_writers_never_block_application_producers() {
    struct Failed;
    impl Write for Failed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("Disk not available"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("Disk not available"))
        }
    }
    struct Slow(Arc<std::sync::atomic::AtomicBool>);
    impl Write for Slow {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            while !self.0.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (mut writer, guard) = NonBlockingBuilder::default()
        .buffered_lines_limit(1)
        .lossy(true)
        .finish(Slow(released.clone()));
    let started = Instant::now();
    for _ in 0..100 {
        writer.write_all(b"record\n").unwrap();
    }
    let dropped = writer.error_counter().dropped_lines();
    released.store(true, Ordering::Release);
    drop(guard);
    assert!(dropped > 0);
    assert!(started.elapsed() < Duration::from_secs(1));
    let health = Arc::new(Health::default());
    let mut writer = CheckedWriter {
        inner: Failed,
        health: health.clone(),
    };
    assert!(writer.write_all(b"safe").is_err());
    assert_eq!(health.write_failures.load(Ordering::Relaxed), 1);
}

#[test]
fn sanitized_in_memory_export_keeps_ids_but_removes_error_content() {
    use opentelemetry::trace::{Span as _, Tracer as _, TracerProvider as _};
    let inner = opentelemetry_sdk::trace::InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(export::Sanitized {
            inner: inner.clone(),
            policy: privacy::Policy::new("C:/workspace".into(), vec![]),
        })
        .build();
    let tracer = provider.tracer("teal-bench");
    let mut span = tracer.start("teal.bench.script");
    span.set_attribute(opentelemetry::KeyValue::new("prompt", "never-export-this"));
    span.set_status(opentelemetry::trace::Status::error("never-export-this"));
    span.add_event(
        "never-export-this",
        vec![opentelemetry::KeyValue::new(
            "arguments",
            "never-export-this",
        )],
    );
    span.end();
    let spans = inner.get_finished_spans().unwrap();
    assert_eq!(spans.len(), 1);
    assert!(spans[0].span_context.is_valid());
    assert!(!format!("{:?}", spans[0]).contains("never-export-this"));
    provider.shutdown().unwrap();
}
