@edition("2026");

import "client.tl" as api;
import "docs.tl" as docs;
import "drive.tl" as drive;
import "http/json.tl" as encode;
host "std.json" as json {
    fn has(text: string, pointer: string) -> bool | Error;

    fn read_string(text: string, pointer: string) -> string | Error;

    fn read_number(text: string, pointer: string) -> string | Error;

    fn read_boolean(text: string, pointer: string) -> bool | Error;

    fn items(text: string, pointer: string) -> string[] | Error;

} host "std.markdown" as parser {
    fn local_images(text: string, base: string) -> string | Error;
} host "std.oauth" as random {
    fn random() -> string | Error;
} host "std.http" as clock {
    task delay(milliseconds: int) -> none | Error;
}
/// 图片关联的可恢复状态；清单保持原文顺序，marker 是导入后识别的唯一文本。
pub class Image { pub path: string, pub filename: string, pub marker: string }
/// 已提交导入任务；保存 ticket 后可重复查询，不重复创建文档。
pub class Import { pub ticket: string, pub images: Image[] }
/// 已创建的文档、待关联图片与服务端警告；部分失败时调用方仍持有文档标识。
pub class Imported { pub document: string, pub url: string, pub images: Image[], pub warnings: string[] }
/// 提交新文档导入，将本地图片替换为唯一独立段落；不上传远程图片。
pub task start(client: api::Client, markdown: string, base_directory: string, title: string, folder: string) -> Import | Error {

    let references = json::items(parser::local_images(markdown, base_directory) ?, "") ?;
    let images: Image[] = [];
    let content = "";
    let offset = 0;
    let prefix = random::random() ?;
    for reference in references {
        let start = int(json::read_number(reference, "/start") ?);
        let end = int(json::read_number(reference, "/end") ?);
        if start < offset || end < start || end > markdown.len() {
            return Error::make("feishu_markdown", "Invalid Markdown image source range", ErrorOrigin.user, false);
        }
        let marker = "TEALIMAGE" + prefix + string(images.len());
        content = content + markdown[offset..start] + "\n\n" + marker + "\n\n";
        images.push(Image { path: json::read_string(reference, "/path") ?, filename: json::read_string(reference, "/filename") ?, marker: marker });
        offset = end;
    }
    content = content + markdown[offset..];
    let source = drive::upload_text(client, "document.md", content).await ?;
    let ticket = drive::import_markdown(client, source, title, folder).await ?;
    return Import { ticket: ticket, images: images };
}

/// 在有界轮询内等待导入，不重复上传或创建；超时后可用同一 Import 再查询。
pub task wait(client: api::Client, pending: Import, attempts: int) -> Imported | Error {

    if attempts < 1 || attempts > 300 {
        return Error::make("feishu_markdown", "Import polling attempts must be between 1 and 300", ErrorOrigin.user, false);
    }
    for attempt in range(0, attempts) {
        let result = drive::import_result(client, pending.ticket).await ?;
        let status = int(json::read_number(result, "/result/job_status") ?);
        if status == 0 {
            return Imported {
                document: json::read_string(result, "/result/token") ?,
                url: json::read_string(result, "/result/url") ?,
                images: pending.images,
                warnings: if json::has(result, "/result/extra") ? { json::items(result, "/result/extra") ? } else { [] }
            };
        }
        if status != 1 && status != 2 {
            return Error::make("feishu_markdown", "Feishu import failed; inspect import_result using the saved ticket", ErrorOrigin. host, false);
        }
        if attempt + 1 < attempts {
            clock::delay(1000).await ?;
        }
    }
    return Error::make("feishu_markdown", "Feishu import is still pending; retain its ticket", ErrorOrigin. host, true);
}

/// 为一个标记段落创建图片；返回新块 ID，再单独删除标记，保留部分完成证据。
pub task attach_image(client: api::Client, imported: Imported, image: Image) -> string | Error {

    let blocks = docs::all_blocks(client, imported.document, "").await ?;
    let marker_block = "";
    let parent = "";
    for block in blocks {
        if json::read_number(block, "/block_type") ? == "2" {
            let content = "";
            for element in json::items(block, "/text/elements") ? {
                if json::has(element, "/text_run/content") ? {
                    content = content + json::read_string(element, "/text_run/content") ?;
                }
            }
            if content == image.marker {
                if marker_block != "" {
                    return Error::make("feishu_markdown", "Duplicate Markdown image marker", ErrorOrigin.user, false);
                }
                marker_block = json::read_string(block, "/block_id") ?;
                parent = json::read_string(block, "/parent_id") ?;
            }
        }
    }
    if marker_block == "" {
        return Error::make("feishu_markdown", "Markdown image marker not found; inspect imported document", ErrorOrigin. host, false);
    }
    let siblings = docs::all_blocks(client, imported.document, parent).await ?;
    let index = -1;
    for i in range(0, siblings.len()) {
        if json::read_string(siblings[i], "/block_id") ? == marker_block {
            index = i;
        }
    }
    if index < 0 {
        return Error::make("feishu_markdown", "Markdown image parent changed", ErrorOrigin. host, false);
    }
    let created = docs::append(client, imported.document, parent, [encode::object([encode::value("block_type", "27") ?, encode::value("image", "{{}}") ?]) ?], index).await ?;
    return json::read_string(created, "/children/0/block_id") ?;
}

/// 上传并绑定图片，完成后才删除原标记；调用方保存 image_block 以便定位部分失败。
pub task fill_image(client: api::Client, imported: Imported, image: Image, image_block: string) -> none | Error {

    let token = drive::upload(client, image.path, image.filename, "docx_image", image_block, "").await ?;
    docs::update(client, imported.document, image_block, encode::object([encode::value("replace_image", encode::object([encode::text("token", token) ?]) ?) ?]) ?).await ?;
    let blocks = docs::all_blocks(client, imported.document, "").await ?;
    for block in blocks {
        if json::read_number(block, "/block_type") ? == "2" {
            let content = "";
            for element in json::items(block, "/text/elements") ? {
                if json::has(element, "/text_run/content") ? {
                    content = content + json::read_string(element, "/text_run/content") ?;
                }
            }
            if content == image.marker {
                let parent = json::read_string(block, "/parent_id") ?;
                let id = json::read_string(block, "/block_id") ?;
                let siblings = docs::all_blocks(client, imported.document, parent).await ?;
                for i in range(0, siblings.len()) {
                    if json::read_string(siblings[i], "/block_id") ? == id {
                        docs::delete_children(client, imported.document, parent, i, i + 1).await ?;
                        return none;
                    }
                }
            }
        }
    }
    return Error::make("feishu_markdown", "Image uploaded but Markdown marker changed; inspect document", ErrorOrigin. host, false);
}
