@edition("2026");

// Cross-tier Showcase：扩展命令把事件输入写入该扩展实例的私有状态。
import "bench/extension.tl" as ext;
fn application() -> none | Error {
    ext::set("greeting", ext::event("input") ?) ?;
    ext::message("Greeting saved. Open :panel extension-hello.") ?;
}

/// 入口明确处理失败；未知外部效果仍由 VM 终止。
fn main() -> int {
    let result = application();
    match result {
        Error { .. } => {
            print(result.message);
            return 1;
        } _ => {
            return 0;
        }
    }
}
