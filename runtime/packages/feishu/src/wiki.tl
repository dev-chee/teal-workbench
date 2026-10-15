@edition("2026");

import "client.tl" as api;
import "http/client.tl" as http;
import "http/json.tl" as encode;
/// 分页列出当前身份可访问的知识空间。
pub task spaces(client: api::Client, page_token: string) -> string | Error {

    return api::call(client, "GET", http::query("/wiki/v2/spaces", [http::Header { name: "page_token", value: page_token }, http::Header { name: "page_size", value: "50" }]) ?, "").await ?;
}

/// 分页列出节点；parent 为空表示空间根。
pub task nodes(client: api::Client, space: string, parent: string, page_token: string) -> string | Error {

    return api::call(client, "GET", http::query("/wiki/v2/spaces/" + api::id(space) ? +"/nodes", [http::Header { name: "parent_node_token", value: parent }, http::Header { name: "page_token", value: page_token }, http::Header { name: "page_size", value: "50" }]) ?, "").await ?;
}

/// 将知识库 token 解析为底层文档 obj_token。
pub task get(client: api::Client, token: string) -> string | Error {

    return api::call(client, "GET", http::query("/wiki/v2/spaces/get_node", [http::Header { name: "token", value: token }]) ?, "").await ?;
}

/// 创建新的 Docx 知识库节点。
pub task create(client: api::Client, space: string, parent: string, title: string) -> string | Error {

    return api::call(client, "POST", "/wiki/v2/spaces/" + api::id(space) ? +"/nodes", encode::object([encode::text("obj_type", "docx") ?, encode::text("node_type", "origin") ?, encode::text("parent_node_token", parent) ?, encode::text("title", title) ?]) ?).await ?;
}

/// 修改节点标题。
pub task title(client: api::Client, space: string, node: string, title: string) -> string | Error {

    return api::call(client, "POST", "/wiki/v2/spaces/" + api::id(space) ? +"/nodes/" + api::id(node) ? +"/update_title", encode::object([encode::text("title", title) ?]) ?).await ?;
}

/// 移动现有节点到指定空间和父节点。
pub task move(client: api::Client, space: string, node: string, target_space: string, parent: string) -> string | Error {

    return api::call(client, "POST", "/wiki/v2/spaces/" + api::id(space) ? +"/nodes/" + api::id(node) ? +"/move", encode::object([encode::text("target_space_id", target_space) ?, encode::text("target_parent_token", parent) ?]) ?).await ?;
}

/// 将已导入的 Docx 移入知识库；返回结果或异步任务标识。
pub task move_document(client: api::Client, space: string, parent: string, document: string) -> string | Error {

    return api::call(client, "POST", "/wiki/v2/spaces/" + api::id(space) ? +"/nodes/move_docs_to_wiki", encode::object([encode::text("parent_wiki_token", parent) ?, encode::text("obj_type", "docx") ?, encode::text("obj_token", document) ?]) ?).await ?;
}
