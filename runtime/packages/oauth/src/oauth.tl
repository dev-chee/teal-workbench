@edition("2026");

import "http/client.tl" as http;
import "http/json.tl" as encode;
host "std.json" as json {
    fn has(text: string, pointer: string) -> bool | Error;

    fn read_string(text: string, pointer: string) -> string | Error;

    fn read_number(text: string, pointer: string) -> string | Error;

    fn read_boolean(text: string, pointer: string) -> bool | Error;

    fn items(text: string, pointer: string) -> string[] | Error;

} host "std.oauth" as native {
    fn random() -> string | Error;

    fn challenge(verifier: string) -> string | Error;

    fn parameter(callback: string, name: string) -> string | Error;

    fn matches_redirect(callback: string, redirect_uri: string) -> bool | Error;

    task listen(port: int) -> string | Error;

    task close(handle: string) -> none | Error;

    task wait(handle: string, path: string, state: string, timeout_ms: int) -> string | Error;
}
/// OAuth 服务配置；secret 由调用方注入，不由库持久化。
pub class Provider { pub authorize_url: string, pub token_url: string, pub client_id: string, pub client_secret: string, pub redirect_uri: string, pub scopes: string[], pub json_body: bool }
/// 一次授权的随机上下文，不应记录到日志或跨授权复用。
pub class Authorization { url: string, state: string, verifier: string }
/// token 有效期以服务端返回秒数为准；缺少刷新授权时 refresh_token 为空。
pub class Token { pub access_token: string, pub refresh_token: string, pub expires_in: int, pub refresh_expires_in: int, pub scope: string, pub token_type: string }

impl Authorization {
    /// 返回应向用户展示的授权地址，不暴露 state 或 PKCE verifier。
    pub fn url() -> string {
        return self.url;
    }
}
/// 生成使用 PKCE S256 的授权地址，不自动打开浏览器。
pub fn begin(provider: Provider) -> Authorization | Error {
    let state = native::random() ?;
    let verifier = native::random() ?;
    let url = http::query(provider.authorize_url, [http::Header { name: "response_type", value: "code" }, http::Header { name: "client_id", value: provider.client_id }, http::Header { name: "redirect_uri", value: provider.redirect_uri }, http::Header { name: "scope", value: provider.scopes.join(" ") }, http::Header { name: "state", value: state }, http::Header { name: "code_challenge", value: native::challenge(verifier) ? }, http::Header { name: "code_challenge_method", value: "S256" }]) ?;
    return Authorization { url: url, state: state, verifier: verifier };
}

/// 先绑定已登记的回环端口，再展示授权地址；句柄只能消费一次。
pub task listen(port: int) -> string | Error {
    return native::listen(port).await ?;
}

/// 在放弃登录时释放尚未进入 wait 的监听句柄；重复关闭无副作用。
pub task close(handle: string) -> none | Error {

    native::close(handle).await ?;
    return none;
}

/// 等待匹配路径和 state 的回调，最多五分钟，返回回调目标。
pub task wait(handle: string, path: string, authorization: Authorization, timeout_ms: int) -> string | Error {
    return native::wait(handle, path, authorization.state, timeout_ms).await ?;
}

fn optional(text: string, pointer: string) -> string | Error {
    return if json::has(text, pointer) ? { json::read_string(text, pointer) ? } else { "" };
}

fn seconds(text: string, pointer: string) -> int | Error {
    return if json::has(text, pointer) ? { int(json::read_number(text, pointer) ?) } else { 0 };
}

/// 解码标准 OAuth token；不把响应原文或秘密写入错误消息。
pub fn decode(response: http::Response) -> Token | Error {

    if response.status < 200 || response.status >= 300 || !response.is_utf8 {
        return Error::make("oauth_http", "OAuth token request failed", ErrorOrigin. host, true);
    }
    let body = response.text;
    if json::has(body, "/error") ? {
        return Error::make("oauth_provider", "OAuth provider rejected the token request", ErrorOrigin. host, false);
    }
    if json::has(body, "/code") ? && json::read_number(body, "/code") ? != "0" {
        return Error::make("oauth_provider", "OAuth provider returned an error", ErrorOrigin. host, false);
    }
    return Token { access_token: json::read_string(body, "/access_token") ?, refresh_token: optional(body, "/refresh_token") ?, expires_in: seconds(body, "/expires_in") ?, refresh_expires_in: seconds(body, "/refresh_token_expires_in") ?, scope: optional(body, "/scope") ?, token_type: json::read_string(body, "/token_type") ? };
}

/// 发送标准 token 请求，不自动重试授权码或刷新凭证。
pub task token_request(provider: Provider, parameters: http::Header[]) -> Token | Error {

    let request = http::request("POST", provider.token_url);
    let fields: encode::Field[] = [];
    let form: string[] = [];
    for parameter in parameters {
        fields.push(encode::text(parameter.name, parameter.value) ?);
        form.push(http::encode_component(parameter.name) ? +"=" + http::encode_component(parameter.value) ?);
    }
    request.headers = [http::Header {
        name: "Content-Type",
        value: if provider.json_body { "application/json" } else { "application/x-www-form-urlencoded" }
    }];
    request.body = http::Body.text {
        value: if provider.json_body { encode::object(fields) ? } else { form.join("&") }
    };
    let response = http::send(request).await ?;
    return decode(response) ?;
}

/// 验证 state 后交换授权码；授权拒绝和缺少 code 都失败。
pub task exchange(provider: Provider, authorization: Authorization, callback: string) -> Token | Error {

    if !native::matches_redirect(callback, provider.redirect_uri) ? {
        return Error::make("oauth_callback", "OAuth callback URI mismatch", ErrorOrigin.user, false);
    }
    if authorization.state == "" || native::parameter(callback, "state") ? != authorization.state {
        return Error::make("oauth_state", "OAuth state mismatch", ErrorOrigin.user, false);
    }
    if native::parameter(callback, "error") ? != "" {
        return Error::make("oauth_denied", "OAuth authorization denied", ErrorOrigin.user, false);
    }
    let code = native::parameter(callback, "code") ?;
    if code == "" {
        return Error::make("oauth_callback", "OAuth callback has no authorization code", ErrorOrigin.user, false);
    }
    return token_request(provider, [http::Header { name: "grant_type", value: "authorization_code" }, http::Header { name: "client_id", value: provider.client_id }, http::Header { name: "client_secret", value: provider.client_secret }, http::Header { name: "redirect_uri", value: provider.redirect_uri }, http::Header { name: "code", value: code }, http::Header { name: "code_verifier", value: authorization.verifier }]).await ?;
}

/// 刷新 token；调用方须用返回的新刷新凭证替换旧值，不重放不明确的交换。
pub task refresh(provider: Provider, refresh_token: string) -> Token | Error {

    if refresh_token == "" {
        return Error::make("oauth_refresh", "No OAuth refresh token available", ErrorOrigin.user, false);
    }
    return token_request(provider, [http::Header { name: "grant_type", value: "refresh_token" }, http::Header { name: "client_id", value: provider.client_id }, http::Header { name: "client_secret", value: provider.client_secret }, http::Header { name: "refresh_token", value: refresh_token }]).await ?;
}
