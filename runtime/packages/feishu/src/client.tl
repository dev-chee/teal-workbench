@edition("2026");

import "http/client.tl" as http;
import "http/json.tl" as encode;
host "std.json" as json {
    fn has(text: string, pointer: string) -> bool | Error;

    fn read_string(text: string, pointer: string) -> string | Error;

    fn read_number(text: string, pointer: string) -> string | Error;

    fn read_boolean(text: string, pointer: string) -> bool | Error;

    fn items(text: string, pointer: string) -> string[] | Error;

}
/// 飞书客户端；token 由调用方提供和更新，不隐式切换用户身份。
pub class Client { base_url: string, token: string, timeout_ms: int }
/// 创建官方端点客户端，也允许修改 base_url 连接测试服务。
pub fn client(token: string) -> Client {
    return Client { base_url: "https://open.feishu.cn/open-apis", token: token, timeout_ms: 30000 };
}

impl Client {
    /// 返回当前 API 根地址。
    pub fn base_url() -> string {
        return self.base_url;
    }

    /// 返回请求超时毫秒数。
    pub fn timeout_ms() -> int {
        return self.timeout_ms;
    }

    /// 更新 API 根地址，用于受控代理或测试服务。
    pub fn set_base_url(value: string) {
        self.base_url = value;
    }

    /// 更新请求超时毫秒数。
    pub fn set_timeout_ms(value: int) {
        self.timeout_ms = value;
    }

    /// 替换后续请求使用的访问 token。
    pub fn set_token(value: string) {
        self.token = value;
    }
}
/// 解码飞书响应，错误只包含状态与业务码，不暴露请求或凭证。
pub fn data(response: http::Response) -> string | Error {
    if response.status < 200 || response.status >= 300 {
        return Error::make("feishu_http", "Feishu HTTP error: " + string(response.status), ErrorOrigin. host, true);
    }
    if !response.is_utf8 {
        return Error::make("feishu_http", "Feishu response is not UTF-8", ErrorOrigin. host, false);
    }
    let code = json::read_number(response.text, "/code") ?;
    if code != "0" {
        return Error::make("feishu_api", "Feishu API error: " + code, ErrorOrigin. host, false);
    }
    return if json::has(response.text, "/data") ? { encode::get(response.text, "/data") ? } else { "{{}}" };
}

/// 调用一个相对 API 路径，不重试写入；分页和部分完成由上层显式处理。
pub task call(client: Client, method: string, path: string, body: string) -> string | Error {

    if !path.starts_with("/") || path.starts_with("//") {
        return Error::make("feishu_path", "Expected relative Feishu API path", ErrorOrigin.user, false);
    }
    let request = http::request(method, client.base_url + path);
    request.timeout_ms = client.timeout_ms;
    request.headers = [http::Header { name: "Authorization", value: "Bearer " + client.token }, http::Header { name: "Content-Type", value: "application/json" }];
    request.body = http::Body.text { value: body };
    return data(http::send(request).await ?) ?;
}

/// 发送带 Bearer 身份的 multipart 请求；边界和 Content-Type 由 HTTP 包生成。
pub task call_multipart(client: Client, path: string, parts: http::Part[]) -> string | Error {

    if !path.starts_with("/") || path.starts_with("//") {
        return Error::make("feishu_path", "Expected relative Feishu API path", ErrorOrigin.user, false);
    }
    let request = http::request("POST", client.base_url + path);
    request.timeout_ms = client.timeout_ms;
    request.headers = [http::Header { name: "Authorization", value: "Bearer " + client.token }];
    request.body = http::Body.multipart { parts: parts };
    return data(http::send(request).await ?) ?;
}

/// 校验并编码资源标识，避免路径注入。
pub fn id(value: string) -> string | Error {
    if value == "" || value == "." || value == ".." || value.contains("/") || value.contains("\\") {
        return Error::make("feishu_path", "Invalid Feishu resource identifier", ErrorOrigin.user, false);
    }
    return http::encode_component(value) ?;
}
