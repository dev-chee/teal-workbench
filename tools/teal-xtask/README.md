# teal-xtask

Teal 仓库维护命令：分发组装、ABI/VM/Host 覆盖率门禁、可恢复的最终门禁调度，以及中文 rustdoc 的审计、构建与验证。通过根目录的 `cargo xtask <command>` 调用，不作为产品运行时 API 发布。

## 覆盖率门禁

`cargo xtask coverage <abi|vm|host|all>`：`abi` 只统计 `teal-abi` 生产源码且要求行/区域覆盖率都不低于 95%；`vm` 与 `host` 分别在工作区上运行一次 `--all-targets` 覆盖采集，再按各自判据检查 `crates/teal-vm/src` 与 `crates/teal-host/src` 的全局阈值与关键模块行覆盖率；`all` 只采集一次，用同一份数据顺序判定 VM 与 Host，并一次性报出两者的失败原因。

关键模块清单必须以工作区相对路径指向真实文件：缺一会以 `missing critical coverage module` 在启动 cargo 之前失败，避免改名后覆盖率静默变成 0%。每次判定都会把结构化摘要写到 `target/llvm-cov/<target>.json`，失败时同样写出以便对照。覆盖率门禁只在需要时运行，不进入默认验证矩阵；CI 在 Linux/Ubuntu 上执行，平台专属代码需在对应主机本地运行同一命令。

命令范围与验证边界见 [teal-workspace 实现细节](../../crates/teal-workspace/README.md#工作区分发与维护工具) 和根 [AGENTS.md](../../AGENTS.md)。
