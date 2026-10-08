@edition("2026");

import "client.tl" as api;
import "http/client.tl" as http;
import "http/json.tl" as encode;
import "std/json.tl" as json;
import "std/error.tl" as errors;
/// 文档标识与标题。
pub class Document { pub id: string, pub title: string, pub revision: int }
/// 分页块结果；每个块保留完整 JSON，避免丢失未知块属性。
pub class Page { pub blocks: string[], pub has_more: bool, pub page_token: string }
fn root(document: string) -> string {
    return "/docx/v1/documents/" + api::id(document);
}
fn block_path(document: string, block_id: string) -> string {
    return root(document) + "/blocks/" + api::id(block_id);
}
fn document(data: string) -> Document {
    return Document { id: json::read_string(data, "/document/document_id"), title: json::read_string(data, "/document/title"), revision: int(json::read_number(data, "/document/revision_id")) };
}
/// 创建空文档，folder_token 为空表示使用当前身份的默认位置。
pub task create(client: api::Client, title: string, folder_token: string) -> Result<Document, errors::Error> {
    try {
        let fields = [encode::text("title", title)];
        if folder_token != "" {
            fields.push(encode::text("folder_token", folder_token));
        }
        return Result::ok(document(api::call_or_throw(client, "POST", "/docx/v1/documents", encode::object(fields)).await));
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "document create did not produce a result", errors::ErrorOrigin.host, true));
}
pub task create_or_throw(client: api::Client, title: string, folder_token: string) -> Document {
    match create(client, title, folder_token).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 获取文档元数据。
pub task get(client: api::Client, id: string) -> Result<Document, errors::Error> {
    try {
        return Result::ok(document(api::call_or_throw(client, "GET", root(id), "").await));
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "document get did not produce a result", errors::ErrorOrigin.host, true));
}
pub task get_or_throw(client: api::Client, id: string) -> Document {
    match get(client, id).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 获取文档纯文本。
pub task text(client: api::Client, id: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(json::read_string(api::call_or_throw(client, "GET", root(id) + "/raw_content", "").await, "/content"));
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "document text did not produce a result", errors::ErrorOrigin.host, true));
}
pub task text_or_throw(client: api::Client, id: string) -> string {
    match text(client, id).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 获取一页块；parent 为空时枚举文档全部块，否则枚举直接子块。
pub task blocks(client: api::Client, document: string, parent: string, page_token: string) -> Result<Page, errors::Error> {
    try {
        let path = if parent == "" { root(document) + "/blocks" } else { block_path(document, parent) + "/children" };
        let url = http::query(path, [http::Header { name: "page_size", value: "500" }, http::Header { name: "page_token", value: page_token }]);
        let data = api::call_or_throw(client, "GET", url, "").await;
        return Result::ok(Page {
            blocks: json::items(data, "/items"),
            has_more: json::read_boolean(data, "/has_more"),
            page_token: if json::has(data, "/page_token") { json::read_string(data, "/page_token") } else { "" }
        });
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "document blocks did not produce a result", errors::ErrorOrigin.host, true));
}
pub task blocks_or_throw(client: api::Client, document: string, parent: string, page_token: string) -> Page {
    match blocks(client, document, parent, page_token).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 有界获取全部块，拒绝重复分页 token。
pub task all_blocks(client: api::Client, document: string, parent: string) -> Result<string[], errors::Error> {
    try {
        let output: string[] = [];
        let tokens: string[] = [];
        let token = "";
        loop {
            let page = blocks(client, document, parent, token).await?;
            for item in page.blocks {
                output.push(item);
            }
            if !page.has_more {
                break;
            }
            if page.page_token == "" || tokens.contains(page.page_token) || tokens.len() >= 1000 {
                throw(errors::make("feishu", "Invalid or excessive Feishu pagination", errors::ErrorOrigin.host, false));
            }
            token = page.page_token;
            tokens.push(token);
        }
        return Result::ok(output);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "document pagination did not produce a result", errors::ErrorOrigin.host, true));
}
pub task all_blocks_or_throw(client: api::Client, document: string, parent: string) -> string[] {
    match all_blocks(client, document, parent).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 创建子块；index 为 -1 时追加，children 是官方块 JSON。
pub task append(client: api::Client, document: string, parent: string, children: string[], index: int) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "POST", block_path(document, parent) + "/children", encode::object([encode::value("children", encode::array(children)), encode::value("index", string(index))])).await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "document append did not produce a result", errors::ErrorOrigin.host, true));
}
pub task append_or_throw(client: api::Client, document: string, parent: string, children: string[], index: int) -> string {
    match append(client, document, parent, children, index).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 更新一个块，payload 使用官方 PATCH 请求结构。
pub task update(client: api::Client, document: string, block_id: string, payload: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "PATCH", block_path(document, block_id), encode::validate(payload)).await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "document update did not produce a result", errors::ErrorOrigin.host, true));
}
pub task update_or_throw(client: api::Client, document: string, block_id: string, payload: string) -> string {
    match update(client, document, block_id, payload).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 删除父块内左闭右开的子块区间。
pub task delete_children(client: api::Client, document: string, parent: string, start: int, end: int) -> Result<string, errors::Error> {
    try {
        if start < 0 || end <= start {
            throw(errors::make("feishu", "Invalid block deletion range", errors::ErrorOrigin.user, false));
        }
        return Result::ok(api::call_or_throw(client, "DELETE", block_path(document, parent) + "/children/batch_delete", encode::object([encode::value("start_index", string(start)), encode::value("end_index", string(end))])).await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, false));
    }
    return Result::err(errors::make("feishu", "document delete did not produce a result", errors::ErrorOrigin.host, false));
}
pub task delete_children_or_throw(client: api::Client, document: string, parent: string, start: int, end: int) -> string {
    match delete_children(client, document, parent, start, end).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 构造普通文本块，支持安全的任意 Unicode 文本。
pub fn paragraph(text: string) -> string {
    let run = encode::object([encode::value("text_run", encode::object([encode::text("content", text)]))]);
    return encode::object([encode::value("block_type", "2"), encode::value("text", encode::object([encode::value("elements", encode::array([run]))]))]);
}
