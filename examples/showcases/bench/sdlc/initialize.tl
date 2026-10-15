@edition("2026");

// Cross-tier Showcase：初始化工作流身份、目标与基线证据。
import "bench/extension.tl" as ext;
import "bench/evidence.tl" as evidence;
import "common.tl" as common;
task application() -> none | Error {
    initialize().await ?;
}

task initialize() -> none | Error {
    let result = evidence::git("begin", common::strings(["codex/sdlc/" + ext::get("flow_id") ?]) ?, "", "").await ?;
    ext::complete(result) ?;
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
