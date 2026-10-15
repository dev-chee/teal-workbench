@edition("2026");

// Cross-tier Showcase：只有结构化验证收据通过后才形成交付结果。
import "bench/extension.tl" as ext;
import "bench/evidence.tl" as evidence;
import "bench/tools.tl" as tools;
import "std/json.tl" as json;
import "common.tl" as common;
task application() -> none | Error {
    deliver().await ?;
}

task deliver() -> none | Error {
    let verification = ext::get("verification") ?;
    if !json::read_boolean(verification, "/passed") {
        return Error::make("runtime", "Verification did not pass", ErrorOrigin.user, false);
    }
    let paths = common::parse_strings(ext::get("planned_files") ?);
    paths.push(common::root() ? +"/verification.md");
    let ids: string[] = [];
    for item in json::items(verification, "/checks") {
        ids.push(json::read_string(item, ""));
    }
    let request = "{{\"paths\":" + common::strings(paths) ? +",\"checks\":" + common::strings(ids) ? +"}}";
    let result = evidence::git("commit_verified", request, "feat: deliver verified change", ext::get("plan_approval") ?).await ?;
    tools::write_file(common::root() ? +"/delivery.json", result).await ?;
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
