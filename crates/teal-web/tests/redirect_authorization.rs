//! 网络重定向必须返回待重新授权的目标，不能在一次请求中透明跟随。

use std::{
    io::{Read as _, Write as _},
    net::TcpListener as StdTcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use teal_web::{TextBrowser, fetch_blocking_with_audit};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[derive(Default)]
struct TestAuditSink(Mutex<u64>);

impl teal_security::AuditSink for TestAuditSink {
    fn append(
        &self,
        _: &teal_security::Event,
    ) -> Result<teal_security::Receipt, teal_security::AuditError> {
        let mut sequence = self.0.lock().unwrap();
        *sequence += 1;
        Ok(teal_security::Receipt {
            sequence: *sequence,
            hash: format!("{:064x}", *sequence),
        })
    }
}

fn audit_context() -> teal_security::AuditContext {
    teal_security::AuditContext::new(Arc::new(TestAuditSink::default()))
}

#[test]
fn blocking_fetch_reports_redirect_without_contacting_the_target() {
    let target = StdTcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let target_address = target.local_addr().unwrap();
    let target_hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&target_hits);
    let target_thread = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            match target.accept() {
                Ok((mut socket, _)) => {
                    counted.fetch_add(1, Ordering::SeqCst);
                    let mut request = [0_u8; 2048];
                    let _ = socket.read(&mut request);
                    let body = b"<html><title>target</title><p>target reached</p></html>";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes());
                    let _ = socket.write_all(body);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("target accept failed: {error}"),
            }
        }
    });

    let source = StdTcpListener::bind("127.0.0.1:0").unwrap();
    let source_address = source.local_addr().unwrap();
    let redirect_target = format!("http://{target_address}/authorized-next");
    let response_target = redirect_target.clone();
    let source_thread = std::thread::spawn(move || {
        let (mut socket, _) = source.accept().unwrap();
        let mut request = [0_u8; 2048];
        let _ = socket.read(&mut request).unwrap();
        write!(
            socket,
            "HTTP/1.1 302 Found\r\nLocation: {response_target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
    });

    let error = fetch_blocking_with_audit(
        &format!("http://{source_address}/initial"),
        &audit_context(),
    )
    .expect_err("redirects require a separately authorized request");
    let message = error.to_string();
    assert!(message.contains(&redirect_target), "{message}");
    source_thread.join().unwrap();
    target_thread.join().unwrap();
    assert_eq!(target_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn text_browser_get_and_head_preserve_history_and_do_not_follow_redirects() {
    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_address = target.local_addr().unwrap();
    let target_hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&target_hits);
    let target_task = tokio::spawn(async move {
        for _ in 0..2 {
            let Ok(Ok((mut socket, _))) =
                tokio::time::timeout(Duration::from_millis(500), target.accept()).await
            else {
                break;
            };
            counted.fetch_add(1, Ordering::SeqCst);
            let mut request = [0_u8; 2048];
            let _ = socket.read(&mut request).await;
            let body = b"<p>target</p>";
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.write_all(body).await.unwrap();
        }
    });

    let source = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let source_address = source.local_addr().unwrap();
    let redirect_target = format!("http://{target_address}/authorized-next");
    let response_target = redirect_target.clone();
    let source_task = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut socket, _) = source.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let read = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            if request.contains(" /ok ") {
                let body = b"<html><title>ok</title><p>stable page</p></html>";
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                socket.write_all(body).await.unwrap();
            } else {
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 302 Found\r\nLocation: {response_target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        }
    });

    let mut browser = TextBrowser::new()
        .unwrap()
        .with_audit_context(audit_context());
    let initial = format!("http://{source_address}/ok");
    browser.open(&initial).await.unwrap();
    assert_eq!(browser.current().unwrap().url.as_str(), initial);

    for method in ["GET", "HEAD"] {
        let error = if method == "GET" {
            browser
                .open(&format!("http://{source_address}/redirect-get"))
                .await
                .expect_err("GET redirect requires separate authorization")
        } else {
            browser
                .head(&format!("http://{source_address}/redirect-head"))
                .await
                .expect_err("HEAD redirect requires separate authorization")
        };
        assert!(error.to_string().contains(&redirect_target), "{error}");
        assert_eq!(
            browser.current().unwrap().url.as_str(),
            initial,
            "redirect failure must not append browser history"
        );
    }

    source_task.await.unwrap();
    target_task.await.unwrap();
    assert_eq!(target_hits.load(Ordering::SeqCst), 0);
}
