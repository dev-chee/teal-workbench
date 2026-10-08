@edition("2026");

// Cross-tier Showcase：初始化工作流身份、目标与基线证据。
import "bench/extension.tl" as ext;
import "bench/evidence.tl" as evidence;
import "common.tl" as common;
task main() { initialize().await; }
task initialize() {
    let result = evidence::git("begin", common::strings(["codex/sdlc/" + ext::get("flow_id")]), "", "").await;
    ext::complete(result);
}
