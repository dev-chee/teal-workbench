# teal-workbench

Teal 的编辑器与 Agent 工作台。仓库提供 Windows 优先的模态终端界面、共享编辑与工作区服务、Teal 自动化、语言协议工具，以及受控的浏览器和桌面交互能力。使用 `teal bench` 启动工作台，也可以在其他宿主中复用各个库。

本仓库从 Teal 拆出，负责产品交互和应用集成；语言、编译器、VM、标准库以及模型、权限、审计和持久 Agent 运行时由 `teal-lang` 提供。工作台通过依赖使用这些能力，业务流程由 Teal 源码程序表达。

## 主要能力

- **编辑与工作区**：共享缓冲区、多选择、事务编辑、分支撤销、文件树、搜索、窗口布局和会话恢复。
- **语言工具**：语法高亮、通用 LSP 客户端、Teal LSP 服务器、DAP 调试适配器，以及只改变空白的源码格式化。
- **源码 Agent**：工作台把用户输入、工具结果和打开文档状态交给持久 Teal 程序，呈现会话、问答、批准、运行状态和交付证据。
- **自动化与扩展**：通过类型化 Host、能力声明和信任边界接入配置、文法、命令、面板与后台任务。
- **外部交互**：结构化文本网页、受管 Chromium CDP 会话和 Windows UI Automation；外部动作使用独立授权与审计。

默认 Agent 入口是 [`runtime/agents/default.tl`](runtime/agents/default.tl)。Chat、Task、目标和交付规则由源码程序及 [`agents`](runtime/packages/agents) 扩展包执行，Bench 负责交互与呈现。相同程序也可由无界面宿主运行。

## 从源码开始

### 构建前提

- Rust **1.98.1**，使用 Rust 2024 edition，并具备目标平台所需的原生编译与链接环境；仓库通过 `rust-toolchain.toml` 固定工具链。
- Git，以及 [`Cargo.lock`](Cargo.lock) 锁定的 `teal-lang` 源码提交。
- 运行终端工作台时使用支持交互输入和终端控制序列的终端。

拆仓依赖通过 HTTPS Git 源和锁定 revision `11491542dbf56393a91df53ce68f2a26ec2efad1` 获取；不要求 sibling checkout，也不使用跨仓库 `path` 依赖。语言仓库后续升级必须先更新稳定 revision，再同步锁文件与格式/运行时兼容验收。

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
| `teal fmt` | `teal-fmt` | `teal-fmt` |
| `teal lsp` | `teal-lsp` | `teal-lsp` |
| `teal dap` | `teal-dap` | `teal-dap` |
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
| [`teal-text`](crates/teal-text/README.md) | 不可变文本快照与 UTF-8 / UTF-16 坐标转换。 |
| [`teal-workbench-services`](crates/teal-workbench-services/README.md) | 文件树、搜索、进程、剪贴板、runtime lock、工具、观测和工作区服务。 |
| [`teal-workbench-automation`](crates/teal-workbench-automation/README.md) | Teal 自动化执行、类型化 Host、扩展装配、能力与信任边界。 |
| [`teal-workspace`](crates/teal-workspace/README.md) | 版本化文件快照、修改提案与修改记录。 |
| [`teal-lsp`](crates/teal-lsp/README.md)、[`teal-dap`](crates/teal-dap/README.md) | Teal 语言服务器和源码调试协议适配。 |
| [`teal-lsp-client`](crates/teal-lsp-client/README.md) | 通用 stdio LSP 客户端、文档同步和协议编辑解析。 |
| [`teal-fmt`](crates/teal-fmt/README.md) | 保留 token 拼写与顺序、仅调整空白的格式化库与命令。 |
| [`teal-browser`](crates/teal-browser/README.md) | 受管 Chromium 进程、CDP 页面与交互会话。 |
| [`teal-computer`](crates/teal-computer/README.md) | 桌面可访问性快照、语义操作和单独授权的原始输入。 |
| [`teal-web`](crates/teal-web/README.md) | 无 JavaScript 的结构化文本网页客户端。 |
| [`tools/teal-xtask`](tools/teal-xtask/README.md) | 分发、文档、覆盖率和显式门禁等仓库维护工具。 |
| [`runtime`](runtime) | 源码 Agent、配置、模板、补全、安装脚本和工作台扩展包。 |
| [`examples`](examples/README.md) | 语言与 Host 用法、Agent 示例、工作台扩展及组合场景。 |
| [`docs`](docs) | 架构说明、工作台手册和语言参考。 |

工作台扩展包由 [`catalog-workbench.toml`](runtime/packages/catalog-workbench.toml) 登记，包含 `agents`、`bench`、`feishu`、`git`、`http` 和 `oauth`。`std` 及其 builtin 制品属于 `teal-lang`。

## 架构与修改边界

依赖方向为工作台使用 `teal-lang`。`teal-bench` 是唯一直接绑定 Ratatui/Crossterm 的终端 UI；编辑器领域层、工作区服务和自动化层通过类型化接口供不同宿主复用。`teal-text` 保持为协议共用的中立文本层。

编辑器、LSP、Agent 工具和自动化对打开文档的修改都经过共享事务。文件操作还需核对工作区路径、文件身份和文档版本，并遵守批准、审计与恢复边界。工作区状态与 VM 状态分别管理，恢复程序不会自动撤销文件或外部命令的副作用。

模型调用、上下文、权限判定、审计和持久运行事实沿用 `teal-lang` 的运行时契约。`teal-workbench-services::agent_algorithms` 只提供无界面状态与交付证据算法；语言工具沿用编译器语义。详细分层见 [顶层架构](docs/TEAL-ARCH.md)。

## 文档与开发

- [工作台手册](docs/TEAL-BENCH-MANUAL.md)：启动、编辑、脚本、模型配置、权限、扩展和 SDLC 工作流。
- [语言手册](docs/TEAL-LANG-MANUAL.md)：Teal 源码与运行时能力参考；语言实现和契约由 `teal-lang` 维护。
- [示例说明](examples/README.md)：示例前提、运行方式和验证边界。
- [仓库开发约定](AGENTS.md)：架构约束、变更范围、针对性验证、显式门禁和提交要求。

维护工具只验证当前 workbench 的输入。`cargo xtask dist` 默认组装 workbench 工具和 runtime；需要语言工具、`std` 与 `catalog-core` 的完整发行包时使用 `cargo xtask dist --language-root PATH`，该 checkout 必须干净且与锁定 revision 一致。`teal-lang` 的 builtin、knowledge 和 ABI/VM/Host 维护命令由语言仓库负责。验证范围按本次变更确定，完整门禁需显式启动。

## 许可证

[MIT](LICENSE)。
