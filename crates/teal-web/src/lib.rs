//! Teal 的结构化文本 Web 客户端。
//!
//! 请求和历史导航返回结构化文本，不执行 JavaScript 或维护 Cookie。
//! 成功解码页面后才提交历史变化，离线会话快照的大小约束由会话模块处理。

use std::{collections::HashSet, io::Read as _, process::Command};

use encoding_rs::{Encoding, UTF_8};
use reqwest::{
    Client, Method, StatusCode,
    header::{HeaderMap, LOCATION},
};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;
mod session;
pub use session::BrowserSession;

const DEFAULT_MAX_BYTES: usize = 5 * 1024 * 1024;

/// 在阻塞工作线程中获取并解析一份 HTTP/HTTPS 文本文档。
///
/// 该入口使用与 [`TextBrowser`] 相同的显式重定向、压缩、字符集和解码后大小边界，但不保存
/// 浏览历史。调用方必须确保它不在异步执行器工作线程上直接阻塞。
///
/// # Errors
/// URL、请求、正文解码或大小检查失败时返回错误。
pub fn fetch_blocking(input: &str) -> Result<WebDocument, WebError> {
    fetch_blocking_with_audit(input, &teal_security::process_context())
}

/// 使用宿主显式提供的接收器阻塞获取文本文档。
///
/// # Errors
/// URL、审计、请求、正文解码或大小检查失败时返回错误。
pub fn fetch_blocking_with_audit(
    input: &str,
    context: &teal_security::AuditContext,
) -> Result<WebDocument, WebError> {
    let url = Url::parse(input)?;
    validate_web_url(&url)?;
    let audit = begin_web(context, teal_security::Action::Browser)?;
    let result = fetch_blocking_url(url);
    audit
        .finish(if result.is_ok() {
            teal_security::Outcome::Succeeded
        } else {
            teal_security::Outcome::Failed
        })
        .map_err(WebError::AuditUnknown)?;
    result
}

fn fetch_blocking_url(url: Url) -> Result<WebDocument, WebError> {
    let response = reqwest::blocking::Client::builder()
        .user_agent(concat!("Teal/", env!("CARGO_PKG_VERSION"), " text-browser"))
        .redirect(reqwest::redirect::Policy::none())
        .build()?
        .get(url)
        .send()?;
    if let Some(target) = redirect_target(response.status(), response.url(), response.headers())? {
        return Err(WebError::Redirect { target });
    }
    if response
        .content_length()
        .is_some_and(|length| length > DEFAULT_MAX_BYTES as u64)
    {
        return Err(WebError::TooLarge {
            limit: DEFAULT_MAX_BYTES,
        });
    }
    let status = response.status().as_u16();
    let final_url = response.url().clone();
    let encoding = response_encoding(response.headers());
    let mut bytes = Vec::new();
    response
        .take((DEFAULT_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(WebError::Body)?;
    if bytes.len() > DEFAULT_MAX_BYTES {
        return Err(WebError::TooLarge {
            limit: DEFAULT_MAX_BYTES,
        });
    }
    let html = encoding.decode(&bytes).0;
    Ok(WebDocument::from_html(final_url, status, &html))
}

fn response_encoding(headers: &reqwest::header::HeaderMap) -> &'static Encoding {
    headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value.split(';').skip(1).find_map(|parameter| {
                let (name, value) = parameter.trim().split_once('=')?;
                name.eq_ignore_ascii_case("charset")
                    .then(|| value.trim().trim_matches('"'))
            })
        })
        .and_then(|label| Encoding::for_label(label.as_bytes()))
        .unwrap_or(UTF_8)
}

fn redirect_target(
    status: StatusCode,
    base: &Url,
    headers: &HeaderMap,
) -> Result<Option<Url>, WebError> {
    if !status.is_redirection() {
        return Ok(None);
    }
    let Some(location) = headers.get(LOCATION) else {
        return Ok(None);
    };
    let location = location.to_str().map_err(|_| WebError::InvalidRedirect)?;
    let target = base.join(location).map_err(|_| WebError::InvalidRedirect)?;
    if validate_web_url(&target).is_err() || target.as_str().len() > 16_384 {
        return Err(WebError::InvalidRedirect);
    }
    Ok(Some(target))
}

/// 从 HTML 提取的一个带编号链接。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebLink {
    /// 从一开始的提示编号。
    pub hint: usize,

    /// 可见的规范化标签。
    pub label: String,

    /// 绝对目标 URL。
    pub url: Url,
}

/// 结构化文本网页文档。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebDocument {
    /// 重定向后的最终 URL。
    pub url: Url,

    /// HTML 标题或作为后备的 URL。
    pub title: String,

    /// HTTP 响应状态。
    pub status: u16,

    /// 已渲染文本行。
    pub lines: Vec<String>,

    /// 按文档顺序排列的已提取链接。
    pub links: Vec<WebLink>,
}

impl WebDocument {
    /// 将 HTML 解析为标题、段落、列表、表格、引用、代码和链接。
    #[must_use]
    pub fn from_html(url: Url, status: u16, html: &str) -> Self {
        let document = Html::parse_document(html);
        let title_selector = Selector::parse("title").expect("static selector");
        let title = document
            .select(&title_selector)
            .next()
            .map(normalized_text)
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| url.as_str().to_owned());
        let block_selector =
            Selector::parse("h1,h2,h3,h4,h5,h6,p,li,blockquote,pre,tr").expect("static selector");
        let anchor_selector = Selector::parse("a[href]").expect("static selector");
        let mut lines = Vec::new();
        let mut anchors = Vec::new();
        let mut seen_links = HashSet::new();
        for element in document.select(&block_selector) {
            let tag = element.value().name();
            let text = if tag == "pre" {
                element.text().collect::<String>().trim_end().to_owned()
            } else {
                normalized_text(element)
            };
            if !text.is_empty() {
                match tag {
                    "h1" => lines.push(format!("# {text}")),
                    "h2" => lines.push(format!("## {text}")),
                    "h3" | "h4" | "h5" | "h6" => lines.push(format!("### {text}")),
                    "li" => lines.push(format!("• {text}")),
                    "blockquote" => lines.push(format!("> {text}")),
                    "pre" => lines.extend(text.lines().map(|line| format!("    {line}"))),
                    "tr" => lines.push(format!(
                        "| {} |",
                        text.split_whitespace().collect::<Vec<_>>().join(" | ")
                    )),
                    _ => lines.push(text),
                }
            }
            for anchor in element.select(&anchor_selector) {
                let Some(href) = anchor.value().attr("href") else {
                    continue;
                };
                let Ok(target) = url.join(href) else {
                    continue;
                };
                if validate_web_url(&target).is_err() {
                    continue;
                }
                let label = normalized_text(anchor);
                let key = (label.clone(), target.clone());
                if seen_links.insert(key) {
                    anchors.push(WebLink {
                        hint: anchors.len() + 1,
                        label,
                        url: target,
                    });
                }
            }
        }
        if lines.is_empty() {
            let text = normalized_text(document.root_element());
            lines.extend(text.lines().map(ToOwned::to_owned));
        }
        Self {
            url,
            title,
            status,
            lines,
            links: anchors,
        }
    }

    /// 返回文内查询的不区分大小写行/字节匹配。
    #[must_use]
    pub fn find(&self, query: &str) -> Vec<(usize, usize)> {
        let query = query.to_lowercase();
        self.lines
            .iter()
            .enumerate()
            .filter_map(|(line, text)| {
                text.to_lowercase()
                    .find(&query)
                    .map(|column| (line, column))
            })
            .collect()
    }
}

/// 获取、URL、响应大小或外部打开失败。
#[derive(Debug, Error)]
pub enum WebError {
    /// 必需的持久审计写入失败。
    #[error("Web audit failed: {0}")]
    Audit(#[from] teal_security::AuditError),
    /// 外部请求可能已发送，但审计结果无法确认。
    #[error("Web audit result unavailable; effect may be unknown: {0}")]
    AuditUnknown(teal_security::AuditError),
    /// 新目标须由调用方作为独立网络请求重新授权。
    #[error("Redirect requires a new authorized request: {target}")]
    Redirect {
        /// 从 Location 解析的规范 HTTP/HTTPS 目标。
        target: Url,
    },
    /// Location 无法解析为无凭据的 HTTP/HTTPS 目标。
    #[error("Redirect target is not a valid HTTP/HTTPS URL without credentials")]
    InvalidRedirect,
    /// 外部浏览器只能接收无凭据的 HTTP/HTTPS URL。
    #[error("External URL must use HTTP or HTTPS without credentials")]
    InvalidExternalUrl,
    /// 无法解析 URL。
    #[error("Invalid URL: {0}")]
    Url(#[from] url::ParseError),

    /// HTTP 请求或正文解码失败。
    #[error("Web page request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// 阻塞响应正文读取失败。
    #[error("Web page body read failed: {0}")]
    Body(std::io::Error),

    /// 响应超过配置的限制。
    #[error("Response exceeds text browser's {limit} byte limit")]
    TooLarge {
        /// 配置的最大响应大小。
        limit: usize,
    },

    /// 启动系统浏览器失败。
    #[error("Unable to open system browser: {0}")]
    External(#[from] std::io::Error),
}

/// HTTP/HTTPS 历史与结构化文本导航状态。
pub struct TextBrowser {
    audit: teal_security::AuditContext,
    client: Client,
    history: Vec<WebDocument>,
    index: Option<usize>,
    max_bytes: usize,
}

impl TextBrowser {
    /// 创建显式报告重定向、支持压缩和字符集解码但无 Cookie 存储的浏览器。
    pub fn new() -> Result<Self, WebError> {
        Ok(Self {
            audit: teal_security::process_context(),
            client: Client::builder()
                .user_agent(concat!("Teal/", env!("CARGO_PKG_VERSION"), " text-browser"))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            history: Vec::new(),
            index: None,
            max_bytes: DEFAULT_MAX_BYTES,
        })
    }

    /// 为外部网页请求指定宿主提供的持久审计接收器。
    #[must_use]
    pub fn with_audit_context(mut self, audit: teal_security::AuditContext) -> Self {
        self.audit = audit;
        self
    }

    /// 设置响应字节限制。
    pub const fn set_max_bytes(&mut self, max_bytes: usize) {
        self.max_bytes = max_bytes;
    }

    /// 返回当前文档。
    #[must_use]
    pub fn current(&self) -> Option<&WebDocument> {
        self.index.and_then(|index| self.history.get(index))
    }

    /// 使用 GET 获取 HTTP/HTTPS URL，并在成功解析后追加历史。
    ///
    /// 请求、解码或大小检查失败时保留当前页面及前进历史。返回值借用浏览器内部的新页面；
    /// 只有成功导航才裁剪当前位置之后的历史，HTTP 状态码保留在文档中供调用方展示。
    pub async fn open(&mut self, input: &str) -> Result<&WebDocument, WebError> {
        self.request(Method::GET, input).await
    }

    /// 使用 HEAD 获取响应头并记录紧凑文档。
    pub async fn head(&mut self, input: &str) -> Result<&WebDocument, WebError> {
        self.request(Method::HEAD, input).await
    }

    /// 不发出网络请求地后退。
    pub fn back(&mut self) -> Option<&WebDocument> {
        let index = self.index?;
        self.index = Some(index.saturating_sub(1));
        self.current()
    }

    /// 不发出网络请求地前进。
    pub fn forward(&mut self) -> Option<&WebDocument> {
        let index = self.index?;
        self.index = Some((index + 1).min(self.history.len().saturating_sub(1)));
        self.current()
    }

    #[tracing::instrument(name = "teal.web.browser", skip_all, fields(operation.result = tracing::field::Empty, otel.status_code = tracing::field::Empty))]
    async fn request(&mut self, method: Method, input: &str) -> Result<&WebDocument, WebError> {
        let audit = begin_web(&self.audit, teal_security::Action::Browser)?;
        let result = self.request_inner(method, input).await;
        audit
            .finish(if result.is_ok() {
                teal_security::Outcome::Succeeded
            } else {
                teal_security::Outcome::Failed
            })
            .map_err(WebError::AuditUnknown)?;
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

    async fn request_inner(
        &mut self,
        method: Method,
        input: &str,
    ) -> Result<&WebDocument, WebError> {
        let url = Url::parse(input)?;
        validate_web_url(&url)?;

        let mut response = self.client.request(method.clone(), url).send().await?;
        if let Some(target) =
            redirect_target(response.status(), response.url(), response.headers())?
        {
            return Err(WebError::Redirect { target });
        }
        if response
            .content_length()
            .is_some_and(|length| length > u64::try_from(self.max_bytes).unwrap_or(u64::MAX))
        {
            return Err(WebError::TooLarge {
                limit: self.max_bytes,
            });
        }

        let status = response.status().as_u16();
        let final_url = response.url().clone();
        let encoding = response_encoding(response.headers());
        let document = if method == Method::HEAD {
            WebDocument {
                url: final_url.clone(),
                title: final_url.as_str().to_owned(),
                status,
                lines: response
                    .headers()
                    .iter()
                    .map(|(name, value)| {
                        format!("{name}: {}", value.to_str().unwrap_or("<binary>"))
                    })
                    .collect(),
                links: Vec::new(),
            }
        } else {
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if bytes.len().saturating_add(chunk.len()) > self.max_bytes {
                    return Err(WebError::TooLarge {
                        limit: self.max_bytes,
                    });
                }
                bytes.extend_from_slice(&chunk);
            }
            let html = encoding.decode(&bytes).0;
            WebDocument::from_html(final_url, status, &html)
        };

        // 获取失败时保留当前页面和前进历史。
        if let Some(index) = self.index {
            self.history.truncate(index + 1);
        }
        self.history.push(document);
        self.index = Some(self.history.len() - 1);
        Ok(self
            .current()
            .expect("The history contains newly added documents"))
    }

    /// 不使用 shell 命令行，在操作系统浏览器中打开当前 URL。
    pub fn open_external(&self) -> Result<(), WebError> {
        let Some(document) = self.current() else {
            return Ok(());
        };
        open_url_external_audited(&document.url, &self.audit)
    }

    /// 不使用 shell 命令行，在操作系统浏览器中打开一个显式 URL。
    pub fn open_url_external(url: &Url) -> Result<(), WebError> {
        open_url_external_audited(url, &teal_security::process_context())
    }

    /// 使用宿主显式提供的接收器启动系统浏览器。
    ///
    /// # Errors
    /// 审计写入或平台启动失败时返回错误。
    pub fn open_url_external_with_audit(
        url: &Url,
        audit: &teal_security::AuditContext,
    ) -> Result<(), WebError> {
        open_url_external_audited(url, audit)
    }
}

fn open_url_external_audited(
    url: &Url,
    context: &teal_security::AuditContext,
) -> Result<(), WebError> {
    validate_web_url(url).map_err(|_| WebError::InvalidExternalUrl)?;
    let audit = begin_web(context, teal_security::Action::Browser)?;
    let result = open_url_external_inner(url);
    audit
        .finish(if result.is_ok() {
            teal_security::Outcome::Succeeded
        } else {
            teal_security::Outcome::Failed
        })
        .map_err(WebError::AuditUnknown)?;
    result
}

fn open_url_external_inner(url: &Url) -> Result<(), WebError> {
    #[cfg(windows)]
    let status = Command::new("rundll32.exe")
        .arg("url.dll,FileProtocolHandler")
        .arg(url.as_str())
        .status()?;
    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg(url.as_str()).status()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open").arg(url.as_str()).status()?;
    if !status.success() {
        return Err(WebError::External(std::io::Error::other(
            "System browser failed to return",
        )));
    }
    Ok(())
}

fn validate_web_url(url: &Url) -> Result<(), WebError> {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(WebError::InvalidExternalUrl);
    }
    Ok(())
}

fn begin_web(
    context: &teal_security::AuditContext,
    action: teal_security::Action,
) -> Result<teal_security::AuditAttempt, WebError> {
    let event = teal_security::Event::new(
        action,
        teal_security::Phase::Intent,
        teal_security::Outcome::Pending,
        teal_security::new_correlation_id(),
    )?
    .with_actor(teal_security::Actor::Host);
    context.begin(event).map_err(WebError::Audit)
}

fn normalized_text(element: scraper::ElementRef<'_>) -> String {
    element
        .text()
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener as StdTcpListener,
        sync::{Arc, Mutex},
    };

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    use url::Url;

    use super::{
        DEFAULT_MAX_BYTES, TextBrowser, WebDocument, fetch_blocking, fetch_blocking_with_audit,
        validate_web_url,
    };

    #[derive(Default)]
    struct TestAuditSink(Mutex<Vec<teal_security::Event>>);

    impl teal_security::AuditSink for TestAuditSink {
        fn append(
            &self,
            event: &teal_security::Event,
        ) -> Result<teal_security::Receipt, teal_security::AuditError> {
            let mut events = self.0.lock().unwrap();
            events.push(event.clone());
            Ok(teal_security::Receipt {
                sequence: events.len() as u64,
                hash: format!("{:064x}", events.len()),
            })
        }
    }

    fn audit_context() -> teal_security::AuditContext {
        teal_security::AuditContext::new(Arc::new(TestAuditSink::default()))
    }

    fn blocking_fixture(response: Vec<u8>) -> String {
        let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 2048];
            let _ = socket.read(&mut request).unwrap();
            socket.write_all(&response).unwrap();
        });
        format!("http://{address}/fixture")
    }

    #[test]
    fn blocking_fetch_uses_local_http_and_rejects_protocol_and_size_boundaries() {
        let body =
            b"<html><head><title>Fixture</title></head><body><p>local text</p></body></html>";
        let url = blocking_fixture(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes()
            .into_iter()
            .chain(body.iter().copied())
            .collect(),
        );
        let audit = audit_context();
        let document = fetch_blocking_with_audit(&url, &audit).unwrap();
        assert_eq!(document.title, "Fixture");
        assert!(document.lines.iter().any(|line| line == "local text"));
        assert!(fetch_blocking("file:///workspace/page.html").is_err());

        let oversized = blocking_fixture(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n",
                DEFAULT_MAX_BYTES + 1
            )
            .into_bytes(),
        );
        assert!(matches!(
            fetch_blocking_with_audit(&oversized, &audit),
            Err(super::WebError::TooLarge { limit }) if limit == DEFAULT_MAX_BYTES
        ));
    }

    #[test]
    fn blocking_fetch_stops_unbounded_bodies_at_the_hard_limit() {
        let mut response =
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n".to_vec();
        response.extend(std::iter::repeat_n(b'x', DEFAULT_MAX_BYTES + 1));
        let url = blocking_fixture(response);
        let audit = audit_context();
        assert!(
            matches!(fetch_blocking_with_audit(&url, &audit), Err(super::WebError::TooLarge { limit }) if limit == DEFAULT_MAX_BYTES)
        );
    }

    #[tokio::test]
    async fn missing_audit_sink_rejects_requests_before_network_io() {
        assert!(
            fetch_blocking("http://127.0.0.1:9/")
                .unwrap_err()
                .to_string()
                .contains("audit sink is required")
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut browser = TextBrowser::new().unwrap();
        let error = browser
            .open(&format!("http://{address}/must-not-connect"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("audit sink is required"));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
    }

    #[test]
    fn converts_html_structure_links_and_code_without_script_content() {
        let document = WebDocument::from_html(
            Url::parse("https://example.test/docs/").unwrap(),
            200,
            r#"<html><head><title>Docs</title><script>bad()</script></head><body><h1>Guide</h1><p>Hello <a href="next">next</a> <a href="file:///tmp/secret">secret</a> <a href="https://user:pass@example.test/private">private</a>.</p><pre>fn main() {}</pre><ul><li>One</li></ul></body></html>"#,
        );
        assert_eq!(document.title, "Docs");
        assert!(document.lines.iter().any(|line| line == "# Guide"));
        assert!(document.lines.iter().any(|line| line.contains("fn main")));
        assert!(!document.lines.iter().any(|line| line.contains("bad()")));
        assert_eq!(
            document.links[0].url.as_str(),
            "https://example.test/docs/next"
        );
        assert_eq!(document.links.len(), 1);
    }

    #[test]
    fn external_urls_require_http_or_https_without_credentials() {
        assert!(validate_web_url(&Url::parse("https://example.test/path").unwrap()).is_ok());
        assert!(validate_web_url(&Url::parse("file:///tmp/secret").unwrap()).is_err());
        assert!(validate_web_url(&Url::parse("mailto:user@example.test").unwrap()).is_err());
        assert!(
            validate_web_url(&Url::parse("https://user:pass@example.test/private").unwrap())
                .is_err()
        );
    }

    #[tokio::test]
    async fn redirects_require_explicit_navigation_before_legacy_charset_decoding() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind server");
        let address = listener.local_addr().expect("Server address");
        tokio::spawn(async move {
            for request_index in 0..2 {
                let (mut socket, _) = listener.accept().await.expect("accept request");
                let mut request = vec![0; 2048];
                let _ = socket.read(&mut request).await.expect("read request");
                if request_index == 0 {
                    socket
                        .write_all(
                            b"HTTP/1.1 302 Found\r\nLocation: /page\r\nContent-Length: 0\r\n\r\n",
                        )
                        .await
                        .expect("write redirect");
                } else {
                    let body = b"<html><head><title>Caf\xe9</title></head><body><p>caf\xe9</p></body></html>";
                    socket
                        .write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=windows-1252\r\nContent-Length: {}\r\n\r\n",
                                body.len()
                            )
                            .as_bytes(),
                        )
                        .await
                        .expect("Write response header");
                    socket.write_all(body).await.expect("Write response body");
                }
            }
        });
        let mut browser = TextBrowser::new()
            .expect("browser")
            .with_audit_context(audit_context());
        let target = format!("http://{address}/page");
        let error = browser
            .open(&format!("http://{address}/redirect"))
            .await
            .expect_err("redirect target requires a new authorized navigation");
        assert!(error.to_string().contains(&target), "{error}");
        assert!(browser.current().is_none());
        let document = browser
            .open(&target)
            .await
            .expect("explicitly opened target page");
        assert!(document.url.path().ends_with("/page"));
        assert_eq!(document.title, "Café");
        assert!(document.lines.iter().any(|line| line == "café"));
    }
}
