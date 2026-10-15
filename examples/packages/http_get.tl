@edition("2026");

import "http/client.tl" as http;
// 运行时需要 network.http 及目标网络资源授权。
task application() -> none | Error {
    let args = argv();
    if args.len() != 1 {
        return Error::make("usage", "Usage: http_get <url>", ErrorOrigin.user, false);
    }
    let response = http::send(http::request("GET", args[0])).await ?;
    print(response.status);
    if response.is_utf8 {
        print(response.text);
    }
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
