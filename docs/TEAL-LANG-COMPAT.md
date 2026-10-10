# teal-lang 跨仓兼容矩阵

这份矩阵是 `teal-workbench` 消费语言仓库的冻结记录。工作台不复制语言实现，也不使用跨仓 `path` 依赖；每次升级必须先在干净语言 checkout 固定提交，再运行两仓库门禁。

| 项目 | 当前联调基线 | 发布目标 | 工作台消费方式 |
| --- | --- | --- | --- |
| Git 源 | `https://github.com/dev-chee/teal-lang.git` | `v0.1.0`（由 `423da3783d45dfbb09a90b1bcddc3f89d8296b28` 创建） | 初始联调锁定 `rev = 11491542dbf56393a91df53ce68f2a26ec2efad1`；全部门禁通过后切换固定 tag |
| `teal-runtime` / 根 `teal` | `0.1.0` | `0.1.0` | `ffi` 按工作台 crate 需要启用；根派发器由工作台构建 |
| `teal-abi` | 程序格式 59 | 程序格式 62 | 语言仓库独占生成和验证，工作台只消费编解码 API |
| `teal-vm` | VM snapshot 22 | VM snapshot 25 | 语言仓库独占快照实现与覆盖率 |
| Agent Store | schema 20 | schema 22 | 工作台不迁移旧数据库；版本不匹配必须拒绝 |
| `teal-config` | CONFIG_VERSION 14 | CONFIG_VERSION 14 | 工作台只使用配置 API |
| Builtin interface | format 7 | format 7 | `builtins.tli` 由语言仓库生成和验证 |
| Segment protocol | version 1 | version 1 | 由语言运行时提供 |
| MCP protocol | `2025-06-18` | `2025-06-18` | 由语言模型/Agent crate 提供 |

## 版本契约

- `teal-workbench` 的 Cargo.lock 必须记录 HTTPS 源和唯一语言提交；升级只允许改变语言源、提交及由此直接需要的依赖解析。
- 发行构建要求语言 checkout 干净、HEAD 与锁文件提交一致。`cargo xtask dist --language-root PATH` 会重新检查这一点，构建语言 sibling binaries，并把语言 `std` 与工作台扩展 catalog 合并到发行目录。
- `teal-lang` 独占 `builtins`、`knowledge`、ABI/VM/Host coverage、语言 rustdoc 和语言二进制分发；工作台 xtask 只维护工作台 package、扩展资源、分发清单和当前 workspace 文档。
- 程序、快照、Store、配置、Builtin 或协议格式变化不得由工作台兼容层静默接受。发现不兼容时，语言仓库必须先提升相应版本并增加旧载荷拒绝测试，再更新本表和工作台锁文件。

## 验收记录

- 语言发布 worktree：`/home/chi/teal-lang-workbench-release`，基线 `423da3783d45dfbb09a90b1bcddc3f89d8296b28`。
- 工作台联调 worktree：当前仓库分支 `codex/workbench-stabilization`。
- 运行 `cargo metadata --locked` 前必须确认 Cargo.lock 的 source 不再是 `file:///D:/teal-lang`。
- 完整 gate 需要记录两个仓库 HEAD、语言 revision/tag、命令、退出码和 `target/test-gate/<task-id>/` 报告路径；输入变化后旧结果标记为 `STALE`。
