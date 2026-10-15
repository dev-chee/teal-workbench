# teal-lang 跨仓兼容矩阵

工作台统一锁定语言提交 `963595f494a1dd1cf86b591cc12f8d3162325f1d`，Git 源为
`https://github.com/dev-chee/teal-lang.git`。不保留旧 Rust 包兼容壳，也不提交跨仓 path 依赖。
该提交尚未由本次任务推送；本地联调通过按命令设置 Git `insteadOf` 映射读取同一提交。

| 边界 | 本次契约 |
| --- | --- |
| 本机服务 | `teal_host::{browser,computer,web}`，三个可选 feature 默认关闭；工作台显式启用 |
| 协议与工具 | 根语言包提供 `teal-fmt/lsp/dap`，工作台保留 LSP client 与产品 `teal` 入口 |
| 共享基础能力 | `teal_utils::{text,image}`、`teal_runtime::debug::parse_expression`、Security Host 审计桥 |
| 执行层级 | `RuntimeTier`：`embedded/hosted/agentic`；层级不授予 Permission |
| 错误 | 源码使用 `T | Error` 与 `?`；未知外部效果不能自动重放 |
| DAP | launch 字段 `tier`，隐藏参数 `--tier`；旧 `runtimeProfile` 拒绝 |
| 程序 / VM 快照 / Agent Store | 69 / 28 / 27；由语言包校验版本，旧文件保留并拒绝恢复 |
| 配置 / Package / Catalog | 15 / 7 / 6 |
| 工作台脚本清单 / runtime lock | 2 / 9；旧 profile 字段与旧层级值拒绝 |

新运行在 prepare 之前解析并冻结模型绑定（包括压缩模型）；恢复使用已存绑定，不从当前配置重新选模型。
默认 Agent 启动需要已配置的模型选择；模型调用仍由权限、联网模式与认证独立控制。

`cargo xtask dist --language-root PATH` 要求语言 checkout 干净且 HEAD 等于锁定提交，
从该 checkout 构建语言二进制并合并 std 包。工作台扩展包装继续使用 `std.bench.*`，
其他宿主通过通用 Host 注册函数选择非标准模块名。

验证记录及已知基线限制见 [跨仓验证记录](TEAL-LANG-CROSS-REPO-GATE.md)。
