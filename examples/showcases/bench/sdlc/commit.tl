@edition("2026");

// Cross-tier Showcase：提交前核对已验证证据和限定文件集合。
import "bench/extension.tl" as ext;
import "bench/evidence.tl" as evidence;
import "common.tl" as common;
task application() -> none | Error {
    commit().await ?;
}

task commit() -> none | Error {
    let result = evidence::git("commit", ext::get("approved_artifacts") ?, "docs: approve development plan", ext::get("plan_approval") ?).await ?;
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
