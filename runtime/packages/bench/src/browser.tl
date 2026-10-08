@edition("2026");

import "std/json.tl" as json;

host "std.bench.browser" as backend {
    task start(visible: bool) -> {string: string};
    task open(session: string, url: string) -> {string: string};
    task allow_network(session: string, url: string);
    task snapshot(session: string, page: string) -> string;
    task screenshot(session: string, page: string, generation: int) -> {string: string};
    task click(session: string, page: string, backend_node_id: int, generation: int);
    task fill(session: string, page: string, backend_node_id: int, generation: int, text: string);
    task close(session: string);
}

/// 受管浏览器页面及当前节点代次。
pub class BrowserPage {
    session: string,
    page: string,
    url: string,
    title: string,
    generation: int,
}

impl BrowserPage {
    /// 返回页面当前 URL。
    pub fn url() -> string { return self.url; }
    /// 返回页面当前标题。
    pub fn title() -> string { return self.title; }
    /// 返回只适用于当前页面状态的节点代次。
    pub fn generation() -> int { return self.generation; }
}

/// 可访问性快照原始 JSON 与更新后的页面代次。
pub class BrowserSnapshot {
    pub page: BrowserPage,
    pub json: string,
}

fn decode_page(data: {string: string}) -> BrowserPage {
    return BrowserPage {
        session: data["session"],
        page: data["page"],
        url: data["url"],
        title: data["title"],
        generation: int(data["generation"]),
    };
}

/// 启动临时独立 profile；不可附加用户日常浏览器。
pub task start(visible: bool) -> string {
    return backend::start(visible).await["session"];
}

/// 在受管会话中打开 HTTP/HTTPS 页面。
pub task open(session: string, url: string) -> BrowserPage {
    return decode_page(backend::open(session, url).await);
}

/// 经独立网络批准后允许当前会话请求一个精确 URL；需重新打开页面。
pub task allow_network(session: string, url: string) {
    backend::allow_network(session, url).await;
}

/// 返回可访问性节点 JSON 和更新后的页面代次。
pub task snapshot(page: BrowserPage) -> BrowserSnapshot {
    let data = backend::snapshot(page.session, page.page).await;
    return BrowserSnapshot {
        page: BrowserPage {
            session: json::read_string(data, "/page/session"),
            page: json::read_string(data, "/page/page_id"),
            url: json::read_string(data, "/page/url"),
            title: json::read_string(data, "/page/title"),
            generation: int(json::read_number(data, "/page/generation")),
        },
        json: data,
    };
}

/// 按当前快照的 backend DOM node ID 单击；调用后旧 generation 失效。
pub task click(page: BrowserPage, backend_node_id: int) {
    backend::click(page.session, page.page, backend_node_id, page.generation).await;
}

/// 聚焦节点并写入非秘密文本；密码由用户在可见窗口中输入。
pub task fill(page: BrowserPage, backend_node_id: int, text: string) {
    backend::fill(page.session, page.page, backend_node_id, page.generation, text).await;
}

/// 关闭受管进程并删除临时 profile。
pub task close(session: string) { backend::close(session).await; }


/// 捕获当前受管页面视口；必须使用最新 snapshot 返回的页面代次。
pub task screenshot(page: BrowserPage) -> {string: string} {
    return backend::screenshot(page.session, page.page, page.generation).await;
}
