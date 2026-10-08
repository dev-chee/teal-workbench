@edition("2026");

import "oauth/oauth.tl" as oauth;
import "http/client.tl" as http;
import "http/json.tl" as encode;
import "std/json.tl" as json;
import "std/error.tl" as errors;
/// 配置飞书 OAuth v3；回调地址必须在应用后台登记。
pub fn provider(app_id: string, app_secret: string, redirect_uri: string, scopes: string[]) -> oauth::Provider {
    return oauth::Provider { authorize_url: "https://accounts.feishu.cn/open-apis/authen/v1/authorize", token_url: "https://accounts.feishu.cn/oauth/v3/token", client_id: app_id, client_secret: app_secret, redirect_uri: redirect_uri, scopes: scopes, json_body: true };
}
/// 应用身份凭证及服务端声明的有效秒数。
pub class TenantToken { pub token: string, pub expires_in: int }
/// 获取自建应用 token；个人文档可另用 OAuth 用户 token。
pub task tenant_token(app_id: string, app_secret: string) -> Result<TenantToken, errors::Error> {
    try {
        let request = http::request("POST", "https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal");
        request.headers = [http::Header { name: "Content-Type", value: "application/json" }];
        request.body = http::Body.text { value: encode::object([encode::text("app_id", app_id), encode::text("app_secret", app_secret)]) };
        let response = http::send_or_throw(request).await;
        if response.status != 200 || !response.is_utf8 {
            throw(errors::make("feishu_auth", "Feishu app authentication failed", errors::ErrorOrigin.host, true));
        }
        if json::read_number(response.text, "/code") != "0" {
            throw(errors::make("feishu_auth", "Feishu app authentication rejected", errors::ErrorOrigin.host, true));
        }
        return Result::ok(TenantToken { token: json::read_string(response.text, "/tenant_access_token"), expires_in: int(json::read_number(response.text, "/expire")) });
    } catch error {
        return Result::err(errors::make("feishu_auth", error.message, errors::ErrorOrigin.host, true));
    }
    return Result::err(errors::make("feishu_auth", "tenant token request did not produce a result", errors::ErrorOrigin.host, true));
}

pub task tenant_token_or_throw(app_id: string, app_secret: string) -> TenantToken {
    match tenant_token(app_id, app_secret).await {
        Result.ok { value } => { return value; }
        Result.err { error } => { throw(error); }
    }
}
