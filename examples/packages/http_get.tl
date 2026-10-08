@edition("2026");

import "http/client.tl" as http;
import "std/error.tl" as errors;

// 运行时需要 network.http 及目标网络资源授权。
task main() {
    let args = argv();
    if args.len() != 1 {
        throw(errors::make("usage", "Usage: http_get <url>", errors::ErrorOrigin.user, false));
    }
    let response = http::send_or_throw(http::request("GET", args[0])).await;
    print(response.status);
    if response.is_utf8 {
        print(response.text);
    }
}
