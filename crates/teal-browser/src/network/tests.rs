//! CDP Fetch broker 的逐请求许可与审计回归。

use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicUsize, Ordering},
};

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use super::*;

struct SequencedAuditSink {
    events: std::sync::Mutex<Vec<teal_security::Event>>,
    fail_at: Option<usize>,
    appends: AtomicUsize,
}

impl SequencedAuditSink {
    fn new(fail_at: Option<usize>) -> Arc<Self> {
        Arc::new(Self {
            events: std::sync::Mutex::new(Vec::new()),
            fail_at,
            appends: AtomicUsize::new(0),
        })
    }
}

impl teal_security::AuditSink for SequencedAuditSink {
    fn append(
        &self,
        event: &teal_security::Event,
    ) -> Result<teal_security::Receipt, teal_security::AuditError> {
        self.events.lock().unwrap().push(event.clone());
        let sequence = self.appends.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_at == Some(sequence) {
            return Err(teal_security::AuditError::Host);
        }
        Ok(teal_security::Receipt {
            sequence: sequence as u64,
            hash: format!("{sequence:064x}"),
        })
    }
}

async fn fake_cdp(urls: Vec<&'static str>) -> (String, JoinHandle<Vec<Value>>) {
    fake_cdp_with_error(urls, None).await
}

async fn fake_cdp_with_error(
    urls: Vec<&'static str>,
    error_at: Option<usize>,
) -> (String, JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        let enable: Value =
            serde_json::from_str(&socket.next().await.unwrap().unwrap().into_text().unwrap())
                .unwrap();
        assert_eq!(enable["method"], "Fetch.enable");
        socket
            .send(Message::Text(
                json!({"id":1,"result":{}}).to_string().into(),
            ))
            .await
            .unwrap();
        let auto_attach: Value =
            serde_json::from_str(&socket.next().await.unwrap().unwrap().into_text().unwrap())
                .unwrap();
        assert_eq!(auto_attach["method"], "Target.setAutoAttach");
        assert_eq!(auto_attach["params"]["autoAttach"], true);
        assert_eq!(auto_attach["params"]["waitForDebuggerOnStart"], true);
        assert_eq!(auto_attach["params"]["flatten"], true);
        socket
            .send(Message::Text(
                json!({"id":auto_attach["id"],"result":{}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();

        let mut commands = Vec::new();
        for (index, url) in urls.into_iter().enumerate() {
            let request_id = format!("request-{index}");
            socket
                .send(Message::Text(
                    json!({
                        "method":"Fetch.requestPaused",
                        "params":{"requestId":request_id,"request":{"url":url}}
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            let command: Value =
                serde_json::from_str(&socket.next().await.unwrap().unwrap().into_text().unwrap())
                    .unwrap();
            socket
                .send(Message::Text(
                    if error_at == Some(index) {
                        json!({"id":command["id"],"error":{"message":"rejected"}})
                    } else {
                        json!({"id":command["id"],"result":{}})
                    }
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            commands.push(command);
        }
        commands
    });
    (format!("ws://{address}"), task)
}

async fn fake_cdp_with_nested_targets(unauthorized_url: &'static str) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();

        let mut saw_fetch = false;
        let mut saw_auto_attach = false;
        for _ in 0..2 {
            let command: Value =
                serde_json::from_str(&socket.next().await.unwrap().unwrap().into_text().unwrap())
                    .unwrap();
            match command["method"].as_str().unwrap() {
                "Fetch.enable" => saw_fetch = true,
                "Target.setAutoAttach" => {
                    saw_auto_attach = true;
                    assert_eq!(command["params"]["autoAttach"], true);
                    assert_eq!(command["params"]["waitForDebuggerOnStart"], true);
                    assert_eq!(command["params"]["flatten"], true);
                }
                method => panic!("unexpected root setup command: {method}"),
            }
            socket
                .send(Message::Text(
                    json!({"id":command["id"],"result":{}}).to_string().into(),
                ))
                .await
                .unwrap();
        }
        assert!(saw_fetch && saw_auto_attach);

        for (parent_session, session, target_type) in [
            (None, "popup-session", "page"),
            (Some("popup-session"), "worker-session", "worker"),
        ] {
            let mut attached = json!({
                "method":"Target.attachedToTarget",
                "params":{
                    "sessionId":session,
                    "targetInfo":{
                        "targetId":format!("{session}-target"),
                        "type":target_type,
                        "url":"about:blank",
                        "attached":true
                    },
                    "waitingForDebugger":true
                }
            });
            if let Some(parent_session) = parent_session {
                attached["sessionId"] = json!(parent_session);
            }
            socket
                .send(Message::Text(attached.to_string().into()))
                .await
                .unwrap();

            for expected_method in [
                "Fetch.enable",
                "Target.setAutoAttach",
                "Runtime.runIfWaitingForDebugger",
            ] {
                let command: Value = serde_json::from_str(
                    &socket.next().await.unwrap().unwrap().into_text().unwrap(),
                )
                .unwrap();
                assert_eq!(command["sessionId"], session);
                assert_eq!(command["method"], expected_method);
                if expected_method == "Target.setAutoAttach" {
                    assert_eq!(command["params"]["autoAttach"], true);
                    assert_eq!(command["params"]["waitForDebuggerOnStart"], true);
                    assert_eq!(command["params"]["flatten"], true);
                }
                socket
                    .send(Message::Text(
                        json!({"id":command["id"],"sessionId":session,"result":{}})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
            }
        }

        socket
            .send(Message::Text(
                json!({
                    "sessionId":"worker-session",
                    "method":"Fetch.requestPaused",
                    "params":{
                        "requestId":"worker-request",
                        "request":{"url":unauthorized_url}
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let blocked: Value =
            serde_json::from_str(&socket.next().await.unwrap().unwrap().into_text().unwrap())
                .unwrap();
        assert_eq!(blocked["sessionId"], "worker-session");
        assert_eq!(blocked["method"], "Fetch.failRequest");
        assert_eq!(blocked["params"]["requestId"], "worker-request");
        socket
            .send(Message::Text(
                json!({"id":blocked["id"],"sessionId":"worker-session","result":{}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
    });
    (format!("ws://{address}"), task)
}

#[tokio::test]
async fn popup_and_nested_worker_are_guarded_before_they_can_run() {
    let target = "https://unapproved.example.test/from-worker";
    let (endpoint, server) = fake_cdp_with_nested_targets(target).await;
    let sink = SequencedAuditSink::new(None);
    let guard = NetworkGuard::start(
        &endpoint,
        Arc::new(Mutex::new(BTreeSet::new())),
        teal_security::AuditContext::new(sink.clone()),
        None,
        None,
    )
    .await
    .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        guard.state().wait_issue().await;
        server.await.unwrap();
    })
    .await
    .expect("popup and worker target setup should not stall");
    let error = guard.state().check().await.unwrap_err();
    assert!(error.to_string().contains(target), "{error}");
    assert!(
        sink.events
            .lock()
            .unwrap()
            .iter()
            .all(|event| event.outcome != teal_security::Outcome::Succeeded)
    );
}

#[tokio::test]
async fn rejected_continue_ack_reports_unknown_without_a_success_audit() {
    let target = "https://example.test/rejected-continue";
    let (endpoint, server) = fake_cdp_with_error(vec![target], Some(0)).await;
    let allowed = Arc::new(Mutex::new(BTreeSet::from([target.to_owned()])));
    let sink = SequencedAuditSink::new(None);
    let guard = NetworkGuard::start(
        &endpoint,
        allowed,
        teal_security::AuditContext::new(sink.clone()),
        None,
        None,
    )
    .await
    .unwrap();

    guard.state().wait_issue().await;
    assert!(matches!(
        guard.state().check().await,
        Err(BrowserError::NetworkAuditUnknown(_))
    ));
    let commands = server.await.unwrap();
    assert_eq!(commands[0]["method"], "Fetch.continueRequest");
    let events = sink.events.lock().unwrap();
    assert!(
        events
            .iter()
            .all(|event| event.outcome != teal_security::Outcome::Succeeded)
    );
    assert!(
        events
            .iter()
            .any(|event| event.outcome == teal_security::Outcome::Unknown)
    );
}

#[tokio::test]
async fn unapproved_request_is_rejected_without_a_success_audit() {
    let target = "https://unapproved.example.test/resource";
    let (endpoint, server) = fake_cdp(vec![target]).await;
    let allowed = Arc::new(Mutex::new(BTreeSet::new()));
    let sink = SequencedAuditSink::new(None);
    let guard = NetworkGuard::start(
        &endpoint,
        allowed,
        teal_security::AuditContext::new(sink.clone()),
        None,
        None,
    )
    .await
    .unwrap();

    guard.state().wait_issue().await;
    assert!(
        guard
            .state()
            .check()
            .await
            .unwrap_err()
            .to_string()
            .contains(target)
    );
    let commands = server.await.unwrap();
    assert_eq!(commands[0]["method"], "Fetch.failRequest");
    assert!(sink.events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn allowed_initial_request_continues_but_redirect_blocks_the_remaining_flow() {
    let initial = "https://example.test/start";
    let redirect = "https://cdn.example.test/redirected";
    let later = "https://example.test/later";
    let (endpoint, server) = fake_cdp(vec![initial, redirect, later]).await;
    let allowed = Arc::new(Mutex::new(BTreeSet::from([initial.to_owned()])));
    let sink = SequencedAuditSink::new(None);
    let guard = NetworkGuard::start(
        &endpoint,
        allowed,
        teal_security::AuditContext::new(sink.clone()),
        None,
        None,
    )
    .await
    .unwrap();

    guard.state().wait_issue().await;
    let error = guard.state().check().await.unwrap_err();
    assert!(error.to_string().contains(redirect), "{error}");
    let commands = server.await.unwrap();
    assert_eq!(commands[0]["method"], "Fetch.continueRequest");
    assert_eq!(commands[1]["method"], "Fetch.failRequest");
    assert_eq!(commands[2]["method"], "Fetch.failRequest");
    let events = sink.events.lock().unwrap();
    assert_eq!(events.len(), 2, "only the allowed request is audited");
    assert!(!serde_json::to_string(&*events).unwrap().contains(initial));
    assert!(!serde_json::to_string(&*events).unwrap().contains(redirect));
}

#[tokio::test]
async fn audit_intent_failure_fails_the_request_before_it_is_sent() {
    let target = "https://example.test/intent";
    let (endpoint, server) = fake_cdp(vec![target]).await;
    let allowed = Arc::new(Mutex::new(BTreeSet::from([target.to_owned()])));
    let sink = SequencedAuditSink::new(Some(1));
    let guard = NetworkGuard::start(
        &endpoint,
        allowed,
        teal_security::AuditContext::new(sink),
        None,
        None,
    )
    .await
    .unwrap();

    guard.state().wait_issue().await;
    assert!(matches!(
        guard.state().check().await,
        Err(BrowserError::NetworkAuditUnavailable(_))
    ));
    let commands = server.await.unwrap();
    assert_eq!(commands[0]["method"], "Fetch.failRequest");
}

#[tokio::test]
async fn audit_result_failure_blocks_every_later_request() {
    let first = "https://example.test/first";
    let later = "https://example.test/later";
    let (endpoint, server) = fake_cdp(vec![first, later]).await;
    let allowed = Arc::new(Mutex::new(BTreeSet::from([
        first.to_owned(),
        later.to_owned(),
    ])));
    let sink = SequencedAuditSink::new(Some(2));
    let guard = NetworkGuard::start(
        &endpoint,
        allowed,
        teal_security::AuditContext::new(sink),
        None,
        None,
    )
    .await
    .unwrap();

    guard.state().wait_issue().await;
    assert!(matches!(
        guard.state().check().await,
        Err(BrowserError::NetworkAuditUnknown(_))
    ));
    let commands = server.await.unwrap();
    assert_eq!(commands[0]["method"], "Fetch.continueRequest");
    assert_eq!(commands[1]["method"], "Fetch.failRequest");
}
