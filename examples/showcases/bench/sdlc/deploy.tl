@edition("2026");

import "std/error.tl" as errors;
// Cross-tier Showcase：部署通过 Host 工具执行并保存可检查收据。
import "bench/extension.tl" as ext;
import "bench/evidence.tl" as evidence;
import "bench/tools.tl" as tools;
import "std/json.tl" as json;
import "common.tl" as common;
task main() { deploy().await; }
task deploy() {
    let record = ext::get("external_deployment");
    if record == "" {
        record = evidence::command("deploy", ext::get("deploy_approval")).await;
        if !json::read_boolean(record, "/success") { throw(errors::make("runtime", "Deployment failed; inspect its receipt before choosing a rollback", errors::ErrorOrigin.user, false)); }
    } else { record = "External result confirmed by the user:\n" + record; }
    tools::write_file(common::root() + "/deployment.md", "# Deployment\n\n" + record).await;
    ext::complete(record);
}
