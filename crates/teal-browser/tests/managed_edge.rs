use tokio::net::TcpListener;

use teal_browser::BrowserManager;

async fn serve_page(body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let body = body.to_owned();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request).await.unwrap();
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(), body
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            });
        }
    });
    format!("http://{address}/")
}

fn node_id(snapshot: &teal_browser::BrowserSnapshot, role: &str) -> i64 {
    snapshot
        .nodes
        .iter()
        .find(|node| {
            node.pointer("/role/value")
                .and_then(serde_json::Value::as_str)
                == Some(role)
        })
        .and_then(|node| node.get("backendDOMNodeId"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_else(|| panic!("{role} should appear in the accessibility tree"))
}

fn has_name(snapshot: &teal_browser::BrowserSnapshot, name: &str) -> bool {
    snapshot.nodes.iter().any(|node| {
        node.pointer("/name/value")
            .and_then(serde_json::Value::as_str)
            == Some(name)
    })
}

#[tokio::test]
#[ignore = "requires installed Edge or Chrome"]
async fn managed_browser_can_fill_and_click_a_local_page() {
    let manager = BrowserManager::default();
    let session = manager.start(false).await.unwrap();
    let url = serve_page(
        "<!doctype html><html><body><label for='name'>Name</label><input id='name' aria-label='Name'><button id='ok'>OK</button></body></html>",
    )
    .await;
    manager.allow_network(&session, &url).await.unwrap();
    let page = manager.open(&session, &url).await.unwrap();
    let mut input = None;
    let mut roles = Vec::new();
    for _ in 0..20 {
        let snapshot = manager.snapshot(&session, &page.page_id).await.unwrap();
        roles = snapshot
            .nodes
            .iter()
            .filter_map(|node| {
                node.pointer("/role/value")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .collect();
        if let Some(id) = snapshot.nodes.iter().find_map(|node| {
            (node
                .pointer("/role/value")
                .and_then(serde_json::Value::as_str)
                == Some("textbox"))
            .then(|| {
                node.get("backendDOMNodeId")
                    .and_then(serde_json::Value::as_i64)
            })
            .flatten()
        }) {
            input = Some((id, snapshot.page.generation));
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let (input, generation) = input.unwrap_or_else(|| {
        panic!(
            "textbox missing url={} title={} roles={roles:?}",
            page.url, page.title
        )
    });
    manager
        .fill(&session, &page.page_id, input, generation, "value")
        .await
        .unwrap();
    let snapshot = manager.snapshot(&session, &page.page_id).await.unwrap();
    let button = snapshot
        .nodes
        .iter()
        .find_map(|node| {
            (node
                .pointer("/role/value")
                .and_then(serde_json::Value::as_str)
                == Some("button"))
            .then(|| {
                node.get("backendDOMNodeId")
                    .and_then(serde_json::Value::as_i64)
            })
            .flatten()
        })
        .expect("button should appear");
    manager
        .click(&session, &page.page_id, button, snapshot.page.generation)
        .await
        .unwrap();
    manager.close(&session).await.unwrap();
}

#[tokio::test]
#[ignore = "requires installed Edge or Chrome"]
async fn managed_browser_rejects_a_snapshot_after_the_page_changes() {
    let manager = BrowserManager::default();
    let session = manager.start(false).await.unwrap();
    let url = serve_page(
        "<!doctype html><button id='action' onclick=\"document.body.innerHTML='after-change'\">before-change</button><script>setTimeout(()=>{let b=document.getElementById('action');b.textContent='armed';b.onclick=()=>document.body.innerHTML='after-change'},200)</script>",
    )
    .await;
    manager.allow_network(&session, &url).await.unwrap();
    let page = manager.open(&session, &url).await.unwrap();
    let snapshot = manager.snapshot(&session, &page.page_id).await.unwrap();
    let button = node_id(&snapshot, "button");

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let error = manager
        .click(&session, &page.page_id, button, snapshot.page.generation)
        .await
        .expect_err("a changed page must reject the old snapshot before click");

    let after = manager.snapshot(&session, &page.page_id).await.unwrap();
    assert!(
        !has_name(&after, "after-change"),
        "rejected stale action must not trigger the changed page: {error}"
    );
    manager.close(&session).await.unwrap();
}

#[tokio::test]
#[ignore = "requires installed Edge or Chrome"]
async fn managed_browser_allows_at_most_one_concurrent_action_per_generation() {
    let manager = BrowserManager::default();
    let session = manager.start(false).await.unwrap();
    let url = serve_page(
        "<!doctype html><span id='count'>0</span><input aria-label='value' oninput=\"let c=document.getElementById('count');c.textContent=String(Number(c.textContent)+1)\">",
    )
    .await;
    manager.allow_network(&session, &url).await.unwrap();
    let page = manager.open(&session, &url).await.unwrap();
    let snapshot = manager.snapshot(&session, &page.page_id).await.unwrap();
    let textbox = node_id(&snapshot, "textbox");
    let generation = snapshot.page.generation;
    let (first, second) = tokio::join!(
        manager.fill(&session, &page.page_id, textbox, generation, "a"),
        manager.fill(&session, &page.page_id, textbox, generation, "b")
    );

    assert!(
        first.is_ok() ^ second.is_ok(),
        "exactly one action should claim a generation: {first:?}, {second:?}"
    );
    let after = manager.snapshot(&session, &page.page_id).await.unwrap();
    assert!(
        !has_name(&after, "2"),
        "only the successful action may affect the page"
    );
    manager.close(&session).await.unwrap();
}
