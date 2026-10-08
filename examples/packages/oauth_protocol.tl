@edition("2026");

// Tier Host 包示例：OAuth token 解码、错误分支和公开配置形状，不发送任何请求。
// PKCE 授权地址需要 crypto.random，回环监听和 token 交换需要 network.oauth_callback；
// 这些外部效果由 packages/feishu_oauth.tl 的 check 入口和宿主夹具单独验证。
import "http/client.tl" as http;
import "oauth/oauth.tl" as oauth;

fn response(status: int, body: string) -> http::Response {
    return http::Response {
        status: status,
        headers: [],
        text: body,
        is_utf8: true,
        base64: "",
    };
}

task main() {
    // 缺少刷新授权时 refresh_token 为空，有效期取服务端返回的秒数。
    let token = oauth::decode_or_throw(response(200, "{{\"access_token\":\"access\",\"token_type\":\"Bearer\",\"expires_in\":3600}}"));
    print(token.access_token, token.token_type, token.expires_in, token.refresh_token == "");

    let provider = oauth::Provider {
        authorize_url: "https://example.com/authorize",
        token_url: "https://example.com/token",
        client_id: "client",
        client_secret: "",
        redirect_uri: "http://127.0.0.1:8765/callback",
        scopes: ["docs:read"],
        json_body: false,
    };
    print(provider.redirect_uri, provider.scopes.join(" "));

    // 非 2xx 响应不携带秘密文本地失败。
    let rejected = false;
    match oauth::decode(response(400, "{{\"error\":\"invalid_grant\"}}")) {
        Result.ok { value } => { print(value.access_token); }
        Result.err { error } => {
            rejected = error.kind == "oauth_http" && error.retryable;
        }
    }
    print(rejected);
}
