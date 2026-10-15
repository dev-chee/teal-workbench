# 本机服务与语言协议验证

初次迁移（历史记录）使用语言版本 `963595f494a1dd1cf86b591cc12f8d3162325f1d`。语言验证使用独立干净 checkout，
工作台 Cargo manifest 与 lock 使用同一 Git revision。本轮未运行跨层 test-gate runner，也未推送。

工作台测试需要合并后的 runtime：语言 std 与工作台扩展来自各自源码。开发联调设置
`TEAL_RUNTIME_DIR` 为该目录，`TEAL_RUNTIME_DEVELOPMENT=1`；不在工作台复制语言实现。
发行目录使用 xtask 的同版本合并流程。

## 验证范围

- 语言：workspace tests/clippy、格式、Host 三 feature 独立构建、Embedded WASM 与正常依赖闭包。
- Host：审计缺失拒绝、精确 URL/CDP 子目标授权、配额、取消未知效果、过期引用、截图图片校验和离线网页恢复。
- 协议：本次源码构建的 LSP 文档同步、UTF-16/CRLF、导入重分析、导航、格式化；DAP launch/attach、断点、步进、求值、权限拒绝和旧字段拒绝。
- 平台单列：Windows Edge 真实 CDP 三项、UIA 窗口枚举与旧引用失效两项已在正式语言提交上通过，不能由 Linux workspace 测试替代。
- 工作台：workspace tests/clippy、扩展包装与示例、发行产品来源与文档清单。

语言提交的 workspace tests（只跳过下述一项）、全 feature clippy 与 Embedded WASM 检查退出码均为 0。
工作台完整 `cargo test --workspace --all-targets --no-fail-fast -j 2 -- --test-threads=1` 和全 feature clippy 退出码均为 0，未跳过测试。包含应用层 137 项、服务层 71 项、发行工具 40 项及其他 package 测试。SDLC 批准/交付/部署、知识发布与事务回归均通过。
Windows 安装器的 PowerShell 语法与 14 产品清单一致性检查通过；未执行实际安装或覆盖用户安装目录。
真实 CLI 的 8 项冒烟检查、workspace verify 与 rustdoc audit 通过，14 个产品的来源分别来自工作台和锁定语言提交。

## 已确认的基线限制

- 语言 `source_string_tests::source_string_literals_use_english_text` 被并行任务新增的
  `examples/agent/12306_trip_planner.tl` 中文字符串阻断；本次未改该文件。其他 workspace 测试另行运行，明确只跳过这一项。
- 工作台全局 rustfmt 报告未改文件的已有排序差异；语言提交的父提交也带来 VM 两个未改文件的格式差异。任务文件单独通过格式检查，未批量改动无关文件。
- 73 个可独立编译示例通过；31 个纯 Embedded 执行中 30 个输出匹配。`core/task_control.tl`
  的 TaskGroup.join 失败可以缩减为空组 join：Compiler 生成子任务，VM 却要求创建者。此契约在迁移前已存在；
  修复需要明确的 join 委托和程序/快照版本升级。本次不更改该格式，不将编译通过表述为运行通过。

原始命令、退出码与日志保存在两仓 `target/migration/`，完整记录在最终提交说明中列明。

## 审查修复记录

本次修复在 `/mnt/d/teal-lang-service-fixes` 与 `/mnt/d/teal-workbench-service-fixes` 两个独立 worktree 完成，目标分别是 `/mnt/d/teal-lang` 和 `/mnt/d/teal-workbench` 的 `main`。语言修复提交为 `ee70036f4bbf917ff686a3b77f551cea649315ad`；工作台所有语言依赖及 Cargo.lock 统一使用该 Git revision。没有跨仓 path 依赖，也没有推送。

修复范围：审核 Agent 固定 `knowledge-review` 预设；未结束源码运行拒绝模型切换；共享调试解析限制输入、节点和深度；浏览器创建页面期间的失败或取消撤销会话；新建文件保留批准前路径证明并穿过共享事务队列；Windows 窗口重新发现不能复用旧代次；扩展使用 `computer.raw-input` 权限名。新文件内容按原字节提交，Bench 与无界面事务的回执摘要保持一致。

浏览器初始化失败后需要关闭旧会话，在新会话重新批准全部所需 URL；未知效果不能自动重放。模型选择需先正常结束旧运行，或显式处理未完成的恢复状态，随后才为新运行保存选择。旧文件和运行账本不自动改写。

原始命令、退出码与日志位于两个修复 worktree 的 `target/service-fixes/`。开发验证 runtime 的完整清单使用 format 6，并同时提供互斥且覆盖完整集合的 `catalog-base.toml`（std）与 `catalog-workbench.toml`（工作台扩展）。

验证结果：

- 语言最终提交：`cargo test --workspace --all-targets -j 2 -- --skip source_string_tests::source_string_literals_use_english_text`、`cargo clippy --workspace --all-targets --all-features -j 2 -- -D warnings` 和 Embedded WASM 编译均退出 0。未跳过时，唯一阻断仍是上文记录的示例中文字符串检查。
- Host：browser/computer/web 三个 feature 分别独立构建成功；三个 feature 合并的 Host 测试通过 132 项。新回归覆盖 Fetch.enable 与 Page.navigate 期间取消、关闭 target 失败、子进程锁延迟、配额释放、会话撤销后的 Unknown 保留，以及窗口重新出现和代次耗尽。
- 协议：本次源码构建的 LSP/DAP 二进制三项集成测试通过；深度及平坦表达式被拒绝后，同一 DAP 会话仍能继续正常求值和步进。
- Windows 单列：桌面后端六项单元测试、真实 Edge/CDP 三项测试、交互桌面 UIA 两项测试退出 0。
- 工作台：锁定新语言提交后，`cargo test --workspace --all-targets -j 2` 与全 feature clippy 均退出 0，未跳过测试。包含 Bench 140 项、automation 12 项、services 72 项，以及其余 workspace 测试。xtask 对受控失败子进程的测试会打印预期的 FAILED，其父测试和整条 workspace 命令均成功。
- 格式：任务涉及的 Rust 与 Teal 文件通过检查，`git diff --check` 通过。两仓 `cargo fmt --all -- --check` 仍报告未修改文件的基线差异：语言侧为 `crates/teal-vm/src/lib.rs` 的模块排序，工作台侧为既有导入排序；本次没有扩大格式化范围。

工作台首轮验证因测试 runtime 缺少基础层清单而失败，补齐后已完整重跑；未将首轮结果作为代码失败或通过证据。修复期间并行提交的 VM 优化已纳入语言修复提交的父历史，并对最终内容重新运行验证。
