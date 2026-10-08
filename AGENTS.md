# AGENTS

本仓库是拆出的 `teal-workbench` 工作台仓库。产品入口与总体说明见 [README](README.md)；编辑器、工作区、协议和应用服务的实现细节以对应 package 的 `README.md` 为准。语言与嵌入能力属于 `teal-lang`，不在本仓库复制实现。

## 架构边界

- 根包 `teal` 只做命令解析、进程 I/O 和产品 bin 分发；入口应派发到 sibling binary，工作台库不得反向承担根 CLI 的全部解析职责。
- `teal-workbench-core` 提供编辑器领域模型、文档、事务、窗口和可选 syntax 模块；`teal-text` 只提供协议共用的文本快照与坐标转换，不依赖编译器、LSP JSON 或编辑事务。
- `teal-workbench-services` 提供文件树、搜索、进程、剪贴板、runtime lock、工具和工作区服务；其中 `agent_algorithms` 只做无界面 Flow 状态与交付证据验证，不启动模型、不登录、不批准、不提交编辑。
- `teal-workbench-automation` 承载 Teal 自动化、typed Host、能力与信任边界；模型、Agent 上下文、权限、审计和持久运行时属于 `teal-lang` 的运行时能力，工作台只做适配。
- `teal-bench` 是唯一直接绑定 Ratatui/Crossterm 的终端 UI。`teal-browser`、`teal-computer`、`teal-web` 分别保持浏览器、桌面可访问性和网页会话边界；编辑修改必须经过共享工作区事务，外部工具受工作区与批准边界约束。
- `teal-lsp`、`teal-dap` 和 `teal-lsp-client` 遵循 `teal-lang` 编译器语义；语言或诊断变化时必须检查协议映射、源码位置和调试会话，不在工作台复制语言规则。
- `teal-workspace`、`teal-fmt`、`tools/teal-xtask` 和 `runtime/packages/{agents,bench,feishu,git,http,oauth}` 分别负责工作区文件、仅改变空白的格式化、仓库维护命令和工作台扩展包。`runtime/packages/std` 及其 builtin 制品属于 `teal-lang`。
- 依赖方向只能是工作台使用 `teal-lang`；跨仓库不得使用 `path` 依赖。拆仓提取期间 Cargo 中若出现 `git = "file:///D:/teal-lang"`，也必须保持版本约束和可替换为发布版/固定 tag 的边界。不得把 `teal-lang` 的源码复制进本仓库。

## 变更、文档与回归

1. 开始前运行 `git status --short --untracked-files=all`，记录并保留已有改动；不得覆盖、格式化或提交无关文件。只读审查不修改文件、不跑无关测试、不创建提交。
2. 先读实际入口、类型、调用方、测试和 package README，再决定范围。修改工作台行为时优先落在产生问题的层，不在 CLI 或 UI 堆补丁。
3. 语言语义、程序格式或公共 API 变化先确认对应 `teal-lang` 版本、Git tag 和 ABI 契约，再检查 LSP/DAP、格式化、工作区服务、扩展包、示例、测试和文档；不要把工作台局部通过当成语言契约已验证。
4. 工作区编辑必须保持共享事务、路径限制、批准和恢复边界；Agent 工具不得绕过这些边界直接写文件或提交编辑。内部重构保持公开协议、诊断阶段、程序格式和路径不变；破坏性变更须同步说明迁移边界。
5. Markdown、rustdoc 和注释使用简体中文（工具标题如 `Safety` 保持原样）；Rust/Teal 标识符、字符串、诊断和协议值使用英文。公开 API 的 rustdoc 说明用途、用法、失败条件和重要约束，关键状态与安全取舍写清“为什么”。

## 快速验证与显式门禁

默认只验证本次改动直接影响的范围；“测试”“交付”或“提交”不等于授权完整门禁。

- Rust 修改运行 `cargo fmt --all -- --check`，并运行受影响行为的精确测试；包内局部实现再运行 `cargo clippy -p <package> --all-targets --all-features -- -D warnings`。
- 跨 crate、工作区配置、公共协议或 `teal-lang` 依赖变更运行 `cargo test --workspace --all-targets` 与 `cargo clippy --workspace --all-targets --all-features -- -D warnings`。测试前确认 Cargo.lock、Git source 和当前 checkout 一致。
- CLI、LSP、DAP、PTY 或平台路径变更，必须用本次源码重建的真实二进制执行对应端到端场景，不能以旧 `target` 产物作证。
- `cargo xtask rustdoc verify`、`cargo xtask workspace verify` 等维护命令只在其输入路径和依赖确实存在时执行；拆仓过渡期间先检查命令是否误指向 `teal-lang` 的 `std` 或 builtin 制品，不要假设 workbench 可独立生成语言制品。
- 只有用户明确要求门禁或全量验证时才执行跨层门禁。若本仓库提供 runner，使用 `cargo xtask test-gate run --spec target/test-gate/<task-id>/gate.toml`；开始前冻结输入，输入变化立即将结果标记为 `STALE`。日志、报告和指纹写入 `target/test-gate/<task-id>/`，验收时核对 HEAD、命令、退出码和提交内容。
- 未运行、环境失败或局部验证不得描述为全部通过；已有无关失败须记录命令、退出码和边界。

## 完成与提交

有文件修改的完成任务必须创建只包含本任务改动的提交，除非用户明确免除或外部条件阻止。提交前运行适用验证和 `git diff --check`，复查最终 diff；只暂存任务文件，并用 `git diff --cached --name-only` 与 `git diff --cached` 核对，禁止宽泛暂存。无法提交时保留工作区并说明阻碍、未完成验证和当前状态。
