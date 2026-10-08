@edition("2026");

import "std/error.tl" as errors;
// Cross-tier Showcase：独立 Reviewer 读取实际源码和验证证据，不复用 Builder 判断。
import "bench/extension.tl" as ext;
import "bench/evidence.tl" as evidence;
import "bench/tools.tl" as tools;
import "std/json.tl" as json;
import "std/agent.tl" as Agent;
import "common.tl" as common;

class Review { important: bool, summary: string }
@model("codex", "gpt-5.6-terra")
agent Reviewer { focus: string }
impl Reviewer {
    pub task review(context: string) -> Review {
        // 并行子 Agent 继承宿主分配的额度，不用固定轮数反向扩大子预算。
        let options = self.options(); options.context = [context];
        return self.think("Independently review the current change for " + self.focus + ". Read approved artifacts and actual source. "
            + "Use only read tools. Important means broken behavior, data exposure or policy violations. Limit nits to five. "
            + "Submit important=true only with actionable source evidence; otherwise false. Do not modify files or claim commands ran without receipts.", options).await;
    }
}
task main() { verify().await; }
task verify() {
    let checks = common::parse_strings(ext::config("checks"));
    if checks.len() == 0 { throw(errors::make("runtime", "Configure at least one deterministic check", errors::ErrorOrigin.user, false)); }
    let passed = true; let ids: string[] = [];
    let report = "# Verification\n\n";
    for name in checks {
        let result = evidence::command(name, "").await;
        if !json::read_boolean(result, "/success") { passed = false; }
        ids.push(json::read_string(result, "/receipt/id"));
        report += "## " + name + "\n\n" + result + "\n\n";
    }
    if passed {
        let bugs = Reviewer { focus: "bugs and boundary cases" };
        let security = Reviewer { focus: "security and authorization" };
        let compliance = Reviewer { focus: "conformance with intent, spec, plan and repository policy" };
        let context = common::context() + "\nImplementation: " + ext::event("input") + "\nChecks:\n" + report;
        let reviews = Task::all([bugs.review(context), security.review(context), compliance.review(context)]).await;
        for review in reviews { if review.important { passed = false; } report += "## Independent review\n" + review.summary + "\n\n"; }
    }
    let path = common::root() + "/verification.md";
    tools::write_file(path, report.trim_right() + "\n").await;
    ext::complete("{{\"passed\":" + string(passed) + ",\"report\":" + ext::quote(path) + ",\"checks\":" + common::strings(ids) + "}}");
}
