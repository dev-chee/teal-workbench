//! 使用确定性子服务器验证真实 stdio 协议的回归测试。
#[path = "../../teal-bench/tests/support/lsp.rs"]
mod fixture;
use teal_lsp_client::{LspClient, LspError, LspManager, TextDocumentChange};
use serde_json::json;
use std::sync::Arc;
#[tokio::test]
async fn real_stdio_negotiates_synchronizes_cancels_and_closes() {
    let directory = tempfile::tempdir().unwrap();
    let client = Arc::new(
        LspClient::spawn(fixture::config(directory.path()))
            .await
            .unwrap(),
    );
    assert_eq!(client.capabilities()["textDocumentSync"], 2);
    let mut manager = LspManager::default();
    let server = manager.insert("rust".into(), client.clone());
    let uri = "file:///mock.rs";
    server.open(uri, 0, "a😀b").await.unwrap();
    server.open(uri, 0, "a😀b").await.unwrap();
    let change = TextDocumentChange::between("a😀b", "a\u{754c}b", 0, 1);
    server.change(uri, &change).await.unwrap();
    server.change(uri, &change).await.unwrap();
    let result = client
        .request("textDocument/completion", json!({}))
        .await
        .unwrap();
    assert_eq!(result[0]["insertTextFormat"], 2);
    let (cancel, cancelled) = tokio::sync::oneshot::channel();
    let slow = client.request_cancellable("mock/slow", json!({}), cancelled);
    let request = async {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        cancel.send(()).unwrap();
    };
    let (result, ()) = tokio::join!(slow, request);
    assert!(matches!(result, Err(LspError::Cancelled)));
    server.close(uri).await.unwrap();
    client.shutdown().await.unwrap();
    let log = fixture::log(directory.path());
    let count = |name| log.iter().filter(|v| v["method"] == name).count();
    assert_eq!(count("textDocument/didOpen"), 1);
    assert_eq!(count("textDocument/didChange"), 1);
    assert_eq!(count("textDocument/didClose"), 1);
    assert_eq!(count("$/cancelRequest"), 1);
    let change = log
        .iter()
        .find(|v| v["method"] == "textDocument/didChange")
        .unwrap();
    assert_eq!(
        change["params"]["contentChanges"][0]["range"]["end"]["character"],
        3
    );
    assert_eq!(change["params"]["contentChanges"][0]["text"], "\u{754c}");
}
