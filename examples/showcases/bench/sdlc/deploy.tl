@edition("2026");

// Cross-tier Showcase：部署通过 Host 工具执行并保存可检查收据。
import "bench/extension.tl" as ext;
import "bench/evidence.tl" as evidence;
import "bench/tools.tl" as tools;
import "std/json.tl" as json;
import "common.tl" as common;
task application() -> none | Error {
    deploy().await ?;
}

task deploy() -> none | Error {
    let record = ext::get("external_deployment") ?;
    if record == "" {
        record = evidence::command("deploy", ext::get("deploy_approval") ?).await ?;
        if !json::read_boolean(record, "/success") {
            return Error::make("runtime", "Deployment failed; inspect its receipt before choosing a rollback", ErrorOrigin.user, false);
        }
    } else {
        record = "External result confirmed by the user:\n" + record;
    }
    tools::write_file(common::root() ? +"/deployment.md", "# Deployment\n\n" + record).await ?;
    ext::complete(record) ?;
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
