# Teal 语言与工作台接入

语言规范、标准包、Builtin Interface 与编译选项由语言仓库维护。本仓库消费固定 Git revision，
语言参考以同版本的 [Teal 语言手册](https://github.com/dev-chee/teal-lang/blob/main/docs/TEAL-LANG-MANUAL.md)
和对应 package 的源码、测试为准；这里保留工作台接入边界，避免维护第二份过期语言规范。

## 运行与错误

运行层级为 `embedded`、`hosted`、`agentic`。层级决定支持的实现机制，Permission 决定
主体、精确操作、资源与版本是否获准。`--tier` 替代旧运行配置档参数。
可处理失败使用 `T | Error` 与 `?`；入口显式检查结果。未知外部效果、取消和 VM RuntimeError
不能当作普通 Error 自动重试。旧字节码与旧持久格式按语言仓库版本规则拒绝，原文件保留。

## 编辑器和调试

工作台保留 `teal-lsp-client`、文档事务与界面适配；`teal-lsp`、`teal-dap`、`teal-fmt`
来自语言根包。LSP 的 UTF-16 坐标共享 `teal_utils::text`，格式化直接使用 Compiler。
DAP 的 launch 字段为 `tier`，接受三个层级，默认 Agentic；`runtimeProfile` 明确拒绝。
权限使用 `permissionFile`，没有批准通道时 Ask 被拒绝。adapter 与 debuggee 保持独立进程和
双向 token 认证，断点、步进、变量及纯表达式求值由 VM/Runtime 负责。

## 工作台扩展

`runtime/packages/{agents,bench,feishu,git,http,oauth}` 由工作台维护；`std` 属于语言仓库。
`bench/browser`、`bench/computer`、`bench/tools` 包装复用 Host 的唯一注册和后端实现。
UI、批准、文件修改、窗口与会话属于工作台；修改仍通过共享事务，不绕过授权和恢复检查。

用户命令与交互流程见 [工作台手册](TEAL-BENCH-MANUAL.md)，跨仓边界见 [架构](TEAL-ARCH.md)。
