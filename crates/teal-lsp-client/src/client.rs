//! 标准输入输出 JSON-RPC 客户端及请求等待者生命周期。
//!
//! 写入端串行发送完整消息，读取任务分发响应与服务器事件。请求须先登记再发送，
//! 取消、超时或传输退出都必须结束对应等待者，避免悬挂请求。

use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{Mutex, broadcast, oneshot},
};

use crate::{LspReader, write_message};

type PendingResponse = oneshot::Sender<Result<Value, LspError>>;

type PendingMap = Arc<Mutex<HashMap<i64, PendingResponse>>>;

/// JSON-RPC 请求标识；服务器请求允许使用数字或字符串标识。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum JsonRpcId {
    /// JSON 数字标识。
    Number(i64),
    /// JSON 字符串标识。
    String(String),
}

impl JsonRpcId {
    fn from_value(value: &Value) -> Option<Self> {
        value
            .as_i64()
            .map(Self::Number)
            .or_else(|| value.as_str().map(|value| Self::String(value.to_owned())))
    }

    fn to_value(&self) -> Value {
        match self {
            Self::Number(value) => Value::from(*value),
            Self::String(value) => Value::from(value.clone()),
        }
    }
}

/// 一个服务器的可执行文件、环境、根目录和初始化负载。
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// 由操作系统解析的服务器可执行文件或命令。
    pub program: String,

    /// 命令行参数。
    pub args: Vec<String>,

    /// 工作区根目录。
    pub root: PathBuf,

    /// 服务器专用初始化选项。
    pub initialization_options: Value,

    /// 额外的子进程环境值。
    pub environment: HashMap<String, String>,
}

impl ServerConfig {
    /// 标准 rust-analyzer stdio 配置。
    #[must_use]
    pub fn rust_analyzer(root: PathBuf) -> Self {
        Self {
            program: "rust-analyzer".to_owned(),
            args: Vec::new(),
            root,
            initialization_options: Value::Null,
            environment: HashMap::new(),
        }
    }

    /// 标准 Teal LSP stdio 配置。
    #[must_use]
    pub fn teal(root: PathBuf) -> Self {
        Self {
            program: "teal-lsp".to_owned(),
            args: Vec::new(),
            root,
            initialization_options: Value::Null,
            environment: HashMap::new(),
        }
    }
}

/// 服务器通知、stderr 行或进程终止。
#[derive(Clone, Debug)]
pub enum LspEvent {
    /// JSON-RPC 通知。
    Notification {
        /// JSON-RPC 方法。
        method: String,

        /// 通知参数。
        params: Value,
    },

    /// 需要响应的服务器到客户端请求。
    Request {
        /// JSON-RPC 请求标识。
        id: JsonRpcId,

        /// JSON-RPC 方法。
        method: String,

        /// 请求参数。
        params: Value,
    },

    /// 一行诊断 stderr。
    Stderr(String),

    /// stdout 已关闭，服务器不再可用。
    ServerExited,
}

/// 进程、帧、超时或协议错误。
#[derive(Debug, Error)]
pub enum LspError {
    /// 子进程或管道访问失败。
    #[error("LSP read and write failure: {0}")]
    Io(#[from] std::io::Error),

    /// 请求超过截止时间。
    #[error("LSP request timeout")]
    Timeout,

    /// 服务器返回 JSON-RPC 错误对象。
    #[error("LSP server error: {0}")]
    Server(String),

    /// 服务器退出导致响应通道关闭。
    #[error("LSP server exits before replying")]
    Exited,

    /// 无法构造工作区 URI。
    #[error("Invalid workspace URI")]
    InvalidRoot,

    /// 调用方取消了已过期请求。
    #[error("LSP request canceled")]
    Cancelled,
}

/// 支持取消与事件广播的并发 stdio JSON-RPC 客户端。
pub struct LspClient {
    writer: Arc<Mutex<ChildStdin>>,
    child: Arc<Mutex<Child>>,
    pending: PendingMap,
    next_id: AtomicI64,
    events: broadcast::Sender<LspEvent>,
    timeout: Duration,
    capabilities: Value,
}

impl LspClient {
    /// 启动服务器和管道读取器，并执行 `initialize`/`initialized`。
    #[allow(
        clippy::single_match_else,
        reason = "The read loop handles a success structure and shares a closed path for stream end and error"
    )]
    pub async fn spawn(config: ServerConfig) -> Result<Self, LspError> {
        let mut command = Command::new(&config.program);
        command
            .args(&config.args)
            .current_dir(&config.root)
            .envs(&config.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("Missing LSP stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("Missing LSP stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("Missing LSP standard error"))?;
        let pending = Arc::new(Mutex::new(HashMap::<
            i64,
            oneshot::Sender<Result<Value, LspError>>,
        >::new()));
        let (events, _) = broadcast::channel(512);

        let reader_pending = Arc::clone(&pending);
        let reader_events = events.clone();
        tokio::spawn(async move {
            let mut reader = LspReader::new(stdout);
            loop {
                match reader.read_message().await {
                    Ok(Some(message)) => {
                        if let Some(id) = message.get("id").and_then(JsonRpcId::from_value)
                            && (message.get("result").is_some() || message.get("error").is_some())
                        {
                            if let JsonRpcId::Number(id) = id
                                && let Some(sender) = reader_pending.lock().await.remove(&id)
                            {
                                let result = if let Some(error) = message.get("error") {
                                    Err(LspError::Server(error.to_string()))
                                } else {
                                    Ok(message.get("result").cloned().unwrap_or(Value::Null))
                                };
                                let _ = sender.send(result);
                            }
                        } else if let Some(method) = message.get("method").and_then(Value::as_str) {
                            let params = message.get("params").cloned().unwrap_or(Value::Null);
                            if let Some(id) = message.get("id").and_then(JsonRpcId::from_value) {
                                let _ = reader_events.send(LspEvent::Request {
                                    id,
                                    method: method.to_owned(),
                                    params,
                                });
                            } else {
                                let _ = reader_events.send(LspEvent::Notification {
                                    method: method.to_owned(),
                                    params,
                                });
                            }
                        }
                    }
                    Ok(None) | Err(_) => {
                        // 共享传输不可用时，唤醒全部请求等待者。
                        for (_, sender) in reader_pending.lock().await.drain() {
                            let _ = sender.send(Err(LspError::Exited));
                        }
                        let _ = reader_events.send(LspEvent::ServerExited);
                        break;
                    }
                }
            }
        });

        let stderr_events = events.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = stderr_events.send(LspEvent::Stderr(line));
            }
        });

        let mut client = Self {
            writer: Arc::new(Mutex::new(stdin)),
            child: Arc::new(Mutex::new(child)),
            pending,
            next_id: AtomicI64::new(1),
            events,
            timeout: Duration::from_secs(30),
            capabilities: Value::Null,
        };
        let root_uri = url::Url::from_directory_path(&config.root)
            .map_err(|()| LspError::InvalidRoot)?
            .to_string();

        // initialize 等待响应前，读取循环必须已经启动。
        let initialized = client
            .request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "rootUri": root_uri,
                    "workspaceFolders": [{"uri": root_uri, "name": config.root.file_name().and_then(|name| name.to_str()).unwrap_or("workspace")}],
                    "initializationOptions": config.initialization_options,
                    "capabilities": client_capabilities(),
                    "clientInfo": {"name":"teal-bench","version":env!("CARGO_PKG_VERSION")}
                }),
            )
            .await?;
        client.capabilities = initialized
            .get("capabilities")
            .cloned()
            .unwrap_or(Value::Null);
        client.notify("initialized", json!({})).await?;
        Ok(client)
    }

    /// 返回协商得到的服务器能力。
    pub fn capabilities(&self) -> &Value {
        &self.capabilities
    }

    /// 更改默认请求超时。
    pub const fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// 订阅服务器通知和生命周期事件。
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<LspEvent> {
        self.events.subscribe()
    }

    /// 发送一个请求并等待匹配响应。
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, LspError> {
        let (_cancel, receiver) = oneshot::channel();
        self.request_cancellable(method, params, receiver).await
    }

    /// 发送可取消请求，上下文保留到匹配响应、取消、超时或传输失败。不记录方法参数或响应内容。
    #[tracing::instrument(name = "teal.lsp.client.request", skip_all, fields(request.id = tracing::field::Empty, operation.result = tracing::field::Empty, otel.status_code = tracing::field::Empty))]
    pub async fn request_cancellable(
        &self,
        method: &str,
        params: Value,
        cancel: oneshot::Receiver<()>,
    ) -> Result<Value, LspError> {
        let result = self.request_cancellable_inner(method, params, cancel).await;
        tracing::Span::current().record(
            "operation.result",
            if result.is_ok() { "ok" } else { "error" },
        );
        if result.is_err() {
            tracing::Span::current().record("otel.status_code", "ERROR");
            tracing::warn!(event.name = "failed");
        } else {
            tracing::info!(event.name = "completed");
        }
        result
    }

    /// 发送可取消请求，取消或超时后移除待处理项。
    ///
    /// 取消通知按尽力方式发送，失败不覆盖原本的取消或超时错误。
    /// 本地移除等待者只能丢弃迟到响应，不能证明服务器没有执行该请求。
    async fn request_cancellable_inner(
        &self,
        method: &str,
        params: Value,
        mut cancel: oneshot::Receiver<()>,
    ) -> Result<Value, LspError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        tracing::Span::current().record("request.id", id);

        // 发送前先登记等待者，避免服务器立即响应时无法匹配。
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        if let Err(error) = write_message(
            &mut *self.writer.lock().await,
            &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
        )
        .await
        {
            self.pending.lock().await.remove(&id);
            return Err(error.into());
        }

        let result = tokio::select! {
            result=tokio::time::timeout(self.timeout,receiver)=>match result{Ok(Ok(result))=>result,Ok(Err(_))=>Err(LspError::Exited),Err(_)=>Err(LspError::Timeout)},
            _=&mut cancel=>Err(LspError::Cancelled),
        };

        if matches!(result, Err(LspError::Cancelled | LspError::Timeout)) {
            self.pending.lock().await.remove(&id);
            let _ = self.notify("$/cancelRequest", json!({"id":id})).await;
        }

        result
    }

    /// 发送一条不带响应 ID 的通知。
    pub async fn notify(&self, method: &str, params: Value) -> Result<(), LspError> {
        write_message(
            &mut *self.writer.lock().await,
            &json!({"jsonrpc":"2.0","method":method,"params":params}),
        )
        .await?;
        Ok(())
    }

    /// 成功回复一个服务器到客户端请求。
    pub async fn respond(&self, id: JsonRpcId, result: Value) -> Result<(), LspError> {
        write_message(
            &mut *self.writer.lock().await,
            &json!({"jsonrpc":"2.0","id":id.to_value(),"result":result}),
        )
        .await?;
        Ok(())
    }

    /// 以一个 JSON-RPC 错误回复服务器到客户端请求。
    pub async fn respond_error(
        &self,
        id: JsonRpcId,
        code: i32,
        message: &str,
    ) -> Result<(), LspError> {
        write_message(
            &mut *self.writer.lock().await,
            &json!({"jsonrpc":"2.0","id":id.to_value(),"error":{"code":code,"message":message}}),
        )
        .await?;
        Ok(())
    }

    /// 发布新打开的 UTF-8 文档。
    pub async fn did_open(
        &self,
        uri: &str,
        language_id: &str,
        version: i32,
        text: &str,
    ) -> Result<(), LspError> {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":language_id,"version":version,"text":text}}),
        )
        .await
    }

    /// 发布增量或完整文档更改。
    pub async fn did_change(
        &self,
        uri: &str,
        version: i32,
        changes: Value,
    ) -> Result<(), LspError> {
        self.notify(
            "textDocument/didChange",
            json!({"textDocument":{"uri":uri,"version":version},"contentChanges":changes}),
        )
        .await
    }

    /// 发布已关闭文档。
    pub async fn did_close(&self, uri: &str) -> Result<(), LspError> {
        self.notify("textDocument/didClose", json!({"textDocument":{"uri":uri}}))
            .await
    }

    /// 优雅地请求关闭，并在需要时终止子进程。
    pub async fn shutdown(&self) -> Result<(), LspError> {
        let _ = self.request("shutdown", Value::Null).await;
        let _ = self.notify("exit", Value::Null).await;
        let mut child = self.child.lock().await;
        if child.try_wait()?.is_none() {
            child.kill().await?;
        }
        Ok(())
    }
}

fn client_capabilities() -> Value {
    json!({
        "workspace": {
            "applyEdit": true,
            "workspaceEdit": {"documentChanges": true, "resourceOperations":["create","rename","delete"]},
            "didChangeConfiguration": {"dynamicRegistration": true},
            "symbol": {"dynamicRegistration": true},
            "configuration": true,
            "workspaceFolders": true
        },
        "textDocument": {
            "synchronization": {"dynamicRegistration": true, "willSave": true, "didSave": true},
            "completion": {"dynamicRegistration":true,"completionItem":{"snippetSupport":true,"resolveSupport":{"properties":["documentation","detail","additionalTextEdits"]}}},
            "hover": {"dynamicRegistration":true,"contentFormat":["markdown","plaintext"]},
            "signatureHelp": {"dynamicRegistration":true},
            "definition": {"dynamicRegistration":true,"linkSupport":true},
            "references": {"dynamicRegistration":true},
            "documentSymbol": {"dynamicRegistration":true,"hierarchicalDocumentSymbolSupport":true},
            "rename": {"dynamicRegistration":true,"prepareSupport":true},
            "codeAction": {"dynamicRegistration":true,"dataSupport":true,"resolveSupport":{"properties":["edit","command"]}},
            "formatting": {"dynamicRegistration":true},
            "rangeFormatting": {"dynamicRegistration":true},
            "inlayHint": {"dynamicRegistration":true,"resolveSupport":{"properties":["tooltip","textEdits","label.tooltip","label.location","label.command"]}},
            "semanticTokens": {"dynamicRegistration":true,"requests":{"range":true,"full":{"delta":true}},"tokenTypes":[],"tokenModifiers":[],"formats":["relative"]},
            "diagnostic": {"dynamicRegistration":true,"relatedDocumentSupport":true}
        },
        "window": {"workDoneProgress": true, "showMessage": {"messageActionItem":{"additionalPropertiesSupport":true}}},
        "general": {"positionEncodings":["utf-16"]}
    })
}

#[cfg(test)]
mod tests {
    use super::JsonRpcId;
    use serde_json::json;

    #[test]
    fn preserves_numeric_and_string_request_ids() {
        assert_eq!(JsonRpcId::from_value(&json!(7)), Some(JsonRpcId::Number(7)));
        assert_eq!(
            JsonRpcId::from_value(&json!("edit-7")),
            Some(JsonRpcId::String("edit-7".into()))
        );
        assert_eq!(JsonRpcId::from_value(&json!(null)), None);
    }

    #[test]
    fn serializes_string_request_ids_without_conversion() {
        assert_eq!(
            JsonRpcId::String("edit-7".into()).to_value(),
            json!("edit-7")
        );
    }
}
