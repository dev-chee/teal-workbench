@edition("2026");

import "git/git.tl" as git;

// 在当前目录查询；运行时需要 process.execute 与 Git 可执行路径授权。
task main() {
    let repo = git::repository(".");
    for entry in git::status(repo).await {
        print(entry.index, entry.worktree, entry.path);
    }
}
