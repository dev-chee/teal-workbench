@edition("2026");

import "client.tl" as api;
import "http/client.tl" as http;
import "http/json.tl" as encode;
import "std/json.tl" as json;
import "std/fs.tl" as files;
import "std/error.tl" as errors;
/// 上传图片、附件或导入素材；parent_type 与 parent_node 使用官方资源类型。
pub task upload(client: api::Client, path: string, filename: string, parent_type: string, parent_node: string, extra: string) -> Result<string, errors::Error> {
    try {
        let info = files::metadata_or_throw(path).await;
        if !info.size.is_int() {
            throw(errors::make("feishu", "Feishu single upload exceeds 20 MiB", errors::ErrorOrigin.user, false));
        }
        if int(info.size) > 20971520 {
            throw(errors::make("feishu", "Feishu single upload exceeds 20 MiB", errors::ErrorOrigin.user, false));
        }
        let parts = [http::field("file_name", filename), http::field("parent_type", parent_type), http::field("parent_node", parent_node), http::field("size", info.size), http::file("file", path, filename, "application/octet-stream")];
        if extra != "" {
            parts.push(http::field("extra", encode::validate(extra)));
        }
        return Result::ok(json::read_string(api::call_multipart_or_throw(client, "/drive/v1/medias/upload_all", parts).await, "/file_token"));
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "drive upload did not produce a result", errors::ErrorOrigin.host, true));
}
pub task upload_or_throw(client: api::Client, path: string, filename: string, parent_type: string, parent_node: string, extra: string) -> string {
    match upload(client, path, filename, parent_type, parent_node, extra).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 上传内存文本作为导入素材，不写临时文件。
pub task upload_text(client: api::Client, filename: string, text: string) -> Result<string, errors::Error> {
    try {
        if text.len() > 20971520 {
            throw(errors::make("feishu", "Feishu single upload exceeds 20 MiB", errors::ErrorOrigin.user, false));
        }
        let parts = [http::field("file_name", filename), http::field("parent_type", "ccm_import_open"), http::field("parent_node", ""), http::field("size", string(text.len())), http::field("extra", encode::object([encode::text("obj_type", "docx"), encode::text("file_extension", "md")])), http::Part { name: "file", text: text, path: "", filename: filename, mime: "text/markdown" }];
        return Result::ok(json::read_string(api::call_multipart_or_throw(client, "/drive/v1/medias/upload_all", parts).await, "/file_token"));
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "drive text upload did not produce a result", errors::ErrorOrigin.host, true));
}
pub task upload_text_or_throw(client: api::Client, filename: string, text: string) -> string {
    match upload_text(client, filename, text).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 创建 Markdown 导入任务，返回 ticket，调用方保存后可恢复查询。
pub task import_markdown(client: api::Client, file_token: string, title: string, folder: string) -> Result<string, errors::Error> {
    try {
        let point = encode::object([encode::value("mount_type", "1"), encode::text("mount_key", folder)]);
        let data = api::call_or_throw(client, "POST", "/drive/v1/import_tasks", encode::object([encode::text("file_extension", "md"), encode::text("file_token", file_token), encode::text("type", "docx"), encode::text("file_name", title), encode::value("point", point)])).await;
        return Result::ok(json::read_string(data, "/ticket"));
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "drive import did not produce a result", errors::ErrorOrigin.host, true));
}
pub task import_markdown_or_throw(client: api::Client, file_token: string, title: string, folder: string) -> string {
    match import_markdown(client, file_token, title, folder).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 查询导入结果，保留 job_status 和服务端错误信息。
pub task import_result(client: api::Client, ticket: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "GET", "/drive/v1/import_tasks/" + api::id(ticket), "").await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "drive import status did not produce a result", errors::ErrorOrigin.host, true));
}
pub task import_result_or_throw(client: api::Client, ticket: string) -> string {
    match import_result(client, ticket).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 分页列出文件夹内容；返回官方 files、has_more 与 next_page_token。
pub task list(client: api::Client, folder: string, page_token: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "GET", http::query("/drive/v1/files", [http::Header { name: "folder_token", value: folder }, http::Header { name: "page_token", value: page_token }, http::Header { name: "page_size", value: "200" }]), "").await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "drive list did not produce a result", errors::ErrorOrigin.host, true));
}
pub task list_or_throw(client: api::Client, folder: string, page_token: string) -> string {
    match list(client, folder, page_token).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 创建子文件夹。
pub task create_folder(client: api::Client, parent: string, name: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "POST", "/drive/v1/files/create_folder", encode::object([encode::text("folder_token", parent), encode::text("name", name)])).await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "drive folder creation did not produce a result", errors::ErrorOrigin.host, true));
}
pub task create_folder_or_throw(client: api::Client, parent: string, name: string) -> string {
    match create_folder(client, parent, name).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 复制文件到文件夹。
pub task copy(client: api::Client, token: string, resource_type: string, folder: string, name: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "POST", "/drive/v1/files/" + api::id(token) + "/copy", encode::object([encode::text("type", resource_type), encode::text("folder_token", folder), encode::text("name", name)])).await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "drive copy did not produce a result", errors::ErrorOrigin.host, true));
}
pub task copy_or_throw(client: api::Client, token: string, resource_type: string, folder: string, name: string) -> string {
    match copy(client, token, resource_type, folder, name).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 移动文件，不改变所有权或协作者设置。
pub task move(client: api::Client, token: string, resource_type: string, folder: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "POST", "/drive/v1/files/" + api::id(token) + "/move", encode::object([encode::text("type", resource_type), encode::text("folder_token", folder)])).await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "drive move did not produce a result", errors::ErrorOrigin.host, true));
}
pub task move_or_throw(client: api::Client, token: string, resource_type: string, folder: string) -> string {
    match move(client, token, resource_type, folder).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 删除指定文件或文件夹，调用方须明确选择目标。
pub task delete(client: api::Client, token: string, resource_type: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "DELETE", http::query("/drive/v1/files/" + api::id(token), [http::Header { name: "type", value: resource_type }]), "").await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "drive delete did not produce a result", errors::ErrorOrigin.host, true));
}
pub task delete_or_throw(client: api::Client, token: string, resource_type: string) -> string {
    match delete(client, token, resource_type).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 列出协作者。
pub task members(client: api::Client, token: string, resource_type: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "GET", http::query("/drive/v1/permissions/" + api::id(token) + "/members", [http::Header { name: "type", value: resource_type }]), "").await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "drive member listing did not produce a result", errors::ErrorOrigin.host, true));
}
pub task members_or_throw(client: api::Client, token: string, resource_type: string) -> string {
    match members(client, token, resource_type).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 添加协作者，member_type 和 perm 使用官方枚举值；默认不发送通知。
pub task add_member(client: api::Client, token: string, resource_type: string, member_id: string, member_type: string, perm: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "POST", http::query("/drive/v1/permissions/" + api::id(token) + "/members", [http::Header { name: "type", value: resource_type }, http::Header { name: "need_notification", value: "false" }]), encode::object([encode::text("member_id", member_id), encode::text("member_type", member_type), encode::text("perm", perm)])).await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "drive member addition did not produce a result", errors::ErrorOrigin.host, true));
}
pub task add_member_or_throw(client: api::Client, token: string, resource_type: string, member_id: string, member_type: string, perm: string) -> string {
    match add_member(client, token, resource_type, member_id, member_type, perm).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 修改一个协作者权限。
pub task update_member(client: api::Client, token: string, resource_type: string, member_id: string, member_type: string, perm: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "PUT", http::query("/drive/v1/permissions/" + api::id(token) + "/members/" + api::id(member_id), [http::Header { name: "type", value: resource_type }]), encode::object([encode::text("member_type", member_type), encode::text("perm", perm)])).await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "drive member update did not produce a result", errors::ErrorOrigin.host, true));
}
pub task update_member_or_throw(client: api::Client, token: string, resource_type: string, member_id: string, member_type: string, perm: string) -> string {
    match update_member(client, token, resource_type, member_id, member_type, perm).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
/// 移除一个协作者。
pub task remove_member(client: api::Client, token: string, resource_type: string, member_id: string, member_type: string) -> Result<string, errors::Error> {
    try {
        return Result::ok(api::call_or_throw(client, "DELETE", http::query("/drive/v1/permissions/" + api::id(token) + "/members/" + api::id(member_id), [http::Header { name: "type", value: resource_type }, http::Header { name: "member_type", value: member_type }]), "").await);
    } catch error {
        return Result::err(errors::make("feishu", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu", "drive member removal did not produce a result", errors::ErrorOrigin.host, true));
}
pub task remove_member_or_throw(client: api::Client, token: string, resource_type: string, member_id: string, member_type: string) -> string {
    match remove_member(client, token, resource_type, member_id, member_type).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
