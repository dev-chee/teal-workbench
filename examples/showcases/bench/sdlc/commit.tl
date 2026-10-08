@edition("2026");

// Cross-tier Showcase：提交前核对已验证证据和限定文件集合。
import "bench/extension.tl" as ext;
import "bench/evidence.tl" as evidence;
import "common.tl" as common;
task main() { commit().await; }
task commit() {
    let result = evidence::git("commit", ext::get("approved_artifacts"), "docs: approve development plan", ext::get("plan_approval")).await;
    ext::complete(result);
}
