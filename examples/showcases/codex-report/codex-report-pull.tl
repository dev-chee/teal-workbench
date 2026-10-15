@edition("2026");
@lint { allow: ["unused-pure-result"] };

import "std/fs.tl" as fs;

// 校验并以 fast-forward-only 更新官方 Codex 仓库，然后准备有界证据。
import "std/process.tl" as process;
import "std/system.tl" as system;

static repository_path = "E:/codex";
static expected_remote = "https://github.com/openai/codex.git";
static report_path = "F:/notes/codex-report.md";
static state_path = "F:/notes/codex-report.state";
static pending_path = "F:/notes/codex-report.pending";
static git_path = "C:/Program Files/Git/cmd/git.exe";

fn process_exit_code(result: process::ProcessResult) -> int {
    let code = -1;
    match result.state {
        process::ProcessState.exited { code: value } => {
            code = value;
        } _ => {}
    }
    return code;
}

fn process_succeeded(result: process::ProcessResult) -> bool {
    return process_exit_code(result) == 0;
}

task run_git(args: string[], timeout_ms: int, capture_limit: int) -> process::ProcessResult | Error {
    let spec = process::command(git_path, args);
    spec.cwd = repository_path;
    spec.capture_limit_bytes = capture_limit;
    return process::run(spec, timeout_ms).await ?;
}

fn require_git(label: string, result: process::ProcessResult) -> string | Error {
    if !process_succeeded(result) {
        return Error::make("example_failure", label + " failed with exit code " + string(process_exit_code(result)) + "\n" + result.stderr.trim(), ErrorOrigin.user, false);
    }
    if result.stdout_truncated || result.stderr_truncated {
        return Error::make("example_failure", label + " exceeded the configured output limit", ErrorOrigin.user, false);
    }
    return result.stdout.trim();
}

task git(args: string[]) -> string | Error {
    return require_git("git " + args.join(" "), run_git(args, 300000, 1048576).await ?) ?;
}

task write_failure(generated_at: string, error: string) -> none | Error {
    let report = "# Codex Update Report\n\n" + "- Status: failed\n" + "- Checked at (UTC): " + generated_at + "\n" + "- Repository: `E:\\codex`\n\n" + "## Failure\n\n```text\n" + error.replace("```", "` ` `") + "\n```\n";
    fs::write(report_path, report).await ?;
}

task prepare(generated_at: string) -> int | Error {
    if git(["remote", "get-url", "origin"]).await ? != expected_remote {
        return Error::make("runtime", "Repository origin does not match the expected official Codex remote", ErrorOrigin.user, false);
    }
    if git(["branch", "--show-current"]).await ? != "main" {
        return Error::make("runtime", "Repository must be on the main branch", ErrorOrigin.user, false);
    }
    if git(["status", "--porcelain=v1", "--untracked-files=all"]).await ? != "" {
        return Error::make("runtime", "Repository has tracked or untracked changes; refusing to pull", ErrorOrigin.user, false);
    }

    let before_pull = git(["rev-parse", "HEAD"]).await ?;
    let baseline = before_pull;
    if fs::exists(state_path).await ? {
        baseline = fs::read(state_path).await ?.trim();
        if baseline == "" {
            return Error::make("runtime", "State file is empty", ErrorOrigin.user, false);
        }
    } else if fs::exists(pending_path).await ? {
        let previous_pending = fs::read(pending_path).await ?;
        let previous_sections = previous_pending.split("\n---EVIDENCE---\n");
        if previous_sections.len() != 2 {
            return Error::make("runtime", "Prepared Git evidence has an invalid envelope", ErrorOrigin.user, false);
        }
        let previous_metadata = previous_sections[0].split("\n");
        if previous_metadata.len() != 4 || previous_metadata[0] == "" {
            return Error::make("runtime", "Prepared Git evidence has invalid metadata", ErrorOrigin.user, false);
        }
        baseline = previous_metadata[0];
    } _ = git(["pull", "--ff-only", "origin", "main"]).await ?;
    let current = git(["rev-parse", "HEAD"]).await ?;
    let ancestry = run_git(["merge-base", "--is-ancestor", baseline, current], 300000, 65536).await ?;
    if !process_succeeded(ancestry) {
        return Error::make("runtime", "Saved report baseline is not an ancestor of the current repository HEAD", ErrorOrigin.user, false);
    }

    if baseline == current {
        let report = "# Codex Update Report\n\n" + "- Status: no updates\n" + "- Checked at (UTC): " + generated_at + "\n" + "- Repository: `E:\\codex`\n" + "- Branch: `main`\n" + "- HEAD: `" + current + "`\n\n" + "No new commits were found since the last successful report.\n";
        fs::write(report_path, report).await ?;
        fs::write(state_path, current + "\n").await ?;
        return 10;
    }

    let range = baseline + ".." + current;
    let commit_count_text = git(["rev-list", "--count", range]).await ?;
    let commit_count = int(commit_count_text);
    if commit_count > 500 {
        return Error::make("runtime", "Update range exceeds 500 commits; manual review is required before advancing the baseline", ErrorOrigin.user, false);
    }

    let subjects = git(["log", "--format=%h%x09%s", range]).await ?;
    let short_stat = git(["diff", "--shortstat", range]).await ?;
    let directory_stat = git(["diff", "--dirstat=files,5,cumulative", range]).await ?;
    let evidence = "Repository: openai/codex\n" + "Branch: main\n" + "Previous commit: " + baseline + "\n" + "Current commit: " + current + "\n" + "Commit count: " + commit_count_text + "\n" + "Diff summary: " + short_stat + "\n\n" + "Changed directory distribution:\n" + directory_stat + "\n\n" + "Commit subjects, newest first:\n" + subjects;
    let pending = baseline + "\n" + current + "\n" + commit_count_text + "\n" + short_stat + "\n---EVIDENCE---\n" + evidence;
    fs::write(pending_path, pending).await ?;
    return 0;
}

task main() -> int {
    let generated_at = system::utc_now();
    let result = prepare(generated_at).await;
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
            return result;
        }
    }
}
