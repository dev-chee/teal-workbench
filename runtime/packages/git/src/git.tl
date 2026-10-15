@edition("2026");

import "std/process.tl" as process;
/// Git 执行配置；所有操作都使用相同工作目录和有界等待。
pub class Repository { path: string, executable: string, timeout_ms: int }
/// 原始退出结果；冲突和拒绝保留为非零退出码，不自动修改现场。
pub class CommandResult { pub code: int, pub stdout: string, pub stderr: string }
/// porcelain v1 的索引及工作区状态；重命名保留原路径。
pub class StatusEntry { pub index: string, pub worktree: string, pub path: string, pub original_path: string }
/// 不解析本地化日期的提交摘要。
pub class Commit { pub oid: string, pub parents: string, pub author: string, pub timestamp: string, pub subject: string }
/// 显式选择重置对索引和工作区的影响。
pub enum ResetMode { soft, mixed, hard }

/// 选择仓库目录；默认使用 PATH 中的 Git，超时两分钟。
pub fn repository(path: string) -> Repository {
    return Repository { path: path, executable: "git", timeout_ms: 120000 };
}

impl Repository {
    /// 返回仓库工作目录。
    pub fn path() -> string {
        return self.path;
    }

    /// 返回当前 Git 可执行文件。
    pub fn executable() -> string {
        return self.executable;
    }

    /// 返回单次 Git 调用的超时毫秒数。
    pub fn timeout_ms() -> int {
        return self.timeout_ms;
    }

    /// 更新仓库工作目录。
    pub fn set_path(value: string) {
        self.path = value;
    }

    /// 更新 Git 可执行文件。
    pub fn set_executable(value: string) {
        self.executable = value;
    }

    /// 更新单次 Git 调用的超时毫秒数。
    pub fn set_timeout_ms(value: int) {
        self.timeout_ms = value;
    }
}
/// 执行显式 Git 参数；不经过 shell，不自动重试或启动凭证提示。
pub task run(repo: Repository, arguments: string[]) -> CommandResult | Error {
    let args = ["--no-pager", "--literal-pathspecs"];
    for argument in arguments {
        args.push(argument);
    }
    let spec = process::command(repo.executable, args);
    spec.cwd = repo.path;
    spec.environment = { "GIT_TERMINAL_PROMPT": "0", "GIT_EDITOR": "true", "GIT_SEQUENCE_EDITOR": "true", "LC_ALL": "C" };
    let result = process::run(spec, repo.timeout_ms).await ?;
    if result.stdout_truncated || result.stderr_truncated {
        return Error::make("git", "Git output exceeded capture limit", ErrorOrigin. host, false);
    }
    let code = -1;
    match result.state {
        process::ProcessState.exited { code: value } => {
            code = value;
        } _ => {
            return Error::make("git", "Git process did not exit normally", ErrorOrigin. host, false);
        }
    }
    return CommandResult { code: code, stdout: result.stdout, stderr: result.stderr };
}

/// 要求退出成功，否则抛出 Git 诊断。
pub fn checked(result: CommandResult) -> string | Error {
    if result.code != 0 {
        return Error::make("git", "Git failed (" + string(result.code) + "): " + result.stderr, ErrorOrigin. host, false);
    }
    return result.stdout;
}

fn operand(value: string) -> string | Error {
    if value == "" || value.starts_with("-") || value.contains("\0") {
        return Error::make("git", "Invalid Git operand", ErrorOrigin.user, false);
    }
    return value;
}

// Git for Windows 不接受 Win32 扩展路径前缀；保留 UNC 的服务器和共享身份。
fn git_path(value: string) -> string {
    if value.starts_with("\\\\?\\UNC\\") {
        return "\\\\" + value[8..];
    }
    if value.starts_with("\\\\?\\") {
        return value[4..];
    }
    return value;
}

/// 查询包含未跟踪文件的结构化状态，保留空格、换行和 Unicode 路径。
pub task status(repo: Repository) -> StatusEntry[] | Error {
    let output = checked(run(repo, ["status", "--porcelain=v1", "-z", "--untracked-files=all"]).await ?) ?;
    let records = output.split("\0");
    let entries: StatusEntry[] = [];
    let i = 0;
    loop {
        if i >= records.len() {
            break;
        }
        let record = records[i];
        i = i + 1;
        if record == "" {
            continue;
        }
        if record.len() < 4 {
            return Error::make("git", "Invalid Git status record", ErrorOrigin. host, false);
        }
        let original = "";
        if record[0..1] == "R" || record[0..1] == "C" || record[1..2] == "R" || record[1..2] == "C" {
            if i >= records.len() {
                return Error::make("git", "Missing Git rename source", ErrorOrigin. host, false);
            }
            original = records[i];
            i = i + 1;
        }
        entries.push(StatusEntry { index: record[0..1], worktree: record[1..2], path: record[3..], original_path: original });
    }
    return entries;
}

/// 返回指定数量的提交；字段使用 NUL 分隔。
pub task log(repo: Repository, count: int) -> Commit[] | Error {
    if count < 1 {
        return Error::make("git", "Git log count must be positive", ErrorOrigin.user, false);
    }
    let output = checked(run(repo, ["log", "-z", "--max-count=" + string(count), "--format=%H%x00%P%x00%an%x00%at%x00%s"]).await ?) ?;
    let fields = output.split("\0");
    let commits: Commit[] = [];
    let i = 0;
    loop {
        if i + 4 >= fields.len() {
            break;
        }
        commits.push(Commit { oid: fields[i], parents: fields[i + 1], author: fields[i + 2], timestamp: fields[i + 3], subject: fields[i + 4] });
        i = i + 5;
    }
    return commits;
}

/// 查询工作区或索引差异。
pub task diff(repo: Repository, staged: bool, paths: string[]) -> CommandResult | Error {
    let args = ["diff", "--no-ext-diff", "--no-textconv"];
    if staged {
        args.push("--cached");
    }
    args.push("--");
    for path in paths {
        args.push(git_path(path));
    }
    return run(repo, args).await ?;
}

/// 仅暂存明确列出的路径；空列表拒绝执行。
pub task add(repo: Repository, paths: string[]) -> CommandResult | Error {
    if paths.len() == 0 {
        return Error::make("git", "Git add requires explicit paths", ErrorOrigin.user, false);
    }
    let args = ["add", "--"];
    for path in paths {
        args.push(git_path(path));
    }
    return run(repo, args).await ?;
}

/// 提交当前索引，不隐式暂存任何文件。
pub task commit(repo: Repository, message: string) -> CommandResult | Error {
    return run(repo, ["commit", "-m", message]).await ?;
}

/// 初始化普通或裸仓库。
pub task init(repo: Repository, bare: bool) -> CommandResult | Error {
    let args = ["init"];
    if bare {
        args.push("--bare");
    }
    return run(repo, args).await ?;
}

/// 在配置的工作目录内克隆到指定目标。
pub task clone(repo: Repository, remote: string, destination: string) -> CommandResult | Error {
    return run(repo, ["clone", "--", operand(git_path(remote)) ?, operand(git_path(destination)) ?]).await ?;
}

/// 列出本地分支名称。
pub task branches(repo: Repository) -> string[] | Error {
    return checked(run(repo, ["for-each-ref", "--format=%(refname:short)", "refs/heads/"]).await ?) ?.trim().split("\n").filter(fn(name: string) {
        return name != "";
    });
}

/// 创建或切换分支。
pub task switch(repo: Repository, branch: string, create: bool) -> CommandResult | Error {
    let args = ["switch"];
    if create {
        args.push("-c");
    }
    args.push(operand(branch) ?);
    return run(repo, args).await ?;
}

/// 删除已合并分支；force 为 true 时允许删除未合并分支。
pub task delete_branch(repo: Repository, branch: string, force: bool) -> CommandResult | Error {
    return run(repo, ["branch", if force { "-D" } else { "-d" }, operand(branch) ?]).await ?;
}

/// 返回远程名称和地址。
pub task remotes(repo: Repository) -> CommandResult | Error {
    return run(repo, ["remote", "-v"]).await ?;
}

/// 添加一个远程。
pub task add_remote(repo: Repository, name: string, url: string) -> CommandResult | Error {
    return run(repo, ["remote", "add", operand(name) ?, operand(url) ?]).await ?;
}

/// 移除远程配置。
pub task remove_remote(repo: Repository, name: string) -> CommandResult | Error {
    return run(repo, ["remote", "remove", operand(name) ?]).await ?;
}

/// 抓取指定远程，不自动清除远程跟踪分支。
pub task fetch(repo: Repository, remote: string) -> CommandResult | Error {
    return run(repo, ["fetch", operand(remote) ?]).await ?;
}

/// 只允许快进拉取；分叉交由调用方显式选择 merge 或 rebase。
pub task pull(repo: Repository, remote: string, branch: string) -> CommandResult | Error {
    return run(repo, ["pull", "--ff-only", operand(remote) ?, operand(branch) ?]).await ?;
}

/// 推送指定引用；不隐式强推。
pub task push(repo: Repository, remote: string, refspec: string) -> CommandResult | Error {
    return run(repo, ["push", operand(remote) ?, operand(refspec) ?]).await ?;
}

/// 合并引用，冲突以非零退出码保留。
pub task merge(repo: Repository, revision: string) -> CommandResult | Error {
    return run(repo, ["merge", "--no-edit", operand(revision) ?]).await ?;
}

/// 在指定基线上变基，不启动交互编辑器。
pub task rebase(repo: Repository, upstream: string) -> CommandResult | Error {
    return run(repo, ["rebase", operand(upstream) ?]).await ?;
}

/// 继续或中止 merge/rebase；拒绝任意操作字符串。
pub task resolve_operation(repo: Repository, operation: string, abort: bool) -> CommandResult | Error {
    if operation != "merge" && operation != "rebase" {
        return Error::make("git", "Expected merge or rebase", ErrorOrigin.user, false);
    }
    return run(repo, [operation, if abort { "--abort" } else { "--continue" }]).await ?;
}

/// 按显式模式重置，hard 会丢弃已跟踪文件改动。
pub task reset(repo: Repository, revision: string, mode: ResetMode) -> CommandResult | Error {
    let flag = "--mixed";
    match mode {
        ResetMode.soft => {
            flag = "--soft";
        }
        ResetMode.mixed => {}
        ResetMode.hard => {
            flag = "--hard";
        }
    }
    return run(repo, ["reset", flag, operand(revision) ?]).await ?;
}

/// 清理明确路径；默认由调用方选择预览，绝不隐式包含忽略文件。
pub task clean(repo: Repository, paths: string[], dry_run: bool, directories: bool) -> CommandResult | Error {
    if paths.len() == 0 {
        return Error::make("git", "Git clean requires explicit paths", ErrorOrigin.user, false);
    }
    let args = ["clean", if dry_run { "-n" } else { "-f" }];
    if directories {
        args.push("-d");
    }
    args.push("--");
    for path in paths {
        args.push(git_path(path));
    }
    return run(repo, args).await ?;
}

/// 保存工作区改动；是否包含未跟踪文件由调用方指定。
pub task stash(repo: Repository, message: string, untracked: bool) -> CommandResult | Error {
    // stash 内部清理会再次解析 pathspec；这里没有用户路径，不能继承 literal 模式。
    let args = ["--no-literal-pathspecs", "stash", "push"];
    if untracked {
        args.push("--include-untracked");
    }
    args.push("-m");
    args.push(message);
    return run(repo, args).await ?;
}

/// 列出保存的改动。
pub task stashes(repo: Repository) -> CommandResult | Error {
    return run(repo, ["stash", "list"]).await ?;
}

/// 应用指定 stash；成功后是否移除由 pop 指定。
pub task apply_stash(repo: Repository, reference: string, pop: bool) -> CommandResult | Error {
    return run(repo, ["stash", if pop { "pop" } else { "apply" }, operand(reference) ?]).await ?;
}

/// 列出工作树的机器可读记录。
pub task worktrees(repo: Repository) -> CommandResult | Error {
    return run(repo, ["worktree", "list", "--porcelain", "-z"]).await ?;
}

/// 从现有引用添加工作树。
pub task add_worktree(repo: Repository, path: string, revision: string) -> CommandResult | Error {
    return run(repo, ["worktree", "add", "--", operand(git_path(path)) ?, operand(revision) ?]).await ?;
}

/// 移除工作树；不强制丢弃未提交改动。
pub task remove_worktree(repo: Repository, path: string) -> CommandResult | Error {
    return run(repo, ["worktree", "remove", "--", operand(git_path(path)) ?]).await ?;
}
