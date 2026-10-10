# teal-lang 与 teal-workbench 跨仓验收记录

本次输入在验收期间冻结如下：

- `teal-lang` 发布 worktree：`/home/chi/teal-lang-workbench-release`，HEAD `f31bd500f7b8c0e5b433cb7dfb427d11e98c83bb`，本地 tag `v0.1.0`。
- `teal-workbench`：HEAD `0a78c99`，分支 `codex/workbench-stabilization`。
- 原始 `/home/chi/teal-lang` checkout 未作为发布输入，也未被格式化、暂存或提交。

## teal-lang

以下命令在上述发布候选 worktree 执行并返回 0：

```text
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked -- --test-threads=1
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo xtask builtins verify
cargo xtask knowledge verify
cargo xtask rustdoc audit
cargo check -p teal-core --target wasm32-unknown-unknown
cargo xtask coverage all
cargo run -p teal-lang --bin teal -- --help
cargo run -p teal-model --bin teal-auth -- --help
cargo run -p teal-runtime --bin teal-test -- --help
```

最终 coverage 报告写入 `target/llvm-cov/all.export.json`，摘要为：VM 行 82.65%/区域 84.08%、VM debug target 行 86.00%/区域 82.23%、Host 行 83.25%/区域 80.47%；授权关键模块行 95.00%/区域 95.77%。

## teal-workbench

已返回 0 的检查包括 `cargo metadata --locked`、`cargo test -p teal-workbench-core --lib`、`cargo test -p teal-lsp-client --lib`、`cargo test -p teal-web --lib`、`cargo test -p teal-xtask`、受影响包 clippy、`cargo xtask workspace verify` 和 `cargo xtask rustdoc audit`。

根工作区的完整格式检查仍报告原有的 130 处跨文件格式差异；完整 bench/workspace 构建还受到环境缺少可用 `cc`（`libffi-sys` 的 `-print-multi-os-directory`）及 `secret-service` runtime feature 的阻断。这些结果没有被记为通过，也没有改动无关文件来掩盖它们。

工作台依赖继续锁定可从 GitHub 获取的 `rev = 11491542dbf56393a91df53ce68f2a26ec2efad1`。本地 `v0.1.0` 已通过发布门禁，但 `git ls-remote https://github.com/dev-chee/teal-lang.git refs/tags/v0.1.0` 尚无结果；远端 tag 发布后再将 Cargo manifest 和 lock source 从 rev 切换到 tag。
