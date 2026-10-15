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
import "std/fs.tl" as files;
/// 上传图片、附件或导入素材；parent_type 与 parent_node 使用官方资源类型。
pub task upload(client: api::Client, path: string, filename: string, parent_type: string, parent_node: string, extra: string) -> string | Error {

    let info = files::metadata(path).await ?;
    if !info.size.is_int() {
        return Error::make("feishu", "Feishu single upload exceeds 20 MiB", ErrorOrigin.user, false);
    }
    if int(info.size) > 20971520 {
        return Error::make("feishu", "Feishu single upload exceeds 20 MiB", ErrorOrigin.user, false);
    }
    let parts = [http::field("file_name", filename), http::field("parent_type", parent_type), http::field("parent_node", parent_node), http::field("size", info.size), http::file("file", path, filename, "application/octet-stream")];
    if extra != "" {
        parts.push(http::field("extra", encode::validate(extra) ?));
    }
    return json::read_string(api::call_multipart(client, "/drive/v1/medias/upload_all", parts).await ?, "/file_token") ?;
}

/// 上传内存文本作为导入素材，不写临时文件。
pub task upload_text(client: api::Client, filename: string, text: string) -> string | Error {

    if text.len() > 20971520 {
        return Error::make("feishu", "Feishu single upload exceeds 20 MiB", ErrorOrigin.user, false);
    }
    let parts = [http::field("file_name", filename), http::field("parent_type", "ccm_import_open"), http::field("parent_node", ""), http::field("size", string(text.len())), http::field("extra", encode::object([encode::text("obj_type", "docx") ?, encode::text("file_extension", "md") ?]) ?), http::Part { name: "file", text: text, path: "", filename: filename, mime: "text/markdown" }];
    return json::read_string(api::call_multipart(client, "/drive/v1/medias/upload_all", parts).await ?, "/file_token") ?;
}

/// 创建 Markdown 导入任务，返回 ticket，调用方保存后可恢复查询。
pub task import_markdown(client: api::Client, file_token: string, title: string, folder: string) -> string | Error {

    let point = encode::object([encode::value("mount_type", "1") ?, encode::text("mount_key", folder) ?]) ?;
    let data = api::call(client, "POST", "/drive/v1/import_tasks", encode::object([encode::text("file_extension", "md") ?, encode::text("file_token", file_token) ?, encode::text("type", "docx") ?, encode::text("file_name", title) ?, encode::value("point", point) ?]) ?).await ?;
    return json::read_string(data, "/ticket") ?;
}

/// 查询导入结果，保留 job_status 和服务端错误信息。
pub task import_result(client: api::Client, ticket: string) -> string | Error {

    return api::call(client, "GET", "/drive/v1/import_tasks/" + api::id(ticket) ?, "").await ?;
}

/// 分页列出文件夹内容；返回官方 files、has_more 与 next_page_token。
pub task list(client: api::Client, folder: string, page_token: string) -> string | Error {

    return api::call(client, "GET", http::query("/drive/v1/files", [http::Header { name: "folder_token", value: folder }, http::Header { name: "page_token", value: page_token }, http::Header { name: "page_size", value: "200" }]) ?, "").await ?;
}

/// 创建子文件夹。
pub task create_folder(client: api::Client, parent: string, name: string) -> string | Error {

    return api::call(client, "POST", "/drive/v1/files/create_folder", encode::object([encode::text("folder_token", parent) ?, encode::text("name", name) ?]) ?).await ?;
}

/// 复制文件到文件夹。
pub task copy(client: api::Client, token: string, resource_type: string, folder: string, name: string) -> string | Error {

    return api::call(client, "POST", "/drive/v1/files/" + api::id(token) ? +"/copy", encode::object([encode::text("type", resource_type) ?, encode::text("folder_token", folder) ?, encode::text("name", name) ?]) ?).await ?;
}

/// 移动文件，不改变所有权或协作者设置。
pub task move(client: api::Client, token: string, resource_type: string, folder: string) -> string | Error {

    return api::call(client, "POST", "/drive/v1/files/" + api::id(token) ? +"/move", encode::object([encode::text("type", resource_type) ?, encode::text("folder_token", folder) ?]) ?).await ?;
}

/// 删除指定文件或文件夹，调用方须明确选择目标。
pub task delete(client: api::Client, token: string, resource_type: string) -> string | Error {

    return api::call(client, "DELETE", http::query("/drive/v1/files/" + api::id(token) ?, [http::Header { name: "type", value: resource_type }]) ?, "").await ?;
}

/// 列出协作者。
pub task members(client: api::Client, token: string, resource_type: string) -> string | Error {

    return api::call(client, "GET", http::query("/drive/v1/permissions/" + api::id(token) ? +"/members", [http::Header { name: "type", value: resource_type }]) ?, "").await ?;
}

/// 添加协作者，member_type 和 perm 使用官方枚举值；默认不发送通知。
pub task add_member(client: api::Client, token: string, resource_type: string, member_id: string, member_type: string, perm: string) -> string | Error {

    return api::call(client, "POST", http::query("/drive/v1/permissions/" + api::id(token) ? +"/members", [http::Header { name: "type", value: resource_type }, http::Header { name: "need_notification", value: "false" }]) ?, encode::object([encode::text("member_id", member_id) ?, encode::text("member_type", member_type) ?, encode::text("perm", perm) ?]) ?).await ?;
}

/// 修改一个协作者权限。
pub task update_member(client: api::Client, token: string, resource_type: string, member_id: string, member_type: string, perm: string) -> string | Error {

    return api::call(client, "PUT", http::query("/drive/v1/permissions/" + api::id(token) ? +"/members/" + api::id(member_id) ?, [http::Header { name: "type", value: resource_type }]) ?, encode::object([encode::text("member_type", member_type) ?, encode::text("perm", perm) ?]) ?).await ?;
}

/// 移除一个协作者。
pub task remove_member(client: api::Client, token: string, resource_type: string, member_id: string, member_type: string) -> string | Error {

    return api::call(client, "DELETE", http::query("/drive/v1/permissions/" + api::id(token) ? +"/members/" + api::id(member_id) ?, [http::Header { name: "type", value: resource_type }, http::Header { name: "member_type", value: member_type }]) ?, "").await ?;
}
