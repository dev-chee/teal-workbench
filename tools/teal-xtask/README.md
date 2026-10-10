# teal-xtask

Teal 工作台仓库维护命令：分发组装、中文 rustdoc、逻辑 workspace 校验和可恢复的最终门禁调度。通过根目录的 `cargo xtask <command>` 调用，不作为产品运行时 API 发布。

## 命令

- `cargo xtask dist` 构建并组装只包含 workbench 工具和 workbench runtime 的发行包。
- `cargo xtask dist --language-root PATH` 在语言仓库 checkout 干净且与锁定 revision 一致时，额外构建 `teal-c`、`teal-test`、`teal-auth`、`teal-model`、`teal-audit`，并合并 `std`、`catalog-core` 与 workbench runtime。
- `cargo xtask rustdoc audit|files|build|verify` 只检查本仓库 `crates/` 下的公开库。
- `cargo xtask workspace verify` 校验当前 Cargo workspace 和 workbench runtime package catalog。
- `cargo xtask test-gate run --spec target/test-gate/<task-id>/gate.toml [--force]` 按显式规范执行最终门禁。

`teal-lang` 的 builtins、knowledge、ABI/VM/Host coverage 和语言仓库发行工具由 `teal-lang` 仓库负责。本仓库不读取 sibling `workspace.toml`，也不从工作台源码生成语言制品。跨仓发行必须使用 `--language-root`，并且该 checkout 必须干净、HEAD 必须等于 Cargo 锁定的 `11491542dbf56393a91df53ce68f2a26ec2efad1`。

覆盖率和完整 workspace 门禁不进入默认验证矩阵；验证范围按根 `AGENTS.md` 和本次改动确定。`teal-xtask` 是 `publish = false` 的维护工具。
