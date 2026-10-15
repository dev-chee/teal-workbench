@edition("2026");

import "client.tl" as api;
import "http/client.tl" as http;
import "http/json.tl" as encode;
host "std.json" as json {
    fn has(text: string, pointer: string) -> bool | Error;

    fn read_string(text: string, pointer: string) -> string | Error;

    fn read_number(text: string, pointer: string) -> string | Error;

    fn read_boolean(text: string, pointer: string) -> bool | Error;

    fn items(text: string, pointer: string) -> string[] | Error;

}
/// 文档标识与标题。
pub class Document { pub id: string, pub title: string, pub revision: int }
/// 分页块结果；每个块保留完整 JSON，避免丢失未知块属性。
pub class Page { pub blocks: string[], pub has_more: bool, pub page_token: string }
fn root(document: string) -> string | Error {
    return "/docx/v1/documents/" + api::id(document) ?;
}

fn block_path(document: string, block_id: string) -> string | Error {
    return root(document) ? +"/blocks/" + api::id(block_id) ?;
}

fn document(data: string) -> Document | Error {
    return Document { id: json::read_string(data, "/document/document_id") ?, title: json::read_string(data, "/document/title") ?, revision: int(json::read_number(data, "/document/revision_id") ?) };
}

/// 创建空文档，folder_token 为空表示使用当前身份的默认位置。
pub task create(client: api::Client, title: string, folder_token: string) -> Document | Error {

    let fields = [encode::text("title", title) ?];
    if folder_token != "" {
        fields.push(encode::text("folder_token", folder_token) ?);
    }
    return document(api::call(client, "POST", "/docx/v1/documents", encode::object(fields) ?).await ?) ?;
}

/// 获取文档元数据。
pub task get(client: api::Client, id: string) -> Document | Error {

    return document(api::call(client, "GET", root(id) ?, "").await ?) ?;
}

/// 获取文档纯文本。
pub task text(client: api::Client, id: string) -> string | Error {

    return json::read_string(api::call(client, "GET", root(id) ? +"/raw_content", "").await ?, "/content") ?;
}

/// 获取一页块；parent 为空时枚举文档全部块，否则枚举直接子块。
pub task blocks(client: api::Client, document: string, parent: string, page_token: string) -> Page | Error {

    let path = if parent == "" { root(document) ? +"/blocks" } else { block_path(document, parent) ? +"/children" };
    let url = http::query(path, [http::Header { name: "page_size", value: "500" }, http::Header { name: "page_token", value: page_token }]) ?;
    let data = api::call(client, "GET", url, "").await ?;
    return Page {
        blocks: json::items(data, "/items") ?,
        has_more: json::read_boolean(data, "/has_more") ?,
        page_token: if json::has(data, "/page_token") ? { json::read_string(data, "/page_token") ? } else { "" }
    };
}

/// 有界获取全部块，拒绝重复分页 token。
pub task all_blocks(client: api::Client, document: string, parent: string) -> string[] | Error {

    let output: string[] = [];
    let tokens: string[] = [];
    let token = "";
    loop {
        let page = blocks(client, document, parent, token).await ?;
        for item in page.blocks {
            output.push(item);
        }
        if !page.has_more {
            break;
        }
        if page.page_token == "" || tokens.contains(page.page_token) || tokens.len() >= 1000 {
            return Error::make("feishu", "Invalid or excessive Feishu pagination", ErrorOrigin. host, false);
        }
        token = page.page_token;
        tokens.push(token);
    }
    return output;
}

/// 创建子块；index 为 -1 时追加，children 是官方块 JSON。
pub task append(client: api::Client, document: string, parent: string, children: string[], index: int) -> string | Error {

    return api::call(client, "POST", block_path(document, parent) ? +"/children", encode::object([encode::value("children", encode::array(children) ?) ?, encode::value("index", string(index)) ?]) ?).await ?;
}

/// 更新一个块，payload 使用官方 PATCH 请求结构。
pub task update(client: api::Client, document: string, block_id: string, payload: string) -> string | Error {

    return api::call(client, "PATCH", block_path(document, block_id) ?, encode::validate(payload) ?).await ?;
}

/// 删除父块内左闭右开的子块区间。
pub task delete_children(client: api::Client, document: string, parent: string, start: int, end: int) -> string | Error {

    if start < 0 || end <= start {
        return Error::make("feishu", "Invalid block deletion range", ErrorOrigin.user, false);
    }
    return api::call(client, "DELETE", block_path(document, parent) ? +"/children/batch_delete", encode::object([encode::value("start_index", string(start)) ?, encode::value("end_index", string(end)) ?]) ?).await ?;
}

/// 构造普通文本块，支持安全的任意 Unicode 文本。
pub fn paragraph(text: string) -> string | Error {
    let run = encode::object([encode::value("text_run", encode::object([encode::text("content", text) ?]) ?) ?]) ?;
    return encode::object([encode::value("block_type", "2") ?, encode::value("text", encode::object([encode::value("elements", encode::array([run]) ?) ?]) ?) ?]) ?;
}
