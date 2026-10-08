@edition("2026");

import "std/error.tl" as errors;
import "std/fs.tl" as fs;

import "feishu/client.tl" as api;
import "feishu/markdown.tl" as markdown;
import "std/system.tl" as system;

// 输入文件与图片目录由调用方指定，写入目标是 token 身份的默认空间。
// 运行前确认这是你希望创建文档的位置；示例不自动分享文档。
task main() {
    let args = argv();
    if args.len() != 2 {
        throw(errors::make("runtime", "Usage: feishu_import <markdown-file> <image-base-directory>", errors::ErrorOrigin.user, false));
    }
    let token = system::environment("FEISHU_ACCESS_TOKEN");
    if !token.found {
        throw(errors::make("runtime", "Set FEISHU_ACCESS_TOKEN", errors::ErrorOrigin.user, false));
    }
    let client = api::client(token.value);
    let text = fs::read_or_throw(args[0]).await;
    let pending = markdown::start_or_throw(client, text, args[1], "Teal Markdown import", "").await;
    print("Import ticket:", pending.ticket);
    let imported = markdown::wait_or_throw(client, pending, 60).await;
    print("Document:", imported.document, imported.url);
    for warning in imported.warnings {
        print("Import warning:", warning);
    }
    for image in imported.images {
        let block = markdown::attach_image_or_throw(client, imported, image).await;
        print("Image block:", block);
        markdown::fill_image_or_throw(client, imported, image, block).await;
    }
}
