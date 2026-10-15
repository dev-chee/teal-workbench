@edition("2026");

import "oauth/oauth.tl" as oauth;
import "http/client.tl" as http;
import "http/json.tl" as encode;
host "std.json" as json {
    fn has(text: string, pointer: string) -> bool | Error;

    fn read_string(text: string, pointer: string) -> string | Error;

    fn read_number(text: string, pointer: string) -> string | Error;

    fn read_boolean(text: string, pointer: string) -> bool | Error;

    fn items(text: string, pointer: string) -> string[] | Error;

}
/// 配置飞书 OAuth v3；回调地址必须在应用后台登记。
pub fn provider(app_id: string, app_secret: string, redirect_uri: string, scopes: string[]) -> oauth::Provider {
    return oauth::Provider { authorize_url: "https://accounts.feishu.cn/open-apis/authen/v1/authorize", token_url: "https://accounts.feishu.cn/oauth/v3/token", client_id: app_id, client_secret: app_secret, redirect_uri: redirect_uri, scopes: scopes, json_body: true };
}
/// 应用身份凭证及服务端声明的有效秒数。
pub class TenantToken { pub token: string, pub expires_in: int }
/// 获取自建应用 token；个人文档可另用 OAuth 用户 token。
pub task tenant_token(app_id: string, app_secret: string) -> TenantToken | Error {

    let request = http::request("POST", "https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal");
    request.headers = [http::Header { name: "Content-Type", value: "application/json" }];
    request.body = http::Body.text { value: encode::object([encode::text("app_id", app_id) ?, encode::text("app_secret", app_secret) ?]) ? };
    let response = http::send(request).await ?;
    if response.status != 200 || !response.is_utf8 {
        return Error::make("feishu_auth", "Feishu app authentication failed", ErrorOrigin. host, true);
    }
    if json::read_number(response.text, "/code") ? != "0" {
        return Error::make("feishu_auth", "Feishu app authentication rejected", ErrorOrigin. host, true);
    }
    return TenantToken { token: json::read_string(response.text, "/tenant_access_token") ?, expires_in: int(json::read_number(response.text, "/expire") ?) };
}
