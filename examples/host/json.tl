@edition("2026");

// Tier Host：演示 std/json.tl 的完整公开读取接口。
// JSON Pointer 查询是类型化 Host 调用；错误路径会作为可捕获的 Host 错误返回。
import "std/json.tl" as json;

task main() {
    let document = "{{\"name\":\"Teal\",\"ready\":true,\"count\":2,\"items\":[\"a\",\"b\"],\"rows\":[{{\"id\":\"1\",\"label\":\"first\"}},{{\"id\":\"2\",\"label\":\"second\"}}]}}";
    print(json::read_string(document, "/name"));
    print(json::read_boolean(document, "/ready"), json::read_number(document, "/count"));
    print(json::has(document, "/missing"), json::items(document, "/items").len());
    print(json::array_fields(document, "/rows", ["id", "label"]).join(","));
}
