@edition("2026");

import "feishu/auth.tl" as auth;
import "feishu/client.tl" as api;
import "feishu/docs.tl" as docs;
import "oauth/oauth.tl" as oauth;
import "std/system.tl" as system;

// 先登记 http://127.0.0.1:8765/callback，并配置应用的文档创建权限。
// 凭证从环境注入，不写入交互记录，不输出 token。
task application() -> none | Error {
    let app_id = system::environment("FEISHU_APP_ID");
    let secret = system::environment("FEISHU_APP_SECRET");
    if !app_id.found || !secret.found {
        return Error::make("runtime", "Set FEISHU_APP_ID and FEISHU_APP_SECRET", ErrorOrigin.user, false);
    }
    let provider = auth::provider(app_id.value, secret.value, "http://127.0.0.1:8765/callback", ["docx:document:create"]);
    let authorization = oauth::begin(provider) ?;
    let listener = oauth::listen(8765).await ?;
    print("Open this URL in your browser:", authorization.url());
    let callback = oauth::wait(listener, "/callback", authorization, 180000).await ?;
    let token = oauth::exchange(provider, authorization, callback).await ?;
    let document = docs::create(api::client(token.access_token), "Teal API example", "").await ?;
    print(document.id, document.title);
}

/// 入口明确处理失败；未知外部效果仍由 VM 终止。
task main() -> int {
    let result = application().await;
    match result {
        Error { .. } => {
            print(result.message);
            return 1;
        } _ => {
            return 0;
        }
    }
}
