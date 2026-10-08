@edition("2026");

// Cross-tier Showcase：扩展命令把事件输入写入该扩展实例的私有状态。
import "bench/extension.tl" as ext;
fn main() {
    ext::set("greeting", ext::event("input"));
    ext::message("Greeting saved. Open :panel extension-hello.");
}
