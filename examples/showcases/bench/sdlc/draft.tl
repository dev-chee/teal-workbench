@edition("2026");

// Cross-tier Showcase：模型读取仓库并起草阶段制品，但不修改生产源码。
import "bench/extension.tl" as ext;
import "bench/tools.tl" as tools;
import "std/agent.tl" as Agent;
import "common.tl" as common;

class Draft { markdown: string, files: string[] }
// 运行时由工作台命名 Agent 绑定覆盖这里的默认模型，不改写源码。
@model("codex", "gpt-5.6-terra")
agent Writer {}
impl Writer {
    pub task draft(stage: string, context: string) -> Draft {
        let options = self.options();
        options.context = [context];
        return self.think("Prepare the " + stage + " artifact for an AI-native SDLC. Read the repository and applicable AGENTS.md/Skills; do not modify source. " + "For intent, clarify problem, affected users, desired outcome, scope, constraints and measurable acceptance criteria; ask questions when needed. " + "For spec, read the current intent and resolve or explicitly retain policy conflicts and open questions. " + "For plan, read the current intent/spec and produce a decision-complete implementation plan with order, risks and configured verification commands. " + "For plan only, files must list every exact workspace-relative source path to modify. Do not use globs, parent traversal, absolute paths or the artifact directory. " + "For other stages return an empty files array. Submit markdown and files through the structured completion tool.", options).await;
    }
}
task application() -> none | Error {
    draft().await ?;
}

task draft() -> none | Error {
    let stage = ext::event("name") ?;
    if stage == "plan" && common::parse_strings(ext::config("checks") ?).len() == 0 {
        return Error::make("runtime", "Configure nonempty required checks before drafting the implementation plan", ErrorOrigin.user, false);
    }
    let writer = Writer {};
    let result = writer.draft(stage, common::context() ?).await;
    let body = result.markdown;
    if stage == "plan" {
        if result.files.len() == 0 {
            return Error::make("runtime", "The implementation plan must name the files to modify", ErrorOrigin.user, false);
        }
        body += "\n\n## Approved write scope\n";
        for path in result.files {
            if path == "" || path.contains("..") || path.starts_with("/") || path.contains(":") || path.starts_with("intent/") {
                return Error::make("runtime", "Invalid planned source path", ErrorOrigin.user, false);
            }
            body += "- " + path + "\n";
        }
        body += "\n## Required checks\n" + ext::config("checks") ? +"\n";
    }
    let path = common::root() ? +"/" + stage + ".md";
    tools::write_file(path, body.trim_right() + "\n").await ?;
    ext::complete("{{\"artifact\":" + ext::quote(path) ? +",\"files\":" + common::strings(result.files) ? +"}}") ?;
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
