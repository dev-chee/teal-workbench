@edition("2026");

import "json.tl" as encode;
host "std.json" as json {
    fn has(text: string, pointer: string) -> bool | Error;

    fn read_string(text: string, pointer: string) -> string | Error;

    fn read_number(text: string, pointer: string) -> string | Error;

    fn read_boolean(text: string, pointer: string) -> bool | Error;

    fn items(text: string, pointer: string) -> string[] | Error;

} host "std.http" as transport {
    task send(request: string) -> string | Error;
} host "std.codec" as codec {
    fn url_encode(text: string) -> string | Error;

    fn decode_base64(text: string) -> string | Error;
}
/// 保留顺序和重复名称的 HTTP 字段。
pub class Header { pub name: string, pub value: string }
/// multipart 字段；path 非空时从授权文件读取，否则使用 text。
pub class Part { pub name: string, pub text: string, pub path: string, pub filename: string, pub mime: string }
/// 文本或 base64 编码的二进制请求体。
pub enum Body {
    text { value: string },
    binary { base64: string },
    multipart { parts: Part[] }
}
/// 有界请求；默认不跟随重定向、不自动重试。
pub class Request { pub method: string, pub url: string, pub headers: Header[], pub body: Body, pub timeout_ms: int, pub max_bytes: int }
/// 非成功状态也作为响应返回；二进制内容始终可用。
pub class Response { pub status: int, pub headers: Header[], pub text: string, pub is_utf8: bool, pub base64: string }
/// 编码单个 URL 路径段或查询参数，保留 RFC 3986 非保留字符。
pub fn encode_component(text: string) -> string | Error {
    return codec::url_encode(text) ?;
}

/// 追加查询参数；调用方传入不含 fragment 的 URL。
pub fn query(url: string, parameters: Header[]) -> string | Error {
    let result = url;
    for parameter in parameters {
        result = result + (if result.contains("?") { "&" } else { "?" }) + encode_component(parameter.name) ? +"=" + encode_component(parameter.value) ?;
    }
    return result;
}

/// 创建 30 秒超时、8 MiB 响应上限的空请求。
pub fn request(method: string, url: string) -> Request {
    return Request {
        method: method,
        url: url,
        headers: [],
        body: Body.text { value: "" },
        timeout_ms: 30000,
        max_bytes: 8388608
    };
}

/// 创建文件上传部分。
pub fn file(name: string, path: string, filename: string, mime: string) -> Part {
    return Part { name: name, text: "", path: path, filename: filename, mime: mime };
}

/// 创建 multipart 文本部分。
pub fn field(name: string, text: string) -> Part {
    return Part { name: name, text: text, path: "", filename: "", mime: "" };
}

/// 发送请求；权限、传输、超时或大小限制失败会抛错。
pub task send(request: Request) -> Response | Error {

    let headers: string[] = [];
    for header in request.headers {
        headers.push(encode::object([encode::text("name", header.name) ?, encode::text("value", header.value) ?]) ?);
    }
    let body = "";
    let binary = false;
    let parts: string[] = [];
    match request.body {
        Body.text { value } => {
            body = value;
        }
        Body.binary { base64 } => {
            body = base64;
            binary = true;
        }
        Body.multipart { parts: values } => {
            for part in values {
                parts.push(encode::object([encode::text("name", part.name) ?, encode::text("text", part.text) ?, encode::text("path", part.path) ?, encode::text("filename", part.filename) ?, encode::text("mime", part.mime) ?]) ?);
            }
        }
    }
    let wire = encode::object([encode::text("method", request.method) ?, encode::text("url", request.url) ?, encode::value("headers", encode::array(headers) ?) ?, encode::text("body", body) ?, encode::value("base64", string(binary)) ?, encode::value("parts", encode::array(parts) ?) ?, encode::value("timeout_ms", string(request.timeout_ms)) ?, encode::value("max_bytes", string(request.max_bytes)) ?]) ?;
    let result = transport::send(wire).await ?;
    let response_headers: Header[] = [];
    for item in json::items(result, "/headers") ? {
        response_headers.push(Header { name: json::read_string(item, "/name") ?, value: json::read_string(item, "/value") ? });
    }
    return Response {
        status: int(json::read_number(result, "/status") ?),
        headers: response_headers,
        text: if json::read_boolean(result, "/utf8") ? { codec::decode_base64(json::read_string(result, "/base64") ?) ? } else { "" },
        is_utf8: json::read_boolean(result, "/utf8") ?,
        base64: json::read_string(result, "/base64") ?
    };
}
