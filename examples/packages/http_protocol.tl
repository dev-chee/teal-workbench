@edition("2026");

// Tier Host 包示例：URL 编码、HTTP 请求构造与 JSON 编码面，不发送任何请求。
// network.http 与具体网络资源只由真实的 http::send 调用单独授权并获得。
import "http/client.tl" as http;
import "http/json.tl" as encode;

task main() {
    let url = http::query("https://example.com/search", [
        http::Header { name: "q", value: "teal lang" },
        http::Header { name: "page", value: "2" },
    ]);
    print(url);

    let request = http::request("POST", url);
    request.headers = [http::Header { name: "Content-Type", value: "application/json" }];
    request.body = http::Body.text { value: encode::object([encode::text("name", "teal")]) };
    print(request.method, request.headers.len(), request.timeout_ms, request.max_bytes);

    let payload = encode::object([
        encode::text("name", "teal"),
        encode::value("parts", encode::array([encode::quote("one"), encode::quote("two")])),
    ]);
    print(payload);
    print(encode::get(payload, "/parts/1"), http::encode_component("a b/c"));

    let upload = http::request("POST", url);
    upload.body = http::Body.multipart { parts: [http::field("note", "hello")] };
    match upload.body {
        http::Body.text { value } => {
            print("text", value);
        }
        http::Body.binary { base64 } => {
            print("binary", base64);
        }
        http::Body.multipart { parts } => {
            print("multipart", parts.len());
        }
    }
}
