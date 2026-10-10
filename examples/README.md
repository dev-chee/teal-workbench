# Teal 使用示例

本目录按 [Teal 顶层架构](../docs/TEAL-ARCH.md) 的能力层级组织：

- [`core`](core)：不依赖本机平台服务或模型供应商的语言、数据与纯 CPU task；
- [`host`](host)：控制台、文件、进程、环境、安全 Host 和 dylib；
- [`agent`](agent)：源码 Agent、模型循环、用户交互、会话、记忆和协作；
- [`packages`](packages)：内置扩展包的协议面与客户端用法，按所需资源计入 Tier Host；
- [`showcases`](showcases)：组合多个 Tier 的工作台与 DHE 完整场景，不作为单项能力的唯一覆盖证据。

每个示例的文件头说明归属、前提和关键边界。示例只在语义不直观处添加注释，不逐句
复述代码。所有 `.tl`、`.teal`、`.rs` 和示例 `.toml` 都登记在
[`manifest.json`](manifest.json)；[`coverage.json`](coverage.json) 以稳定能力 ID 记录当前已实现
的 Teal 编程面及其验证等级。

## Tier Core

Core 示例可在没有 Agent 服务、模型认证和本机扩展的环境中学习。CLI 只用于装载并观察结果；
示例中的 `print` 不表示语言语义依赖某种终端实现。

```powershell
cargo run -p teal -- run examples/core/literals_and_variables.tl
cargo run -p teal -- compile examples/core/type_syntax.tl
cargo run -p teal -- run examples/core/functions_and_closures.tl --profile target/functions.pb.gz
cargo run -p teal -- run examples/core/task_main.tl
```

| 示例 | 主要内容 |
| --- | --- |
| `literals_and_variables.tl` | 注释、字面量、转义与插值、声明、推演、未初始化和 shadow |
| `operators_and_conversions.tl` | 算术、比较、逻辑、赋值和类型转换 |
| `control_flow.tl` | 块、两种 `if`、循环、标签、`break`、`continue` 与 `for` |
| `collections_and_slices.tl` | 数组、映射、索引、切片、修改和遍历 |
| `functions_and_closures.tl` | 命名函数、递归、函数值、匿名函数和捕获 |
| `classes.tl` / `enums_and_match.tl` | 数据结构、构造、字段、判别式、解构和穷尽匹配 |
| `errors.tl` / `statics.tl` | 错误传播与模块静态状态 |
| `imports_and_namespaces.tl` | 导入、别名、公开导出、重导出和文件私有入口 |
| `type_syntax.tl` / `attributes.tl` | 类型表达式与自定义 Attribute |
| `attribute_targets.tl` | 属性目标类别、声明式投影前缀与同一实例身份 |
| `traits_and_impl.tl` | 固有方法、Trait、`Self`、UFCS 和 `impl Trait` |
| `builtin_functions.tl` / `builtin_methods.tl` | 全部 Core 内建函数和方法 |
| `bindings_and_unicode.tl` | 丢弃绑定、Unicode scalar 索引和 UTF-8 字节偏移 |
| `lint_control.tl` / `invalid/lint_denied.tl` | 文件级与声明级检查项级别、分组、`_` 抑制和 deny 诊断 |
| `compiler_directives.tl` | 文件头构建标量，以及导入文件只校验并产生 `ignored-compiler-option` |
| `runtime_error_kinds.tl` | 可由 `try` 捕获的全部内建运行时错误种类 |
| `tasks.tl` / `task_main.tl` / `task_control.tl` | CPU task、`task main`、`Task<T>`、`.await`、收集、取消、状态和失败隔离 |
| `terminal_ui.tl` / `terminal_ui_catalog.tl` | `std/ui.tl` 的全部公开类型、组件、事件与编码接口 |

`modules/` 是导入辅助模块，`parser_only/` 明确表示只保证解析的边界形式，`invalid/` 保存必须
持续失败的负例：未初始化值求值、未等待任务和被 `deny` 提升的检查项。它们不是遗漏验证的
普通入口。

## Tier Host

以下命令要求已按 [跨仓兼容矩阵](../docs/TEAL-LANG-COMPAT.md) 组装语言 sibling binaries 和标准包。Host 示例区分实现支持、函数注册、操作授权和动态资源范围。启用 Extension 或登记
函数不会自动开放文件、环境、进程或动态库资源。

```powershell
cargo build --locked --workspace --bins
$tealExampleCli = (Resolve-Path target/debug/teal.exe).Path
$permissionFile = "C:/path/to/permissions.toml"

"Ada" | & $tealExampleCli run examples/host/console_and_arguments.tl -- alpha beta
$env:TEAL_EXAMPLE_VALUE = "demo"
& $tealExampleCli run examples/host/system.tl `
  --permission-file $permissionFile

$tealExampleParent = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $tealExampleParent | Out-Null
& $tealExampleCli run examples/host/filesystem_and_crypto.tl `
  --permission-file $permissionFile `
  -- (Join-Path $tealExampleParent "scratch")
Remove-Item -LiteralPath $tealExampleParent

& $tealExampleCli run examples/host/process.tl `
  --permission-file $permissionFile
```

`json.tl` 覆盖 `std/json.tl` 的全部公开读取接口；`system.tl` 覆盖平台、可执行文件、时钟与
名称级环境读取；`filesystem_and_crypto.tl` 在新建临时目录中覆盖完整文件接口与 SHA-256；
`process.tl` 使用当前 Teal 可执行文件覆盖命令、运行、启动、查询、等待、终止、解析与查找，
不依赖 shell。

`safe_host_declarations.tl` 由同目录的 Rust `host_registry.rs` 提供实现。该文件是嵌入宿主示例源码；当前工作台根包没有登记 `host_registry` Cargo example，需在消费 `teal-lang` 的宿主 package 中登记并运行。

它先证明缺少授权会在加载阶段失败且没有副作用，再以完全匹配的签名和 Capability 执行。
`dylib_declarations.tl` 只做编译检查；真实动态库执行仍依赖平台 C ABI、库文件、符号和
显式授权，不能由声明编译成功推断为已运行。

`packages/` 覆盖随发行提供的 `git`、`http`、`oauth` 和 `feishu` 包。纯协议面不需要外部
服务：`http_protocol.tl` 展示 URL 编码、请求体和 JSON 构造，`oauth_protocol.tl` 展示 token
解码、错误分支和公开配置形状。发送请求、访问远端仓库、PKCE 授权、回环回调和文档导入需要
`network.http`、`process.spawn`、`network.oauth_callback` 等具体授权，由对应的 check 入口
和 语言仓库的 [`general_packages.rs`](https://github.com/dev-chee/teal-lang/blob/v0.1.0/crates/teal-runtime/tests/general_packages.rs) 宿主夹具验证，不属于离线示例输出。

## Tier Agent 与 Core task

`class_task_parallel.tl` 与 `task_status.tl` 是不含模型调用的 `class`/普通 `task` 示例，
不绑定模型供应商，可离线运行：

```powershell
cargo run -p teal -- run examples/core/class_task_parallel.tl
cargo run -p teal -- run examples/core/task_status.tl
```

其余 Agent 示例在仓库门禁中做真实编译和确定性协议验证，但不会自动发送模型请求：

| 示例 | 运行前提与边界 |
| --- | --- |
| `task_main_model.tl` | `task main` 直接等待 Agent task；需要 Codex 认证，自动门禁只编译 |
| `codex_graph_review.tl` | 两个独立 Codex Agent 并行审查；真实运行需要认证 |
| `deepseek_review.tl` | DeepSeek 活动 profile、Responses 模型调用与类型化完成 |
| `model_options.tl` | `self.options`、限制、Context 与类型化完成结果 |
| `user_interaction.tl` | 文本问答、稳定候选 ID、跳过和回复文件恢复；回答不授权 |
| `model_questions.tl` | 模型自主提问、检查点和回答后继续 |
| `sessions_and_memory.tl` | 命名会话、显式 Context、MemoryStore 召回和候选写入 |
| `model_team.tl` | 受控 Agent 工厂及消息、等待、cursor 确认、列举和打断工具 |

真实 Codex 模型示例可在明确接受外部请求后运行：

```powershell
cargo run -p teal -- auth login codex --method codex-cli
cargo run -p teal -- compile examples/agent/model_options.tl --runtime-profile agent
cargo run -p teal -- run examples/agent/model_options.tl
```

模型返回必须通过 `__teal_complete` 满足结果契约。认证、用户回答、工具批准和 Capability 授权
是互相独立的协议；持久恢复不会自动重放 Unknown 外部效果。

DeepSeek 示例先执行 `teal auth login deepseek`，再执行
`teal run examples/agent/deepseek_review.tl`。可使用 `--env-key DEEPSEEK_API_KEY`
引用已有环境变量，或用 `--stdin-key` 从标准输入提交密钥。登录校验会访问官方 `/models`，
示例运行会产生真实模型请求；自动门禁只进行编译和本地协议验证。

## Cross-tier Showcases

- `showcases/bench` 包含配置、面板、扩展和可独立运行的持久源码 Agent SDLC 工作模式；例如
  `:source! examples/showcases/bench/ui-demo.tl` 与
  `:extension load examples/showcases/bench/sdlc`。
- `showcases/codex-report` 组合定时启动器、native Git 证据采集和 DeepSeek Agent 概要；进程
  调用与可恢复 Agent 分为两个 Teal 入口，报告和成功基线保存在显式授权的外部目录。
- `showcases/dhe/dhe.tl` 是 DHE 唯一入口，组合源码身份、构建确认、当前 SnapshotManifest、
  制品哈希、双次 Player 启动和可选只读复核。Jenkins Replay/Rebuild、共享目录下载、解压和
  游戏画面判断仍由操作者完成，但每个确认都绑定当前 run/effect 并写入追加式事件报告。
  默认源仓库是 `E:/HybridCLR` 与 `E:/SS/worktrees`；Player、日志、制品和部署目录必须通过
  `--run-root`、`--player`、`--log` 等参数指定，脚本不使用机器特定的 `G:` 或个人目录。
  运行方式为 `release`、`inspect`、`demo`、`player`、`all` 和 `resume`；发布命令包括
  `begin`、`baseline-success|<build>|<commit>`、
  `hotfix-success|<build>|<baseline_version>|<commit>`、
  `artifacts-ready|<archive>|<player_dir>|<source>`、`verify-player`、
  `failure-artifacts|<symbols>|<build_inputs>`、`status`、`reconcile|<effect>|<result>` 和
  `review`。报告保存在 `<run-root>/dhe-evidence.md`，包含可读 Markdown 和 `DHE-EVENT` 行；
  未完成的外部效果会进入 `needs_attention`，不会在重启后自动重放。

DHE 入口先做无副作用编译检查：

```powershell
cargo run -p teal -- compile examples/showcases/dhe/dhe.tl --runtime-profile agent
```

真实 `release`、`inspect`、`demo`、`player` 或 `all` 运行需要按脚本帮助显式授予实际目录、
`git`、Python 和 Player。外部 Jenkins、下载、Player、部署以及真实模型不属于自动示例门禁；
`dhe-build-inputs.zip` 需要操作者先解压，脚本只验证提供的 manifest、目录和哈希证据。

## 覆盖与验证

`manifest.json` 记录每个资产的 Tier、验证模式、预期输出、stdout/stderr 期望、stdin、退出
状态、选项和参数；预期结果必须按语义复核，不能从一次当前输出盲目重录。`coverage.json`
中每项能力必须至少由一个同 Tier、非 showcase 的示例覆盖；`verification` 区分 `run`、
`check`、确定性 `harness`
和需要外部服务的 `external`。

```powershell
cargo test -p teal-workbench-automation shipped_bench_examples_prepare_and_execute
```

语言示例 manifest 与 coverage 的完整执行属于语言发行验收；当前工作台只登记工作台扩展场景测试。Host 缺少 Capability 或资源时必须在副作用前拒绝。编译通过不等于真实模型、平台动态库、终端交互、
浏览器服务、远程运行或外部业务效果已经通过。
