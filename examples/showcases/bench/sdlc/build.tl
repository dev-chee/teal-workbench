@edition("2026");

// Cross-tier Showcase：模型只在批准的写范围内实现已确认计划。
import "bench/extension.tl" as ext;
import "std/agent.tl" as Agent;
import "common.tl" as common;

class Implementation { summary: string, plan_changed: bool }
@model("codex", "gpt-5.6-terra")
agent Builder {}
impl Builder {
    pub task implement(context: string) -> Implementation {
        let options = self.options();
        options.context = [context];
        return self.think("Implement the approved intent/spec/plan. Read them first. Only modify the exact approved write scope; the Host enforces it. " + "Respect AGENTS.md. Preserve unrelated edits. Do not change tests to hide failures. Do not commit, push or deploy. " + "If the implementation requires changing the plan, stop before making out-of-scope changes and return plan_changed=true with the reason. " + "Otherwise return plan_changed=false and a precise summary. Deterministic checks and independent reviews run in the next stage.", options).await;
    }
}
task application() -> none | Error {
    build().await ?;
}

task build() -> none | Error {
    let builder = Builder {};
    let result = builder.implement(common::context() ? +"\nApproved files: " + ext::get("planned_files") ?).await;
    ext::complete("{{\"summary\":" + ext::quote(result.summary) ? +",\"plan_changed\":" + string(result.plan_changed) + "}}") ?;
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
