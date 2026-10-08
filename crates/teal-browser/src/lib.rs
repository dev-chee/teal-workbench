//! 受管 Chromium 会话、页面和网络授权的本机能力。
#![deny(missing_docs)]

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use futures_util::{SinkExt as _, StreamExt as _};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    process::{Child, Command},
    sync::{Mutex, OwnedSemaphorePermit, Semaphore},
};
use tokio_tungstenite::tungstenite::Message;
mod network;
use network::{BrowserSafety, NetworkGuard, normalize_network_url};

const START_TIMEOUT: Duration = Duration::from_secs(15);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CDP_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_SESSIONS: usize = 8;
const MAX_PAGES_PER_SESSION: usize = 64;
const MAX_AX_NODES: usize = 4_096;

/// 受管浏览器会话的稳定进程内身份。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BrowserSessionId(pub String);

/// 一个受管页面及其当前交互代次。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserPage {
    /// 所属会话。
    pub session: BrowserSessionId,
    /// CDP 页面目标 ID。
    pub page_id: String,
    /// 当前最终 URL。
    pub url: String,
    /// 页面标题。
    pub title: String,
    /// 节点引用适用的页面代次。
    pub generation: u64,
}

/// 可访问性树快照；树节点由 CDP 原始 JSON 表示。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrowserSnapshot {
    /// 页面身份及本快照代次。
    pub page: BrowserPage,
    /// `Accessibility.getFullAXTree` 返回的节点数组。
    pub nodes: Vec<Value>,
}

/// Chromium 发现、启动、CDP 通信或页面代次失败。
#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    /// 必需的持久审计写入失败。
    #[error("Managed browser audit failed: {0}")]
    Audit(#[from] teal_security::AuditError),
    /// 浏览器动作可能已发生，但审计结果无法确认。
    #[error("Managed browser audit result unavailable; effect may be unknown: {0}")]
    AuditUnknown(teal_security::AuditError),
    /// 页面尝试请求尚未获准的精确网络目标。
    #[error("Managed browser network request requires approval: {target}")]
    NetworkApprovalRequired {
        /// 被拦截的规范目标。
        target: String,
    },
    /// 单次请求的审计意图失败，请求未发送。
    #[error("Managed browser network audit unavailable; request was blocked: {0}")]
    NetworkAuditUnavailable(String),
    /// 请求可能已交给 Chromium，但审计结果无法确认。
    #[error("Managed browser network audit result unavailable; effect may be unknown: {0}")]
    NetworkAuditUnknown(String),
    /// 没有找到可用的 Edge 或 Chrome。
    #[error("Managed Edge or Chrome executable was not found")]
    MissingExecutable,
    /// 浏览器进程或 profile 文件访问失败。
    #[error("Managed browser I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// CDP HTTP endpoint 请求失败。
    #[error("Managed browser HTTP discovery failed: {0}")]
    Http(#[from] reqwest::Error),
    /// CDP WebSocket 失败。
    #[error("Managed browser CDP transport failed: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    /// CDP 返回无效 JSON。
    #[error("Managed browser CDP JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    /// 会话、页面、节点或协议回复无效。
    #[error("Managed browser request is invalid: {0}")]
    Invalid(String),
    /// 浏览器或单条 CDP 命令超过期限。
    #[error("Managed browser operation timed out")]
    Timeout,
}

/// 拥有全部受管浏览器进程和页面代次的异步服务。
///
/// 会话和页面名额在启动进程或创建 target 之前由 semaphore 预占；创建失败、取消或
/// 关闭会自动释放对应 permit。运行中的会话/页面保留 permit，避免并发请求超卖配额。
#[derive(Clone)]
pub struct BrowserManager {
    audit: teal_security::AuditContext,
    sessions: Arc<Mutex<BTreeMap<BrowserSessionId, BrowserSession>>>,
    session_slots: Arc<Semaphore>,
    next: Arc<AtomicU64>,
    executable: Arc<Option<PathBuf>>,
}

struct BrowserSession {
    child: Arc<Mutex<Child>>,
    active: Arc<AtomicBool>,
    _profile: tempfile::TempDir,
    endpoint: String,
    pages: BTreeMap<String, Arc<Mutex<PageState>>>,
    allowed_urls: Arc<Mutex<BTreeSet<String>>>,
    page_slots: Arc<Semaphore>,
    _session_slot: OwnedSemaphorePermit,
}

struct PageState {
    websocket: String,
    url: String,
    title: String,
    generation: u64,
    snapshot_nodes: Option<Vec<Value>>,
    network: NetworkGuard,
    _page_slot: OwnedSemaphorePermit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetInfo {
    id: String,
    web_socket_debugger_url: String,
}

/// `/json/new` 成功后到登记页面之间的清理守卫；取消或导航失败不能遗留孤儿 target。
struct TargetCleanup {
    endpoint: String,
    id: Option<String>,
}

impl TargetCleanup {
    fn new(endpoint: String, id: String) -> Self {
        Self {
            endpoint,
            id: Some(id),
        }
    }

    fn disarm(&mut self) {
        self.id = None;
    }
}

impl Drop for TargetCleanup {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else { return };
        let url = format!("{}/json/close/{id}", self.endpoint);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = reqwest::Client::new().get(url).send().await;
            });
        }
    }
}

impl BrowserManager {
    /// 为外部浏览器动作指定宿主提供的持久审计接收器。
    #[must_use]
    pub fn with_audit_context(mut self, audit: teal_security::AuditContext) -> Self {
        self.audit = audit;
        self
    }

    async fn audit_operation<T>(
        &self,
        operation: impl std::future::Future<Output = Result<T, BrowserError>>,
    ) -> Result<T, BrowserError> {
        let event = teal_security::Event::new(
            teal_security::Action::Browser,
            teal_security::Phase::Intent,
            teal_security::Outcome::Pending,
            teal_security::new_correlation_id(),
        )?
        .with_actor(teal_security::Actor::Host);
        let audit = self.audit.begin(event)?;
        let result = operation.await;
        audit
            .finish(if result.is_ok() {
                teal_security::Outcome::Succeeded
            } else {
                teal_security::Outcome::Failed
            })
            .map_err(BrowserError::AuditUnknown)?;
        result
    }

    /// 返回本管理器创建时固定的 Chromium 可执行文件。
    ///
    /// # Errors
    /// 没有通过环境或平台安装位置发现 Edge/Chrome 时返回错误。
    pub fn executable(&self) -> Result<&Path, BrowserError> {
        self.executable
            .as_deref()
            .ok_or(BrowserError::MissingExecutable)
    }

    /// 启动使用临时独立 profile 的受管浏览器。
    ///
    /// # Errors
    /// 浏览器不可用、启动失败或未在期限内写出 CDP endpoint 时返回错误。
    #[tracing::instrument(name = "teal.browser.start", skip_all)]
    pub async fn start(&self, visible: bool) -> Result<BrowserSessionId, BrowserError> {
        self.audit_operation(self.start_inner(visible)).await
    }

    async fn start_inner(&self, visible: bool) -> Result<BrowserSessionId, BrowserError> {
        let session_slot = self
            .session_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                BrowserError::Invalid("managed browser session limit was reached".into())
            })?;
        let executable = self.executable()?;
        let profile = tempfile::tempdir()?;
        let mut command = Command::new(executable);
        command
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if !visible {
            command.arg("--headless=new");
        }
        let child = command.spawn()?;
        let endpoint = wait_endpoint(profile.path()).await?;
        let id = BrowserSessionId(format!(
            "browser-{}-{}",
            std::process::id(),
            self.next.fetch_add(1, Ordering::Relaxed)
        ));
        self.sessions.lock().await.insert(
            id.clone(),
            BrowserSession {
                child: Arc::new(Mutex::new(child)),
                active: Arc::new(AtomicBool::new(true)),
                _profile: profile,
                endpoint,
                pages: BTreeMap::new(),
                allowed_urls: Arc::new(Mutex::new(BTreeSet::new())),
                page_slots: Arc::new(Semaphore::new(MAX_PAGES_PER_SESSION)),
                _session_slot: session_slot,
            },
        );
        Ok(id)
    }

    /// 在调用方已批准精确 Network 资源后，为一个受管会话登记该 URL。
    ///
    /// 只登记规范 HTTP/HTTPS URL，不发送请求。登记前持久审计意图和结果必须均成功；
    /// 关闭会话会丢弃许可。嵌入宿主必须先独立授权该网络目标。
    ///
    /// # Errors
    /// 会话、URL 或审计不可用时拒绝登记。
    pub async fn allow_network(
        &self,
        session: &BrowserSessionId,
        url: &str,
    ) -> Result<String, BrowserError> {
        let target = normalize_network_url(url)?;
        let sessions = self.sessions.lock().await;
        let browser = sessions
            .get(session)
            .ok_or_else(|| BrowserError::Invalid("browser session is unknown".into()))?;
        if !browser.active.load(Ordering::Acquire) {
            return Err(BrowserError::Invalid(
                "browser session stopped; start a new session".into(),
            ));
        }
        let allowed = Arc::clone(&browser.allowed_urls);
        let event = teal_security::Event::new(
            teal_security::Action::Browser,
            teal_security::Phase::Intent,
            teal_security::Outcome::Pending,
            teal_security::new_correlation_id(),
        )?
        .with_actor(teal_security::Actor::Host);
        self.audit
            .begin(event)?
            .finish(teal_security::Outcome::Succeeded)
            .map_err(BrowserError::AuditUnknown)?;
        if !browser.active.load(Ordering::Acquire) {
            return Err(BrowserError::Invalid(
                "browser session stopped; start a new session".into(),
            ));
        }
        // 保持会话锁直到许可写入，关闭会话不能夹在审计和登记之间。
        allowed.lock().await.insert(target.clone());
        Ok(target)
    }

    /// 在受管会话中打开 HTTP/HTTPS 页面。
    ///
    /// # Errors
    /// 会话不存在、URL 非 HTTP/HTTPS 或 CDP target 创建失败时返回错误。
    #[tracing::instrument(name = "teal.browser.open", skip_all)]
    pub async fn open(
        &self,
        session: &BrowserSessionId,
        url: &str,
    ) -> Result<BrowserPage, BrowserError> {
        self.audit_operation(self.open_inner(session, url)).await
    }

    async fn open_inner(
        &self,
        session: &BrowserSessionId,
        url: &str,
    ) -> Result<BrowserPage, BrowserError> {
        let url = normalize_network_url(url)?;
        let (page_slot, endpoint, allowed, child, active) = {
            let sessions = self.sessions.lock().await;
            let session = sessions
                .get(session)
                .ok_or_else(|| BrowserError::Invalid("browser session is unknown".into()))?;
            if !session.active.load(Ordering::Acquire) {
                return Err(BrowserError::Invalid(
                    "browser session stopped; start a new session".into(),
                ));
            }
            let slot = session
                .page_slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| {
                    BrowserError::Invalid("managed browser page limit was reached".into())
                })?;
            (
                slot,
                session.endpoint.clone(),
                Arc::clone(&session.allowed_urls),
                Arc::clone(&session.child),
                Arc::clone(&session.active),
            )
        };
        if !allowed.lock().await.contains(&url) {
            return Err(BrowserError::NetworkApprovalRequired { target: url });
        }
        let request = format!("{endpoint}/json/new?about%3Ablank");
        let target: TargetInfo = reqwest::Client::new()
            .put(request)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let mut cleanup = TargetCleanup::new(endpoint.clone(), target.id.clone());
        let network = NetworkGuard::start(
            &target.web_socket_debugger_url,
            allowed,
            self.audit.clone(),
            Some(format!("{endpoint}/json/close/{}", target.id)),
            Some(BrowserSafety {
                child: Arc::clone(&child),
                active: Arc::clone(&active),
            }),
        )
        .await;
        let network = match network {
            Ok(network) => network,
            Err(error) => {
                active.store(false, Ordering::Release);
                let _ = child.lock().await.kill().await;
                return Err(error);
            }
        };
        let navigation = command(
            &target.web_socket_debugger_url,
            "Page.navigate",
            json!({"url":url}),
        )
        .await?;
        network.state().check().await?;
        if let Some(error) = navigation.get("errorText").and_then(Value::as_str) {
            return Err(BrowserError::Invalid(format!(
                "browser navigation failed: {error}"
            )));
        }
        let document = tokio::select! {
            result = wait_document(&target.web_socket_debugger_url) => Some(result),
            () = network.state().wait_issue() => None,
        };
        network.state().check().await?;
        let (final_url, title) = document.ok_or_else(|| {
            BrowserError::Invalid("browser network interception stopped".into())
        })??;
        let page = BrowserPage {
            session: session.clone(),
            page_id: target.id.clone(),
            url: final_url.clone(),
            title: title.clone(),
            generation: 1,
        };
        let mut sessions = self.sessions.lock().await;
        let current = sessions.get_mut(session).ok_or_else(|| {
            BrowserError::Invalid("browser session closed during navigation".into())
        })?;
        current.pages.insert(
            target.id,
            Arc::new(Mutex::new(PageState {
                websocket: target.web_socket_debugger_url,
                url: final_url,
                title,
                generation: 1,
                snapshot_nodes: None,
                network,
                _page_slot: page_slot,
            })),
        );
        cleanup.disarm();
        Ok(page)
    }

    /// 读取当前页面的完整可访问性树并推进节点代次。
    ///
    /// # Errors
    /// 页面不存在、CDP 失败或响应不含节点数组时返回错误。
    pub async fn snapshot(
        &self,
        session: &BrowserSessionId,
        page_id: &str,
    ) -> Result<BrowserSnapshot, BrowserError> {
        let page = self.page_state(session, page_id).await?;
        let mut state = page.lock().await;
        state.network.state().check().await?;
        let nodes = accessibility_nodes(&state.websocket).await?;
        state.network.state().check().await?;
        state.generation = state.generation.saturating_add(1);
        state.snapshot_nodes = Some(nodes.clone());
        Ok(BrowserSnapshot {
            page: BrowserPage {
                session: session.clone(),
                page_id: page_id.into(),
                url: state.url.clone(),
                title: state.title.clone(),
                generation: state.generation,
            },
            nodes,
        })
    }

    /// 捕获受管页面当前视口的 PNG；必须提交当前快照代次。
    ///
    /// # Errors
    /// 页面或代次失效、页面已变化、响应过大或 CDP 失败时拒绝。
    pub async fn screenshot(
        &self,
        session: &BrowserSessionId,
        page_id: &str,
        generation: u64,
    ) -> Result<Vec<u8>, BrowserError> {
        use base64::Engine;
        let page = self.page_state(session, page_id).await?;
        let state = page.lock().await;
        state.network.state().check().await?;
        state.require_generation(generation)?;
        require_unchanged_snapshot(&state).await?;
        let result = command(
            &state.websocket,
            "Page.captureScreenshot",
            json!({"format":"png","fromSurface":true,"captureBeyondViewport":false}),
        )
        .await?;
        let data = result["data"]
            .as_str()
            .ok_or_else(|| BrowserError::Invalid("Screenshot data is missing".into()))?;
        if data.len() > 12 * 1024 * 1024 {
            return Err(BrowserError::Invalid(
                "Screenshot exceeds encoding limit".into(),
            ));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| BrowserError::Invalid("Screenshot Base64 is invalid".into()))?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(BrowserError::Invalid("Screenshot exceeds 8 MiB".into()));
        }
        require_unchanged_snapshot(&state).await?;
        state.network.state().check().await?;
        Ok(bytes)
    }

    /// 按可访问性节点的 backend DOM ID 单击元素。
    ///
    /// # Errors
    /// 页面代次过期、节点没有可点击盒模型或 CDP 输入失败时返回错误。
    #[tracing::instrument(name = "teal.browser.click", skip_all)]
    pub async fn click(
        &self,
        session: &BrowserSessionId,
        page_id: &str,
        backend_node_id: i64,
        generation: u64,
    ) -> Result<(), BrowserError> {
        self.audit_operation(self.click_inner(session, page_id, backend_node_id, generation))
            .await
    }

    async fn click_inner(
        &self,
        session: &BrowserSessionId,
        page_id: &str,
        backend_node_id: i64,
        generation: u64,
    ) -> Result<(), BrowserError> {
        let page = self.page_state(session, page_id).await?;
        let mut state = page.lock().await;
        state.network.state().check().await?;
        state.require_generation(generation)?;
        require_unchanged_snapshot(&state).await?;
        let model = command(
            &state.websocket,
            "DOM.getBoxModel",
            json!({"backendNodeId":backend_node_id}),
        )
        .await?;
        let quad = model
            .pointer("/model/content")
            .and_then(Value::as_array)
            .ok_or_else(|| BrowserError::Invalid("browser node has no content box".into()))?;
        let coordinates = quad.iter().filter_map(Value::as_f64).collect::<Vec<_>>();
        if coordinates.len() < 8 {
            return Err(BrowserError::Invalid(
                "browser node box is incomplete".into(),
            ));
        }
        let x = coordinates.iter().step_by(2).sum::<f64>() / 4.0;
        let y = coordinates.iter().skip(1).step_by(2).sum::<f64>() / 4.0;
        // 一次代次只能尝试一次输入；即使 CDP 在部分输入后失败也不能重放。
        state.advance_generation();
        command(
            &state.websocket,
            "Input.dispatchMouseEvent",
            json!({"type":"mousePressed","x":x,"y":y,"button":"left","clickCount":1}),
        )
        .await?;
        command(
            &state.websocket,
            "Input.dispatchMouseEvent",
            json!({"type":"mouseReleased","x":x,"y":y,"button":"left","clickCount":1}),
        )
        .await?;
        state.network.state().check().await?;
        Ok(())
    }

    /// 聚焦非密码元素并插入文本，不执行页面 JavaScript。
    ///
    /// # Errors
    /// 页面代次过期、节点无法聚焦或 CDP 输入失败时返回错误。
    #[tracing::instrument(name = "teal.browser.fill", skip_all)]
    pub async fn fill(
        &self,
        session: &BrowserSessionId,
        page_id: &str,
        backend_node_id: i64,
        generation: u64,
        text: &str,
    ) -> Result<(), BrowserError> {
        self.audit_operation(self.fill_inner(session, page_id, backend_node_id, generation, text))
            .await
    }

    async fn fill_inner(
        &self,
        session: &BrowserSessionId,
        page_id: &str,
        backend_node_id: i64,
        generation: u64,
        text: &str,
    ) -> Result<(), BrowserError> {
        if text.len() > 1024 * 1024 {
            return Err(BrowserError::Invalid(
                "browser fill text exceeds 1 MiB".into(),
            ));
        }
        let page = self.page_state(session, page_id).await?;
        let mut state = page.lock().await;
        state.network.state().check().await?;
        state.require_generation(generation)?;
        require_unchanged_snapshot(&state).await?;
        let described = command(
            &state.websocket,
            "DOM.describeNode",
            json!({"backendNodeId":backend_node_id,"depth":0}),
        )
        .await?;
        let attributes = described
            .pointer("/node/attributes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if attributes.as_chunks::<2>().0.iter().any(|pair| {
            pair[0]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case("type"))
                && pair[1]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case("password"))
        }) {
            return Err(BrowserError::Invalid(
                "browser fill cannot write password controls".into(),
            ));
        }
        // 聚焦本身即可改变页面状态，因此在第一个输入命令前消费代次。
        state.advance_generation();
        command(
            &state.websocket,
            "DOM.focus",
            json!({"backendNodeId":backend_node_id}),
        )
        .await?;
        command(&state.websocket, "Input.insertText", json!({"text":text})).await?;
        state.network.state().check().await?;
        Ok(())
    }

    /// 关闭一个受管浏览器进程及其临时 profile。
    ///
    /// # Errors
    /// 会话不存在或进程终止失败时返回错误。
    #[tracing::instrument(name = "teal.browser.close", skip_all)]
    pub async fn close(&self, session: &BrowserSessionId) -> Result<(), BrowserError> {
        self.audit_operation(self.close_inner(session)).await
    }

    async fn close_inner(&self, session: &BrowserSessionId) -> Result<(), BrowserError> {
        let session_id = session.clone();
        let session = self
            .sessions
            .lock()
            .await
            .remove(session)
            .ok_or_else(|| BrowserError::Invalid("browser session is unknown".into()))?;
        let result = {
            let mut child = session.child.lock().await;
            match child.kill().await {
                Ok(()) => child.wait().await,
                Err(error) => child.try_wait()?.ok_or(error),
            }
        };
        if let Err(error) = result {
            self.sessions.lock().await.insert(session_id, session);
            return Err(error.into());
        }
        Ok(())
    }

    async fn page_state(
        &self,
        session: &BrowserSessionId,
        page_id: &str,
    ) -> Result<Arc<Mutex<PageState>>, BrowserError> {
        let sessions = self.sessions.lock().await;
        sessions
            .get(session)
            .and_then(|session| session.pages.get(page_id))
            .cloned()
            .ok_or_else(|| BrowserError::Invalid("browser page is unknown".into()))
    }
}

impl PageState {
    fn require_generation(&self, generation: u64) -> Result<(), BrowserError> {
        if self.generation == generation {
            Ok(())
        } else {
            Err(BrowserError::Invalid(
                "browser page generation is stale".into(),
            ))
        }
    }

    fn advance_generation(&mut self) {
        self.generation = self.generation.saturating_add(1);
        self.snapshot_nodes = None;
    }
}

async fn accessibility_nodes(websocket: &str) -> Result<Vec<Value>, BrowserError> {
    let result = command(websocket, "Accessibility.getFullAXTree", json!({})).await?;
    let nodes = result
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| BrowserError::Invalid("accessibility snapshot has no nodes".into()))?;
    if nodes.len() > MAX_AX_NODES {
        return Err(BrowserError::Invalid(
            "accessibility snapshot exceeds 4096 nodes".into(),
        ));
    }
    Ok(nodes)
}

async fn require_unchanged_snapshot(state: &PageState) -> Result<(), BrowserError> {
    let expected = state.snapshot_nodes.as_ref().ok_or_else(|| {
        BrowserError::Invalid("browser action requires a current accessibility snapshot".into())
    })?;
    if accessibility_nodes(&state.websocket).await? == *expected {
        Ok(())
    } else {
        Err(BrowserError::Invalid(
            "browser page changed after its accessibility snapshot".into(),
        ))
    }
}

async fn wait_document(websocket: &str) -> Result<(String, String), BrowserError> {
    let started = tokio::time::Instant::now();
    loop {
        let document = command(websocket, "DOM.getDocument", json!({"depth":0})).await?;
        let document_url = document
            .pointer("/root/documentURL")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if url::Url::parse(document_url).is_ok_and(|url| matches!(url.scheme(), "http" | "https")) {
            let target = command(websocket, "Target.getTargetInfo", json!({})).await?;
            let title = target
                .pointer("/targetInfo/title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            return Ok((document_url.to_owned(), title));
        }
        if started.elapsed() >= START_TIMEOUT {
            return Err(BrowserError::Timeout);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

impl Default for BrowserManager {
    fn default() -> Self {
        Self {
            audit: teal_security::process_context(),
            sessions: Arc::default(),
            next: Arc::default(),
            executable: Arc::new(browser_executable()),
            session_slots: Arc::new(Semaphore::new(MAX_SESSIONS)),
        }
    }
}

async fn wait_endpoint(profile: &Path) -> Result<String, BrowserError> {
    let path = profile.join("DevToolsActivePort");
    let started = tokio::time::Instant::now();
    loop {
        match tokio::fs::read_to_string(&path).await {
            Ok(value) => {
                let port = value
                    .lines()
                    .next()
                    .and_then(|value| value.parse::<u16>().ok())
                    .ok_or_else(|| BrowserError::Invalid("DevToolsActivePort is invalid".into()))?;
                return Ok(format!("http://127.0.0.1:{port}"));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if started.elapsed() >= START_TIMEOUT {
            return Err(BrowserError::Timeout);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn command(websocket: &str, method: &str, params: Value) -> Result<Value, BrowserError> {
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_CDP_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_CDP_MESSAGE_BYTES));
    let (mut socket, _) = tokio::time::timeout(
        COMMAND_TIMEOUT,
        tokio_tungstenite::connect_async_with_config(websocket, Some(config), false),
    )
    .await
    .map_err(|_| BrowserError::Timeout)??;
    let request = json!({"id":1,"method":method,"params":params}).to_string();
    socket.send(Message::Text(request.into())).await?;
    loop {
        let message = tokio::time::timeout(COMMAND_TIMEOUT, socket.next())
            .await
            .map_err(|_| BrowserError::Timeout)?
            .ok_or_else(|| {
                BrowserError::Invalid("CDP connection closed before a response".into())
            })??;
        let text = match message {
            Message::Text(text) => text,
            Message::Binary(bytes) if bytes.len() <= MAX_CDP_MESSAGE_BYTES => {
                String::from_utf8(bytes.to_vec())
                    .map_err(|_| BrowserError::Invalid("CDP response is not UTF-8".into()))?
                    .into()
            }
            Message::Ping(value) => {
                socket.send(Message::Pong(value)).await?;
                continue;
            }
            Message::Close(_) => {
                return Err(BrowserError::Invalid(
                    "CDP connection closed before a response".into(),
                ));
            }
            _ => continue,
        };
        if text.len() > MAX_CDP_MESSAGE_BYTES {
            return Err(BrowserError::Invalid("CDP response exceeds 16 MiB".into()));
        }
        let response: Value = serde_json::from_str(&text)?;
        if response.get("id") != Some(&json!(1)) {
            continue;
        }
        if let Some(error) = response.get("error") {
            return Err(BrowserError::Invalid(format!(
                "CDP command failed: {error}"
            )));
        }
        return response
            .get("result")
            .cloned()
            .ok_or_else(|| BrowserError::Invalid("CDP response has no result".into()));
    }
}

fn browser_executable() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("TEAL_BROWSER_PATH").map(PathBuf::from)
        && path.is_file()
    {
        return Some(path);
    }
    let mut candidates = Vec::new();
    for root in ["PROGRAMFILES(X86)", "PROGRAMFILES", "LOCALAPPDATA"] {
        if let Some(root) = std::env::var_os(root).map(PathBuf::from) {
            candidates.push(root.join("Microsoft/Edge/Application/msedge.exe"));
            candidates.push(root.join("Google/Chrome/Application/chrome.exe"));
        }
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .or_else(|| {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .flat_map(|directory| {
                        [directory.join("msedge.exe"), directory.join("chrome.exe")]
                    })
                    .find(|path| path.is_file())
            })
        })
}

#[cfg(test)]
mod tests {
    use futures_util::{SinkExt as _, StreamExt as _};
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    use super::command;

    async fn fake_cdp(response: serde_json::Value) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let request = socket.next().await.unwrap().unwrap().into_text().unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&request).unwrap()["id"],
                1
            );
            socket
                .send(Message::Text(response.to_string().into()))
                .await
                .unwrap();
        });
        format!("ws://{address}")
    }

    #[tokio::test]
    async fn cdp_command_returns_structured_result_and_rejects_protocol_error() {
        let endpoint = fake_cdp(json!({"id":1,"result":{"nodes":[]}})).await;
        assert_eq!(
            command(&endpoint, "Accessibility.getFullAXTree", json!({}))
                .await
                .unwrap(),
            json!({"nodes":[]})
        );

        let endpoint = fake_cdp(json!({"id":1,"error":{"message":"denied"}})).await;
        assert!(
            command(&endpoint, "DOM.focus", json!({}))
                .await
                .unwrap_err()
                .to_string()
                .contains("CDP command failed")
        );
    }
}
