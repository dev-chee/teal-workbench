use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::TcpListener,
    sync::{
        mpsc, Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
    trace::v1::ExportTraceServiceRequest,
};
use prost::Message as _;
use teal_workbench_services::observability::{Arguments, Guard, Options};

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
fn all_signals_correlate_real_teal_jobs_and_keep_concurrent_contexts_separate() {
    let _unobserved_threads = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    let server = Capture::new(200);
    let dir = tempfile::tempdir().unwrap();
    let config = options(
        Arguments {
            otel_logs: true,
            otel_traces: true,
            otel_metrics: true,
            otel_endpoint: Some(server.endpoint.clone()),
            log_format: teal_workbench_services::observability::LogFormat::Json,
            ..Default::default()
        },
        dir.path(),
        &[("OPENAI_API_KEY", "private-token")],
    );
    let (guard, subscriber) = Guard::build(config).unwrap();
    let meter = guard.meter();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let dispatch = tracing::Dispatch::new(subscriber);
    let script = dir.path().join("private-token.teal");
    std::fs::write(&script, "host \"std.bench.config\" as config { fn state_set(arg0: string, arg1: string); } fn main() { config::state_set(\"private-token\", \"private-token\"); }").unwrap();
    let state = dir.path().join("state.json");
    tracing::dispatcher::with_default(&dispatch, || {
        tracing::callsite::rebuild_interest_cache();
        runtime.block_on(async {
            let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
            let sender = crate::runtime_bridge::RuntimeSender::new(sender);
            let mut tasks = crate::runtime_bridge::TaskSupervisor::default();
            for id in 1..=2 {
                let script = script.clone();
                let state = state.with_file_name(format!("state-{id}.json"));
                let meter = meter.clone();
                let sender = sender.clone();
                let span = tracing::info_span!(
                    "teal.bench.script",
                    operation.id = id,
                    operation.result = tracing::field::Empty,
                    otel.status_code = tracing::field::Empty
                );
                let context = crate::runtime_bridge::Context::with_span(span);
                tasks.spawn(context.instrument(async move {
                    let result = crate::runtime_bridge::spawn_blocking(move || {
                        let mut teal =
                            teal_workbench_automation::WorkbenchAutomationRuntime::new(state)?;
                        teal.set_observation(meter, false);
                        let prepared = teal
                            .prepare(&script, teal_workbench_automation::ScriptKind::UserConfig)?;
                        teal.execute(prepared, "{}")
                    })
                    .await
                    .unwrap();
                    teal_workbench_services::observability::outcome(&result);
                    result.unwrap();
                    sender
                        .send(crate::runtime_bridge::RuntimeEvent::Message(
                            teal_workbench_core::Message::new(
                                teal_workbench_core::Severity::Info,
                                "test",
                                "completed",
                            ),
                        ))
                        .await
                        .unwrap();
                }));
            }
            for _ in 0..2 {
                let crate::runtime_bridge::RuntimeEvent::Observed { context, .. } =
                    tokio::time::timeout(Duration::from_secs(5), receiver.recv())
                        .await
                        .unwrap()
                        .unwrap()
                else {
                    panic!("context envelope")
                };
                context.in_scope(|| tracing::info!(event.name = "completed"));
            }
            tasks.shutdown().await;
        });
    });
    runtime.shutdown_timeout(Duration::from_secs(1));
    guard.shutdown();
    let requests = server.finish();
    let traces = requests
        .iter()
        .filter(|(p, _)| p == "/v1/traces")
        .map(|(_, b)| ExportTraceServiceRequest::decode(b.as_slice()).unwrap())
        .collect::<Vec<_>>();
    let spans: Vec<_> = traces
        .iter()
        .flat_map(|r| &r.resource_spans)
        .flat_map(|r| &r.scope_spans)
        .flat_map(|s| &s.spans)
        .collect();
    let roots: Vec<_> = spans
        .iter()
        .filter(|s| s.name == "teal.bench.script")
        .collect();
    assert_eq!(roots.len(), 2);
    assert_ne!(roots[0].trace_id, roots[1].trace_id);
    for root in &roots {
        assert!(spans.iter().any(|s| s.name == "teal.compile"
            && s.parent_span_id == root.span_id
            && s.trace_id == root.trace_id));
        assert!(
            spans
                .iter()
                .any(|s| s.name == "teal.execute" && s.trace_id == root.trace_id)
        );
    }
    let logs = requests
        .iter()
        .filter(|(p, _)| p == "/v1/logs")
        .map(|(_, b)| ExportLogsServiceRequest::decode(b.as_slice()).unwrap())
        .collect::<Vec<_>>();
    let records: Vec<_> = logs
        .iter()
        .flat_map(|r| &r.resource_logs)
        .flat_map(|r| &r.scope_logs)
        .flat_map(|s| &s.log_records)
        .collect();
    for root in roots {
        assert!(
            records
                .iter()
                .any(|r| r.trace_id == root.trace_id && r.span_id == root.span_id)
        );
    }
    let metrics = requests
        .iter()
        .filter(|(p, _)| p == "/v1/metrics")
        .map(|(_, b)| ExportMetricsServiceRequest::decode(b.as_slice()).unwrap())
        .collect::<Vec<_>>();
    let counters: Vec<_> = metrics
        .iter()
        .flat_map(|r| &r.resource_metrics)
        .flat_map(|r| &r.scope_metrics)
        .flat_map(|s| &s.metrics)
        .filter(|m| m.name == "teal.execute.count")
        .collect();
    assert_eq!(counters.len(), 1);
    let opentelemetry_proto::tonic::metrics::v1::metric::Data::Sum(sum) =
        counters[0].data.as_ref().unwrap()
    else {
        panic!("counter")
    };
    assert_eq!(
        sum.data_points
            .iter()
            .map(|p| match p.value {
                Some(opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsInt(
                    v,
                )) => v,
                _ => 0,
            })
            .sum::<i64>(),
        2
    );
    for (_, body) in &requests {
        assert!(
            !body
                .windows(b"private-token".len())
                .any(|w| w == b"private-token")
        );
    }
    let logs: String = std::fs::read_dir(dir.path().join("logs"))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect();
    assert!(!logs.contains("private-token"));
    assert!(logs.contains("trace_id"));
    assert!(logs.contains("completed"));
}
