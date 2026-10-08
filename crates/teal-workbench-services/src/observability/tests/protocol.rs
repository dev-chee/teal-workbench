//! 真实 OTLP HTTP 载荷、队列上下文传播及禁用信号隔离测试。
// 测试环境保持 NoSubscriber 已登记，避免其他未观测的测试线程
// 通过 tracing 单分发的线程局部快捷路径初始化共享调用点。

use super::*;
#[allow(unused_imports)]
use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
    trace::v1::ExportTraceServiceRequest,
};
#[allow(unused_imports)]
use prost::Message as _;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{atomic::AtomicBool, mpsc},
    thread,
};

struct Capture {
    endpoint: String,
    requests: mpsc::Receiver<(String, Vec<u8>)>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Capture {
    fn new(status: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (tx, requests) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let ended = stop.clone();
        let thread = thread::spawn(move || {
            while !ended.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => break,
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut data = Vec::new();
                let mut chunk = [0; 4096];
                let (header_end, size) = loop {
                    let size = stream.read(&mut chunk).unwrap();
                    assert!(size > 0);
                    data.extend_from_slice(&chunk[..size]);
                    if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&data[..end]);
                        let length = header
                            .lines()
                            .filter_map(|line| line.split_once(':'))
                            .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .unwrap()
                            .1
                            .trim()
                            .parse::<usize>()
                            .unwrap();
                        break (end + 4, length);
                    }
                };
                while data.len() < header_end + size {
                    let size = stream.read(&mut chunk).unwrap();
                    assert!(size > 0);
                    data.extend_from_slice(&chunk[..size]);
                }
                let path = String::from_utf8_lossy(&data[..header_end])
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned();
                if status == 0 {
                    tx.send((path, data[header_end..header_end + size].to_vec()))
                        .unwrap();
                    while !ended.load(Ordering::Acquire) {
                        thread::sleep(Duration::from_millis(2));
                    }
                    continue;
                }
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {status} OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                );
                tx.send((path, data[header_end..header_end + size].to_vec()))
                    .unwrap();
            }
        });
        Self {
            endpoint,
            requests,
            stop,
            thread: Some(thread),
        }
    }
    fn finish(mut self) -> Vec<(String, Vec<u8>)> {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
        self.requests.try_iter().collect()
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[test]
fn each_signal_is_independent_and_defaults_never_contact_collector() {
    let _unobserved_threads = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    for enabled in [None, Some("logs"), Some("traces"), Some("metrics")] {
        let server = Capture::new(200);
        let dir = tempfile::tempdir().unwrap();
        let (guard, subscriber) = Guard::build(options(
            Arguments {
                otel_logs: enabled == Some("logs"),
                otel_traces: enabled == Some("traces"),
                otel_metrics: enabled == Some("metrics"),
                otel_endpoint: Some(server.endpoint.clone()),
                ..Default::default()
            },
            dir.path(),
            &[],
        ))
        .unwrap();
        tracing::subscriber::with_default(subscriber, || {
            tracing::callsite::rebuild_interest_cache();
            tracing::info_span!("teal.bench.script", operation.result = "ok")
                .in_scope(|| tracing::info!(event.name = "completed"));
        });
        guard.shutdown();
        let requests = server.finish();
        match enabled {
            None => assert!(requests.is_empty()),
            Some(signal) => {
                assert!(!requests.is_empty());
                assert!(requests.iter().all(|(p, _)| p == &format!("/v1/{signal}")));
            }
        }
    }
}

#[test]
fn collector_failure_does_not_fail_work_or_exceed_shutdown_budget() {
    let _unobserved_threads = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    let server = Capture::new(400);
    let dir = tempfile::tempdir().unwrap();
    let (guard, subscriber) = Guard::build(options(
        Arguments {
            otel_traces: true,
            otel_logs: true,
            otel_metrics: true,
            otel_endpoint: Some(server.endpoint.clone()),
            ..Default::default()
        },
        dir.path(),
        &[],
    ))
    .unwrap();
    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        tracing::info_span!("teal.bench.script", operation.result = "ok")
            .in_scope(|| tracing::info!(event.name = "completed"));
    });
    let started = Instant::now();
    guard.shutdown();
    assert!(started.elapsed() < Duration::from_secs(6));
    assert!(!server.finish().is_empty());
}

#[test]
fn stalled_collector_has_one_shared_shutdown_deadline_for_all_signals() {
    let _unobserved_threads = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    let server = Capture::new(0);
    let dir = tempfile::tempdir().unwrap();
    let (guard, subscriber) = Guard::build(options(
        Arguments {
            otel_traces: true,
            otel_logs: true,
            otel_metrics: true,
            otel_endpoint: Some(server.endpoint.clone()),
            ..Default::default()
        },
        dir.path(),
        &[("OTEL_EXPORTER_OTLP_TIMEOUT", "20000")],
    ))
    .unwrap();
    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        tracing::info_span!("teal.bench.script", operation.result = "ok")
            .in_scope(|| tracing::info!(event.name = "completed"));
    });
    let started = Instant::now();
    guard.shutdown();
    assert!(
        started.elapsed() < Duration::from_millis(5800),
        "shutdown elapsed {:?}",
        started.elapsed()
    );
    assert!(!server.finish().is_empty());
}
