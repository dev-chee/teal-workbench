# teal-workspace

为编码工作流提供版本化文件快照、修改提案和修改记录。工作区状态与 VM 状态分开管理，脚本恢复不会自动撤销文件或外部命令的副作用。版本检查和操作日志不提供针对任意外部进程的跨文件原子事务。

## 工作区、分发与维护工具

`teal-workspace` 提供 Agent 使用的文本快照和变更记录；打开文档事务与文件操作由 `teal-workbench-core` 的共享协调器负责。

本仓库的 `runtime/packages` 只包含 `agents`、`bench`、`feishu`、`git`、`http`、`oauth` 六个扩展包。`std`、`builtins.tli` 及其生成验证属于 `teal-lang`。`cargo xtask dist` 构建九个工作台二进制；完整发行显式提供版本一致的语言 checkout，合并标准包并构建语言 sibling binaries，见 [维护工具说明](../../tools/teal-xtask/README.md)。

`cargo xtask workspace verify` 以当前 Cargo metadata 为准，不依赖 sibling `workspace.toml`。rustdoc 产品清单覆盖本仓库 `crates/` 下的 13 个 library target；语言 API 文档由语言仓库独立构建。

`cargo xtask test-gate run --spec target/test-gate/<task-id>/gate.toml` 按规范启动 argv 命令。规范须显式列出受测文件，runner 不推断范围；日志、前后 SHA-256 指纹、`run.json` 与 `run-summary.md` 保存于该任务目录。相同规范和冻结输入的 PASS 可复用，命令失败阻断后续项，输入变化使结果为 `STALE`。

修改范围、回归矩阵和提交要求见 [AGENTS.md](../../AGENTS.md)；跨仓持久格式和验收状态见 [兼容矩阵](../../docs/TEAL-LANG-COMPAT.md)。
