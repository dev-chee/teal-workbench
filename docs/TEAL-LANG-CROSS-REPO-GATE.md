# 本机服务与语言协议迁移验证

语言版本固定为 `963595f494a1dd1cf86b591cc12f8d3162325f1d`。语言验证使用独立干净 checkout，
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
