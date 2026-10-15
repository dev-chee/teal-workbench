# teal-workbench

Teal 的编辑器与 Agent 工作台。仓库提供 Windows 优先的模态终端界面、共享编辑与工作区服务、Teal 自动化，以及对语言仓库协议工具、本机浏览器和桌面服务的集成。使用 `teal bench` 启动工作台，也可以在其他宿主中复用各个库。

本仓库从 Teal 拆出，负责产品交互和应用集成；语言、编译器、VM、标准库以及模型、权限、审计和持久 Agent 运行时由 `teal-lang` 提供。工作台通过依赖使用这些能力，业务流程由 Teal 源码程序表达。

## 主要能力

- **编辑与工作区**：共享缓冲区、多选择、事务编辑、分支撤销、文件树、搜索、窗口布局和会话恢复。
- **语言工具**：语法高亮、通用 LSP 客户端，并消费语言仓库提供的 LSP、DAP 和源码格式化工具。
- **源码 Agent**：工作台把用户输入、工具结果和打开文档状态交给持久 Teal 程序，呈现会话、问答、批准、运行状态和交付证据。
- **自动化与扩展**：通过类型化 Host、能力声明和信任边界接入配置、文法、命令、面板与后台任务。
- **外部交互**：结构化文本网页、受管 Chromium CDP 会话和 Windows UI Automation；外部动作使用独立授权与审计。

默认 Agent 入口是 [`runtime/agents/default.tl`](runtime/agents/default.tl)。Chat、Task、目标和交付规则由源码程序及 [`agents`](runtime/packages/agents) 扩展包执行，Bench 负责交互与呈现。相同程序也可由无界面宿主运行。

## 从源码开始

### 构建前提

- Rust **1.98.1 或更高版本**，使用 Rust 2024 edition，并具备目标平台所需的原生编译与链接环境。
- Git，以及 [`Cargo.lock`](Cargo.lock) 锁定的 `teal-lang` 源码提交。
- 运行终端工作台时使用支持交互输入和终端控制序列的终端。

[`Cargo.toml`](Cargo.toml) 中的语言依赖使用 GitHub HTTPS 源和固定 `rev`，[`Cargo.lock`](Cargo.lock) 固定实际提交。发布 tag 只在跨仓门禁全部通过后使用；格式号、消费 feature 和验证状态见 [跨仓兼容矩阵](docs/TEAL-LANG-COMPAT.md)。跨仓库依赖不使用 `path`。

### 构建并打开文件

在仓库根目录执行，以下示例使用 PowerShell：

```powershell
cargo build --locked --workspace --bins
.\target\debug\teal.exe bench README.md --safe-mode --offline
```

`--safe-mode` 跳过启动脚本、Agent、内置文法和会话恢复；`--offline` 禁止远程模型、网络、遥测导出和扩展外部副作用。这个入口适合先体验文件编辑。

打开工作区并使用正常启动配置：

```powershell
.\target\debug\teal.exe bench .
```

文件路径打开 Editor 窗口，目录路径打开 Agent 工作窗口。模型运行需要配置认证和模型预设；配置、键位、权限与扩展操作见 [工作台手册](docs/TEAL-BENCH-MANUAL.md)。源码构建也可以直接运行 `target/debug/teal-bench.exe`。

### 命令入口

根命令 `teal` 负责把参数派发到同一目录中的独立二进制；每个工具拥有自己的参数解析和进程生命周期。具体参数使用 `teal <command> --help` 查看。

| 根命令 | 独立二进制 | 所属 package |
| --- | --- | --- |
| `teal bench` | `teal-bench` | `teal-bench` |
| `teal run` | `teal-run` | `teal-workbench-services`，需启用 `tools` feature |
| `teal config` / `teal perm` | `teal-config` / `teal-perm` | `teal-bench` |
| `teal knowledge` | `teal-knowledge` | `teal-workbench-automation` |
| `teal fmt` | `teal-fmt` | 语言仓库根包 `teal-lang` |
| `teal lsp` | `teal-lsp` | 语言仓库根包 `teal-lang` |
| `teal dap` | `teal-dap` | 语言仓库根包 `teal-lang` |
| `teal compile` / `teal test` | `teal-c` / `teal-test` | `teal-lang` 的 `teal-runtime` |
| `teal auth` / `teal model` | `teal-auth` / `teal-model` | `teal-lang` 的 `teal-model` |
| `teal audit` | `teal-audit` | `teal-lang` 的 `teal-security` |

上述 workspace 构建覆盖本仓库的二进制，`teal-lang` 的工具需另行构建并与工作台工具放在同一目录、保持版本一致。仅构建根包 `teal` 会得到派发器；它不会自动构建其余工具。

## 仓库结构

各 package 的 README 说明其实现、公开接口和恢复契约。

| 位置 | 职责 |
| --- | --- |
| [`src`](src) | 根 `teal` 命令与独立工具派发。 |
| [`teal-bench`](crates/teal-bench/README.md) | 终端生命周期、事件循环、窗口呈现、应用装配，以及配置和权限命令。 |
| [`teal-workbench-core`](crates/teal-workbench-core/README.md) | 文档、选择、事务、撤销、布局、窗口、恢复和可选 `syntax` 模块。 |
| [teal_utils::text](https://github.com/dev-chee/teal-lang/blob/main/crates/teal-utils/README.md) | 不可变文本快照与 UTF-8 / UTF-16 坐标转换。 |
| [`teal-workbench-services`](crates/teal-workbench-services/README.md) | 文件树、搜索、进程、剪贴板、runtime lock、工具、观测和工作区服务。 |
| [`teal-workbench-automation`](crates/teal-workbench-automation/README.md) | Teal 自动化执行、类型化 Host、扩展装配、能力与信任边界。 |
| [`teal-workspace`](crates/teal-workspace/README.md) | 版本化文件快照、修改提案与修改记录。 |
| [teal_lang::lsp](https://github.com/dev-chee/teal-lang/blob/main/src/lsp/README.md)、[teal_lang::dap](https://github.com/dev-chee/teal-lang/blob/main/src/dap/README.md) | Teal 语言服务器和源码调试协议适配。 |
| [`teal-lsp-client`](crates/teal-lsp-client/README.md) | 通用 stdio LSP 客户端、文档同步和协议编辑解析。 |
| [teal_compiler::format_named](https://github.com/dev-chee/teal-lang/blob/main/crates/teal-compiler/README.md) | 保留 token 拼写与顺序、仅调整空白的格式化库与命令。 |
| [teal_host::browser](https://github.com/dev-chee/teal-lang/blob/main/crates/teal-host/README.md) | 受管 Chromium 进程、CDP 页面与交互会话。 |
| [teal_host::computer](https://github.com/dev-chee/teal-lang/blob/main/crates/teal-host/README.md) | 桌面可访问性快照、语义操作和单独授权的原始输入。 |
| [teal_host::web](https://github.com/dev-chee/teal-lang/blob/main/crates/teal-host/README.md) | 无 JavaScript 的结构化文本网页客户端。 |
| [`tools/teal-xtask`](tools/teal-xtask/README.md) | 工作台分发、文档、workspace 校验和显式门禁等仓库维护工具。 |
| [`runtime`](runtime) | 源码 Agent、配置、模板、补全、安装脚本和工作台扩展包。 |
| [`examples`](examples/README.md) | 语言与 Host 用法、Agent 示例、工作台扩展及组合场景。 |
| [`docs`](docs) | 架构说明、工作台手册和语言参考。 |

工作台扩展包由 [`catalog-workbench.toml`](runtime/packages/catalog-workbench.toml) 登记，包含 `agents`、`bench`、`feishu`、`git`、`http` 和 `oauth`。`std` 及其 builtin 制品属于 `teal-lang`。

## 架构与修改边界

依赖方向为工作台使用 `teal-lang`。`teal-bench` 是唯一直接绑定 Ratatui/Crossterm 的终端 UI；编辑器领域层、工作区服务和自动化层通过类型化接口供不同宿主复用。`teal_utils::text` 保持为协议共用的中立文本层。

编辑器、LSP、Agent 工具和自动化对打开文档的修改都经过共享事务。文件操作还需核对工作区路径、文件身份和文档版本，并遵守批准、审计与恢复边界。工作区状态与 VM 状态分别管理，恢复程序不会自动撤销文件或外部命令的副作用。

模型调用、上下文、权限判定、审计和持久运行事实沿用 `teal-lang` 的运行时契约。`teal-workbench-services::agent_algorithms` 只提供无界面状态与交付证据算法；语言工具沿用编译器语义。详细分层见 [顶层架构](docs/TEAL-ARCH.md)。

## 文档与开发

- [工作台手册](docs/TEAL-BENCH-MANUAL.md)：启动、编辑、脚本、模型配置、权限、扩展和 SDLC 工作流。
- [语言手册](docs/TEAL-LANG-MANUAL.md)：Teal 源码与运行时能力参考；语言实现和契约由 `teal-lang` 维护。
- [示例说明](examples/README.md)：示例前提、运行方式和验证边界。
- [仓库开发约定](AGENTS.md)：架构约束、变更范围、针对性验证、显式门禁和提交要求。

本仓库的维护工具只审计当前 Cargo workspace。`cargo xtask dist` 组装工作台工具和扩展资源；完整发行使用 `cargo xtask dist --language-root <干净且版本一致的语言 checkout>`，由语言仓库构建语言二进制并合并标准包。`builtins`、`knowledge`、ABI/VM/Host coverage 和语言 rustdoc 由 `teal-lang` 独占。验证范围按本次变更确定，完整门禁需显式启动。

## 许可证

[MIT](LICENSE)。

## 本机服务与语言工具迁移

浏览器、桌面和文本网页服务使用 `teal-host` 的独立可选 feature；工作台显式启用并注入审计。
通用注册层传入 `std.bench.browser/computer/web`，Teal 包装仍由本仓库维护。句柄只能在当前
后端实例中使用，旧持久运行不能把失效句柄当作恢复后的能力。

源码与 Rust 调用方统一使用 RuntimeTier、Permission、Embedded 和 `T | Error`。
运行锁升级为版本 9，catalog 为 6、package 为 7；旧格式拒绝并保留原文件，旧程序需重新编译。
DAP launch 使用 `tier: embedded/hosted/agentic`，旧 `runtimeProfile` 拒绝。
发行必须以同一固定 Git revision 构建语言工具，工作台自己的 `teal` 入口保持独立。
