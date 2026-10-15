@edition("2026");

import "git/git.tl" as git;

// 在当前目录查询；运行时需要 process.execute 与 Git 可执行路径授权。
task application() -> none | Error {
    let repo = git::repository(".");
    for entry in git::status(repo).await ? {
        print(entry.index, entry.worktree, entry.path);
    }
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
