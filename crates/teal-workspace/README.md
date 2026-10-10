# teal-workspace

为编码工作流提供版本化文件快照、修改提案和修改记录。工作区状态与 VM 状态分开管理，脚本恢复不会自动撤销文件或外部命令的副作用，版本检查与操作日志也不提供针对任意外部进程的跨文件原子事务。

本 README 后续章节说明边界与维护工具。

## 实现细节

### 工作区、分发与维护工具

`teal-workspace` 提供 Agent 使用的版本化文本快照和变更记录，不承担工作台缓冲区事务或 Git 仓库整体回滚。

`runtime/packages` 是工作台扩展包的唯一源码；构建把每个 module 解析为版本化二进制 IR，发行目录存在时必须重新预编译并与内嵌 package digest 一致，目录完全缺失时才整体使用内嵌制品。`runtime/README.md` 描述资源目录；`cargo xtask dist` 组装 workbench 工具和 runtime，生成发行清单与摘要。需要语言标准包和语言工具的完整发行包必须显式传入 `--language-root PATH`，由同一命令校验锁定 revision 后合并。

语言标准包和 `runtime/packages/std/builtins.tli` 由 `teal-lang` 负责生成与校验；workbench 只在 `dist --language-root PATH` 的跨仓发行模式复制已经生成的制品。

`teal-xtask` 提供基于显式规范与内容指纹的 `test-gate run` 调度、当前 workbench 的中文 rustdoc audit/build/files/verify，以及 workspace verify。仓库验证范围由根 `AGENTS.md` 规定，调度器不推断影响范围，也不能把局部编译或旧 target 产物描述为完整门禁。

rustdoc 产品清单覆盖当前 workbench `crates/` 下全部 Cargo library target；每次审计、构建或验证前与当前工作区目标集合比较，新增库遗漏时直接失败。`tools/` 下的测试辅助库不进入产品 API 站点。

#### 维护工具契约

`cargo xtask test-gate run --spec target/test-gate/<task-id>/gate.toml` 按声明顺序直接启动 argv 命令，规范必须放在对应任务目录中并显式列出受测文件，runner 不推断影响范围。每轮输出 `run.json`、`run-summary.md`、前后 SHA-256 指纹和逐项日志；相同规范、输入、命令、工具链及相关 Cargo/Rust 环境下的 PASS 会直接复用，`--force` 可显式重跑，命令失败时后续门禁标记为 `BLOCKED`，运行期间输入变化时总结果为 `STALE`。中文 rustdoc 的 audit 会在构建与验证前检查公开单元的说明语言。`teal-xtask` 是 `publish = false` 的维护工具，不作为产品运行时 API 发布。

`teal-workspace` 提供版本化文件快照、修改提案和修改记录；工作区状态与 VM 状态分开管理，脚本恢复不会自动撤销文件或外部命令的副作用，版本检查与操作日志也不能提供针对任意外部进程的跨文件原子事务。

### 历史材料与维护入口

仓库修改范围、回归矩阵和提交要求只由根 [AGENTS.md](../../AGENTS.md) 维护，本文不复制门禁流程。历史审查不再作为当前规范或仓库文档保留；当前机制以源码、测试和本文为准。
