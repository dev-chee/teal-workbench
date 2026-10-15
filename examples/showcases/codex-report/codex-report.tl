@edition("2026");
@lint { allow: ["unused-pure-result"] };

import "std/fs.tl" as fs;

// 将已准备的 Git 证据转换为持久且受证据约束的更新报告。
import "std/agent.tl" as Agent;
import "std/system.tl" as system;

static report_path = "F:/notes/codex-report.md";
static state_path = "F:/notes/codex-report.state";
static pending_path = "F:/notes/codex-report.pending";

@model("deepseek", "deepseek-v4-flash", "codex-report")
agent UpdateSummarizer {}

impl UpdateSummarizer {
    pub task summarize(evidence: string) -> string {
        let options = self.options();
        options.model_rounds = 3;
        options.max_output_tokens = 8192;
        options.timeout_ms = 600000;
        options.propose_memory = false;
        return self.think("Write concise release notes in Simplified Chinese from the supplied Git evidence. " + "Return Markdown only without a top-level heading. Organize supported claims into sections for new features, " + "experience improvements, reliability and security, engineering maintenance and removals, and items worth attention. " + "Use 8 to 15 thematic bullets total and stay under about 1200 Chinese characters; never enumerate every commit. " + "Omit empty sections. Distinguish additions from fixes and removals. Do not invent APIs, behavior, or motivation. " + "When commit subjects are ambiguous, state the uncertainty. Do not use external tools. End with a short direct answer " + "to what functionality this update added. Submit the complete Markdown through __teal_complete alone.\n\n" + evidence, options,).await;
    }
}

task write_failure(generated_at: string, error: string) -> none | Error {
    let report = "# Codex Update Report\n\n" + "- Status: failed\n" + "- Checked at (UTC): " + generated_at + "\n" + "- Repository: `E:\\codex`\n\n" + "## Failure\n\n```text\n" + error.replace("```", "` ` `") + "\n```\n";
    fs::write(report_path, report).await ?;
}

task build_report(generated_at: string) -> none | Error {
    if !fs::exists(pending_path).await ? {
        return Error::make("runtime", "Prepared Git evidence is missing", ErrorOrigin.user, false);
    }

    let pending = fs::read(pending_path).await ?;
    let sections = pending.split("\n---EVIDENCE---\n");
    if sections.len() != 2 {
        return Error::make("runtime", "Prepared Git evidence has an invalid envelope", ErrorOrigin.user, false);
    }
    let metadata = sections[0].split("\n");
    if metadata.len() != 4 {
        return Error::make("runtime", "Prepared Git evidence has invalid metadata", ErrorOrigin.user, false);
    }

    let baseline = metadata[0];
    let current = metadata[1];
    let commit_count = metadata[2];
    let short_stat = metadata[3];
    let summarizer = UpdateSummarizer {};
    let summary = summarizer.summarize(sections[1]).await.trim();
    if summary == "" {
        return Error::make("runtime", "Agent returned an empty summary", ErrorOrigin.user, false);
    }

    let report = "# Codex Update Report\n\n" + "- Status: updated\n" + "- Generated at (UTC): " + generated_at + "\n" + "- Repository: `E:\\codex`\n" + "- Branch: `main`\n" + "- Previous commit: `" + baseline + "`\n" + "- Current commit: `" + current + "`\n" + "- Commits analyzed: " + commit_count + "\n" + "- Diff summary: " + short_stat + "\n\n" + summary + "\n";

    fs::write(report_path, report).await ?;
    fs::write(state_path, current + "\n").await ?;
}

task main() -> int {
    let generated_at = system::utc_now();
    let result = build_report(generated_at).await;
    match result {
        Error { .. } => {
            let recorded = write_failure(generated_at, result.message).await;
            match recorded {
                Error { .. } => {
                    print(recorded.message);
                } _ => {}
            }
            print(result.message);
            return 1;
        } _ => {
            return 0;
        }
    }
}
