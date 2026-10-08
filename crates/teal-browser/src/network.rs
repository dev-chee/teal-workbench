//! 受管页面的 CDP Fetch 请求拦截；许可固定在所属浏览器会话。

use std::{
    collections::{BTreeSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::{
    process::Child,
    sync::{Mutex, Notify},
    task::JoinHandle,
};
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use crate::{BrowserError, COMMAND_TIMEOUT, MAX_CDP_MESSAGE_BYTES};

const MAX_DEFERRED_REQUESTS: usize = 512;
const MAX_CHILD_SESSIONS: usize = 128;

#[derive(Clone)]
enum NetworkIssue {
    Approval(String),
    AuditUnavailable(String),
    AuditUnknown(String),
    Protocol(String),
}

pub(super) struct NetworkState {
    issue: Mutex<Option<NetworkIssue>>,
    changed: Notify,
    fatal: AtomicBool,
    child_seen: AtomicBool,
}

impl NetworkState {
    fn new() -> Self {
        Self {
            issue: Mutex::new(None),
            changed: Notify::new(),
            fatal: AtomicBool::new(false),
            child_seen: AtomicBool::new(false),
        }
    }

    async fn report(&self, issue: NetworkIssue) {
        if matches!(
            issue,
            NetworkIssue::Protocol(_) | NetworkIssue::AuditUnknown(_)
        ) {
            self.fatal.store(true, Ordering::Release);
        }
        let mut current = self.issue.lock().await;
        if current.is_none() {
            *current = Some(issue);
            self.changed.notify_waiters();
        }
    }

    pub(super) async fn check(&self) -> Result<(), BrowserError> {
        match self.issue.lock().await.clone() {
            None => Ok(()),
            Some(NetworkIssue::Approval(target)) => {
                Err(BrowserError::NetworkApprovalRequired { target })
            }
            Some(NetworkIssue::AuditUnavailable(message)) => {
                Err(BrowserError::NetworkAuditUnavailable(message))
            }
            Some(NetworkIssue::AuditUnknown(message)) => {
                Err(BrowserError::NetworkAuditUnknown(message))
            }
            Some(NetworkIssue::Protocol(message)) => Err(BrowserError::Invalid(message)),
        }
    }

    pub(super) async fn wait_issue(&self) {
        loop {
            let notified = self.changed.notified();
            if self.issue.lock().await.is_some() {
                return;
            }
            notified.await;
        }
    }

    async fn blocked(&self) -> bool {
        self.issue.lock().await.is_some()
    }
}

pub(super) struct NetworkGuard {
    state: Arc<NetworkState>,
    task: JoinHandle<()>,
    safety: Option<BrowserSafety>,
}

#[derive(Clone)]
pub(super) struct BrowserSafety {
    pub(super) child: Arc<Mutex<Child>>,
    pub(super) active: Arc<AtomicBool>,
}

impl Drop for NetworkGuard {
    fn drop(&mut self) {
        self.task.abort();
        if (self.state.fatal.load(Ordering::Acquire)
            || self.state.child_seen.load(Ordering::Acquire))
            && let Some(safety) = self.safety.take()
        {
            safety.active.store(false, Ordering::Release);
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _ = safety.child.lock().await.kill().await;
                });
            }
        }
    }
}

impl NetworkGuard {
    pub(super) async fn start(
        websocket: &str,
        allowed: Arc<Mutex<BTreeSet<String>>>,
        audit: teal_security::AuditContext,
        close_target: Option<String>,
        safety: Option<BrowserSafety>,
    ) -> Result<Self, BrowserError> {
        let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(MAX_CDP_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_CDP_MESSAGE_BYTES));
        let (mut socket, _) = tokio::time::timeout(
            COMMAND_TIMEOUT,
            tokio_tungstenite::connect_async_with_config(websocket, Some(config), false),
        )
        .await
        .map_err(|_| BrowserError::Timeout)??;
        let mut deferred = VecDeque::new();
        send_cdp(
            &mut socket,
            1,
            "Fetch.enable",
            json!({"patterns":[{"urlPattern":"*","requestStage":"Request"}]}),
            None,
        )
        .await?;
        wait_cdp_ack(&mut socket, 1, None, &mut deferred).await?;
        send_cdp(
            &mut socket,
            2,
            "Target.setAutoAttach",
            json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true}),
            None,
        )
        .await?;
        wait_cdp_ack(&mut socket, 2, None, &mut deferred).await?;
        let state = Arc::new(NetworkState::new());
        let active = Arc::clone(&state);
        let kill_on_loss = safety.clone();
        let task = tokio::spawn(async move {
            run_broker(socket, allowed, audit, active, deferred).await;
            if let Some(safety) = kill_on_loss {
                safety.active.store(false, Ordering::Release);
                let _ = safety.child.lock().await.kill().await;
            }
            if let Some(target) = close_target {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    reqwest::Client::new().get(target).send(),
                )
                .await;
            }
        });
        Ok(Self {
            state,
            task,
            safety,
        })
    }

    pub(super) fn state(&self) -> &Arc<NetworkState> {
        &self.state
    }
}

pub(super) fn normalize_network_url(input: &str) -> Result<String, BrowserError> {
    let mut url = Url::parse(input).map_err(|error| BrowserError::Invalid(error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.as_str().len() > 16_384
    {
        return Err(BrowserError::Invalid(
            "browser network URL must be HTTP/HTTPS without credentials".into(),
        ));
    }
    url.set_fragment(None);
    Ok(url.to_string())
}

async fn run_broker(
    mut socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    allowed: Arc<Mutex<BTreeSet<String>>>,
    audit: teal_security::AuditContext,
    state: Arc<NetworkState>,
    mut deferred: VecDeque<Value>,
) {
    let mut next_id = 3_u64;
    let mut child_sessions = BTreeSet::<String>::new();
    loop {
        let value = if let Some(value) = deferred.pop_front() {
            value
        } else {
            let message = match socket.next().await {
                Some(Ok(message)) => message,
                Some(Err(error)) => {
                    state
                        .report(NetworkIssue::Protocol(error.to_string()))
                        .await;
                    break;
                }
                None => break,
            };
            match parse_message(&mut socket, message).await {
                Ok(value) => value,
                Err(error) => {
                    state
                        .report(NetworkIssue::Protocol(error.to_string()))
                        .await;
                    break;
                }
            }
        };
        if value
            .get("id")
            .and_then(Value::as_u64)
            .is_some_and(|id| id >= 2)
            && let Some(error) = value.get("error")
        {
            state
                .report(NetworkIssue::Protocol(format!(
                    "CDP Fetch command failed: {error}"
                )))
                .await;
            break;
        }
        let method = value.get("method").and_then(Value::as_str);
        if method == Some("Target.attachedToTarget") {
            state.child_seen.store(true, Ordering::Release);
            let Some(session_id) = value.pointer("/params/sessionId").and_then(Value::as_str)
            else {
                state
                    .report(NetworkIssue::Protocol(
                        "CDP child session ID is missing".into(),
                    ))
                    .await;
                break;
            };
            if value
                .pointer("/params/waitingForDebugger")
                .and_then(Value::as_bool)
                != Some(true)
                || session_id.len() > 256
                || child_sessions.len() >= MAX_CHILD_SESSIONS
                || child_sessions.contains(session_id)
                || value
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .is_some_and(|parent| !child_sessions.contains(parent))
            {
                state
                    .report(NetworkIssue::Protocol(
                        "CDP child target was not paused before network interception".into(),
                    ))
                    .await;
                break;
            }
            if let Err(error) =
                configure_child_session(&mut socket, session_id, &mut next_id, &mut deferred).await
            {
                state
                    .report(NetworkIssue::Protocol(error.to_string()))
                    .await;
                break;
            }
            child_sessions.insert(session_id.to_owned());
            continue;
        }
        if method == Some("Target.detachedFromTarget") {
            if let Some(session_id) = value.pointer("/params/sessionId").and_then(Value::as_str) {
                child_sessions.remove(session_id);
            }
            continue;
        }
        if method != Some("Fetch.requestPaused") {
            continue;
        }
        let session_id = value.get("sessionId").and_then(Value::as_str);
        if session_id.is_some_and(|session| !child_sessions.contains(session)) {
            state
                .report(NetworkIssue::Protocol(
                    "CDP request came from an unguarded child target".into(),
                ))
                .await;
            break;
        }
        let Some(request_id) = value.pointer("/params/requestId").and_then(Value::as_str) else {
            state
                .report(NetworkIssue::Protocol(
                    "CDP Fetch request ID is missing".into(),
                ))
                .await;
            break;
        };
        let Some(raw_url) = value.pointer("/params/request/url").and_then(Value::as_str) else {
            state
                .report(NetworkIssue::Protocol(
                    "CDP Fetch request URL is missing".into(),
                ))
                .await;
            break;
        };
        if state.blocked().await {
            if send_fetch(
                &mut socket,
                next_id,
                "Fetch.failRequest",
                json!({"requestId":request_id,"errorReason":"BlockedByClient"}),
                session_id,
            )
            .await
            .is_err()
            {
                break;
            }
            next_id = next_id.saturating_add(1);
            continue;
        }
        let target = match normalize_network_url(raw_url) {
            Ok(target) => target,
            Err(_) => {
                state
                    .report(NetworkIssue::Approval("invalid browser network URL".into()))
                    .await;
                if send_fetch(
                    &mut socket,
                    next_id,
                    "Fetch.failRequest",
                    json!({"requestId":request_id,"errorReason":"BlockedByClient"}),
                    session_id,
                )
                .await
                .is_err()
                {
                    break;
                }
                next_id = next_id.saturating_add(1);
                continue;
            }
        };
        next_id = next_id.saturating_add(1);
        if !allowed.lock().await.contains(&target) {
            state.report(NetworkIssue::Approval(target)).await;
            if send_fetch(
                &mut socket,
                next_id,
                "Fetch.failRequest",
                json!({"requestId":request_id,"errorReason":"BlockedByClient"}),
                session_id,
            )
            .await
            .is_err()
            {
                break;
            }
            next_id = next_id.saturating_add(1);
            continue;
        }
        let event = match teal_security::Event::new(
            teal_security::Action::Browser,
            teal_security::Phase::Intent,
            teal_security::Outcome::Pending,
            teal_security::new_correlation_id(),
        ) {
            Ok(event) => event.with_actor(teal_security::Actor::Host),
            Err(error) => {
                state
                    .report(NetworkIssue::AuditUnavailable(error.to_string()))
                    .await;
                if send_fetch(
                    &mut socket,
                    next_id,
                    "Fetch.failRequest",
                    json!({"requestId":request_id,"errorReason":"BlockedByClient"}),
                    session_id,
                )
                .await
                .is_err()
                {
                    break;
                }
                next_id = next_id.saturating_add(1);
                continue;
            }
        };
        let attempt = match audit.begin(event) {
            Ok(attempt) => attempt,
            Err(error) => {
                state
                    .report(NetworkIssue::AuditUnavailable(error.to_string()))
                    .await;
                if send_fetch(
                    &mut socket,
                    next_id,
                    "Fetch.failRequest",
                    json!({"requestId":request_id,"errorReason":"BlockedByClient"}),
                    session_id,
                )
                .await
                .is_err()
                {
                    break;
                }
                next_id = next_id.saturating_add(1);
                continue;
            }
        };
        let dispatch = send_fetch(
            &mut socket,
            next_id,
            "Fetch.continueRequest",
            json!({"requestId":request_id}),
            session_id,
        )
        .await;
        // 只有 CDP 确认接收 continueRequest 后，才能把这次交付记为成功。
        let confirmed = match dispatch {
            Ok(()) => wait_cdp_ack(&mut socket, next_id, session_id, &mut deferred).await,
            Err(error) => Err(error),
        };
        if let Err(error) = confirmed {
            state
                .report(NetworkIssue::AuditUnknown(error.to_string()))
                .await;
            drop(attempt);
            break;
        }
        if let Err(error) = attempt.finish(teal_security::Outcome::Succeeded) {
            state
                .report(NetworkIssue::AuditUnknown(error.to_string()))
                .await;
            next_id = next_id.saturating_add(1);
            continue;
        }
        next_id = next_id.saturating_add(1);
    }
    state
        .report(NetworkIssue::Protocol(
            "CDP Fetch interception ended".into(),
        ))
        .await;
}

async fn configure_child_session(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    session_id: &str,
    next_id: &mut u64,
    deferred: &mut VecDeque<Value>,
) -> Result<(), BrowserError> {
    for (method, params) in [
        (
            "Fetch.enable",
            json!({"patterns":[{"urlPattern":"*","requestStage":"Request"}]}),
        ),
        (
            "Target.setAutoAttach",
            json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true}),
        ),
        ("Runtime.runIfWaitingForDebugger", json!({})),
    ] {
        let id = *next_id;
        send_cdp(socket, id, method, params, Some(session_id)).await?;
        wait_cdp_ack(socket, id, Some(session_id), deferred).await?;
        *next_id = (*next_id).saturating_add(1);
    }
    Ok(())
}

async fn wait_cdp_ack(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    expected_id: u64,
    expected_session: Option<&str>,
    deferred: &mut VecDeque<Value>,
) -> Result<(), BrowserError> {
    loop {
        let message = tokio::time::timeout(COMMAND_TIMEOUT, socket.next())
            .await
            .map_err(|_| BrowserError::Timeout)?
            .ok_or_else(|| BrowserError::Invalid("CDP Fetch acknowledgment is missing".into()))??;
        let value = parse_message(socket, message).await?;
        let id = value.get("id").and_then(Value::as_u64);
        if id == Some(expected_id) {
            if value.get("sessionId").and_then(Value::as_str) != expected_session {
                return Err(BrowserError::Invalid(
                    "CDP acknowledgment session mismatch".into(),
                ));
            }
            if let Some(error) = value.get("error") {
                return Err(BrowserError::Invalid(format!(
                    "CDP command failed: {error}"
                )));
            }
            return Ok(());
        }
        if id.is_some_and(|id| id >= 2)
            && let Some(error) = value.get("error")
        {
            return Err(BrowserError::Invalid(format!(
                "CDP Fetch command failed: {error}"
            )));
        }
        if value.get("method").and_then(Value::as_str).is_some() {
            if deferred.len() >= MAX_DEFERRED_REQUESTS {
                return Err(BrowserError::Invalid("CDP event queue is full".into()));
            }
            deferred.push_back(value);
        }
    }
}

async fn send_fetch(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    id: u64,
    method: &str,
    params: Value,
    session_id: Option<&str>,
) -> Result<(), BrowserError> {
    send_cdp(socket, id, method, params, session_id).await
}

async fn send_cdp(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    id: u64,
    method: &str,
    params: Value,
    session_id: Option<&str>,
) -> Result<(), BrowserError> {
    let mut command = json!({"id":id,"method":method,"params":params});
    if let Some(session_id) = session_id {
        command["sessionId"] = Value::String(session_id.to_owned());
    }
    socket
        .send(Message::Text(command.to_string().into()))
        .await?;
    Ok(())
}

async fn parse_message(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    message: Message,
) -> Result<Value, BrowserError> {
    let text = match message {
        Message::Text(text) => text.to_string(),
        Message::Binary(bytes) if bytes.len() <= MAX_CDP_MESSAGE_BYTES => {
            String::from_utf8(bytes.to_vec())
                .map_err(|_| BrowserError::Invalid("CDP network event is not UTF-8".into()))?
        }
        Message::Ping(payload) => {
            socket.send(Message::Pong(payload)).await?;
            return Ok(Value::Null);
        }
        Message::Close(_) => {
            return Err(BrowserError::Invalid(
                "CDP network connection closed".into(),
            ));
        }
        _ => return Ok(Value::Null),
    };
    if text.len() > MAX_CDP_MESSAGE_BYTES {
        return Err(BrowserError::Invalid(
            "CDP network event exceeds 16 MiB".into(),
        ));
    }
    serde_json::from_str(&text).map_err(BrowserError::from)
}

#[cfg(test)]
mod tests;
