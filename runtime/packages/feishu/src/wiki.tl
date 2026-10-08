@edition("2026");

import "client.tl" as api;
import "http/client.tl" as http;
import "http/json.tl" as encode;
import "std/error.tl" as errors;
/// 分页列出当前身份可访问的知识空间。
pub task spaces(client: api::Client, page_token: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "GET", http::query("/wiki/v2/spaces", [http::Header { name: "page_token", value: page_token }, http::Header { name: "page_size", value: "50" }]), "").await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "wiki space listing did not produce a result", errors::ErrorOrigin.host, true));
}
pub task spaces_or_throw(client: api::Client, page_token: string) -> string {
    match spaces(client, page_token).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 分页列出节点；parent 为空表示空间根。
pub task nodes(client: api::Client, space: string, parent: string, page_token: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "GET", http::query("/wiki/v2/spaces/" + api::id(space) + "/nodes", [http::Header { name: "parent_node_token", value: parent }, http::Header { name: "page_token", value: page_token }, http::Header { name: "page_size", value: "50" }]), "").await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "wiki node listing did not produce a result", errors::ErrorOrigin.host, true));
}
pub task nodes_or_throw(client: api::Client, space: string, parent: string, page_token: string) -> string {
    match nodes(client, space, parent, page_token).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 将知识库 token 解析为底层文档 obj_token。
pub task get(client: api::Client, token: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "GET", http::query("/wiki/v2/spaces/get_node", [http::Header { name: "token", value: token }]), "").await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "wiki node lookup did not produce a result", errors::ErrorOrigin.host, true));
}
pub task get_or_throw(client: api::Client, token: string) -> string {
    match get(client, token).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 创建新的 Docx 知识库节点。
pub task create(client: api::Client, space: string, parent: string, title: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "POST", "/wiki/v2/spaces/" + api::id(space) + "/nodes", encode::object([encode::text("obj_type", "docx"), encode::text("node_type", "origin"), encode::text("parent_node_token", parent), encode::text("title", title)])).await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "wiki node creation did not produce a result", errors::ErrorOrigin.host, true));
}
pub task create_or_throw(client: api::Client, space: string, parent: string, title: string) -> string {
    match create(client, space, parent, title).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 修改节点标题。
pub task title(client: api::Client, space: string, node: string, title: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "POST", "/wiki/v2/spaces/" + api::id(space) + "/nodes/" + api::id(node) + "/update_title", encode::object([encode::text("title", title)])).await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "wiki title update did not produce a result", errors::ErrorOrigin.host, true));
}
pub task title_or_throw(client: api::Client, space: string, node: string, new_title: string) -> string {
    match title(client, space, node, new_title).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 移动现有节点到指定空间和父节点。
pub task move(client: api::Client, space: string, node: string, target_space: string, parent: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "POST", "/wiki/v2/spaces/" + api::id(space) + "/nodes/" + api::id(node) + "/move", encode::object([encode::text("target_space_id", target_space), encode::text("target_parent_token", parent)])).await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "wiki node move did not produce a result", errors::ErrorOrigin.host, true));
}
pub task move_or_throw(client: api::Client, space: string, node: string, target_space: string, parent: string) -> string {
    match move(client, space, node, target_space, parent).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 将已导入的 Docx 移入知识库；返回结果或异步任务标识。
pub task move_document(client: api::Client, space: string, parent: string, document: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "POST", "/wiki/v2/spaces/" + api::id(space) + "/nodes/move_docs_to_wiki", encode::object([encode::text("parent_wiki_token", parent), encode::text("obj_type", "docx"), encode::text("obj_token", document)])).await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "wiki document move did not produce a result", errors::ErrorOrigin.host, true));
}
pub task move_document_or_throw(client: api::Client, space: string, parent: string, document: string) -> string {
    match move_document(client, space, parent, document).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
