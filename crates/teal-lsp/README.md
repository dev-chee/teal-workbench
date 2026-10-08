# teal-lsp

Teal 的 Language Server Protocol 3.17 库实现。`teal-lsp` package 生成独立 bin 并拥有其命令行与异步运行时入口；根 `teal` 只把 `lsp` 子命令派发给该 sibling binary。

本 README 后续章节说明协议边界及其与编译器、格式化库和 IDE 客户端的关系。

重命名只消费语法有效、文本及依赖状态仍匹配的当前分析；不完整编辑期间返回空结果，
不会把只读导航使用的历史坐标当作写入范围。文档打开、修改或关闭时按实际导入依赖
重新检查所有受影响入口，包括上次编译失败时尝试读取的文件，再统一发布聚合后的诊断。

## 实现细节

### 格式化、LSP、DAP 与调试

`teal-fmt` 只调整 token 间空白，保留 token 拼写和顺序；独立二进制与 LSP 文档格式化复用同一库。对应 package 拥有 `teal-fmt`、`teal-lsp` 与 `teal-dap` 的命令行和进程适配，协议与格式化实现仍位于对应库；根 `teal` 的 `fmt`、`lsp`、`dap` 只派发到同目录工具，不在总入口进程内执行协议或格式化逻辑。

`teal fmt` 写文件时在同目录暂存完整输出，发布前逐字节复核原文并拒绝链接目标，最终原子替换；并发修改使本次格式化失败而保留外部内容。

`teal-text` 保存不可变 UTF-8 快照并统一字节偏移与 UTF-16 行列坐标转换，不包含协议 JSON、AST 或编辑事务。`teal-lsp` 是 LSP 3.17 stdio 服务器，在协议边界把共享坐标转换为 JSON。打开缓冲区覆盖导入时的磁盘内容；`initializationOptions.entryFiles` 指定分析根，未指定时使用含顶层 `main` 的文档。诊断、符号、定义、引用、重命名、格式化、代码操作和语义词元来自编译器与格式化库。

服务端在临时文本上验证同一通知的全部 `contentChanges` 后才更新文档及分析。客户端读取头部与正文分别限于 16 KiB 和 32 MiB；语义词元增量先校验原范围顺序、重叠和完整五元组，非法响应清除缓存并重新请求完整结果。签名提示从词元和括号深度选择仍未关闭的调用。

`teal-vm` 的 `debug::session` 保存工作台与 DAP 共用的断点、步进、异常和源码位置状态。`teal-dap` 是 stdio adapter，launch 通过 `runInTerminal` 启动隐藏 debuggee，并用版本化挑战应答双向验证 adapter/debuggee TCP 通道。证明绑定双方随机 nonce、版本和 TCP 角色；不发送令牌正文，整个握手限时 10 秒、单包上限 4 KiB，旧握手直接拒绝。令牌只认证、不加密；远程 attach 应使用环回或加密隧道。暂停求值器拒绝函数调用、赋值和 Host 副作用。

共享调试会话只在尚未离开刚暂停位置时跳过同一断点，回到该位置会再次暂停；DAP adapter 对正常结束转发一次 `terminated`，仅在连接意外关闭且未收到事件时补发。

各包入口见 [格式化](../../crates/teal-fmt/README.md)、[LSP](../../crates/teal-lsp/README.md)、[DAP](../../crates/teal-dap/README.md)、[VM](../../crates/teal-vm/README.md) 的 README；实现与接入参数以对应源码和 rustdoc 为准。

LSP 的文档状态、诊断、语言查询与协议传输分别维护。文档、覆盖加载器和 `teal-text` 行索引共享不可变文本，
同一次成功编译的分析通过 `Arc` 共享；源码 ID 只在该分析快照内解释。新分析只绑定文本仍一致的文档，
不完整文本继续使用既有回退策略，关闭文档恢复磁盘快照时重新建立来源关系。

#### 包级契约

- `teal-fmt`：文件参数就地格式化；没有文件或单独传入 `-` 时读取标准输入并在非检查模式写入标准输出；`--check` 不写入文件或标准输出，输入需要变化、读取失败或语法无效时返回退出码 1，语法无效的文件不会被覆盖。库入口为 `format` 与 `format_named`，后者让语法诊断携带逻辑文件名。
- `teal-text`：`LineIndex` 的克隆共享不可变文本与行表，已有 `Arc<str>` 文本可以通过 `LineIndex::from_shared` 直接建立索引；更新文本应构造新索引，旧快照的坐标规则保持不变。
- `teal-lsp`：已实现增量 UTF-16 文档同步、解析器/编译器诊断、补全、悬停、签名帮助、定义、引用、预备重命名/重命名、文档/工作区符号、整篇文档格式化和完整语义词法单元。完整编译成功时导航使用同一次 `CompilationAnalysis` 的跨模块声明身份，选择性重导出会落到实际声明模块；缓冲区不完整时才回退最近一次 resolved 快照。导入源码中的编译错误与检查项警告发布到导入源码的 URI，Warn 使用 LSP Warning、deny/forbid 使用 Error 并保留关联诊断，`pub` 作为关键字词法单元发出。核心函数、保留命名空间和接收器方法来自编译器共享的 `BuiltinCatalog`，其定义导航指向按接口摘要物化在用户缓存目录中的只读 `builtins.tli`。服务器以无参数启动，language identifier 为 `teal`；`initializationOptions.entryFiles` 接受根文件列表，否则使用含顶层 `main` 的文档，旧 `entryPoints` 字段被拒绝。`textDocument/formatting` 复用 `teal-fmt` 规则，语法不完整或已符合格式的文档返回空编辑；`textDocument/documentSymbol` 返回平面 `SymbolInformation`，位置指向声明名称。分析把当前构建实现的全部扩展作为全局范围，并从最终可达代码推导所需 Extension；服务器不执行程序，也不打开 VM、宿主、dylib、遥测或性能剖析连接。
- `teal-dap`：launch 使用 DAP `runInTerminal` 在所选终端启动程序，`console` 可选 `integratedTerminal` 或 `externalTerminal`，省略时使用外部终端，非法值返回失败响应且不启动程序。当前支持断点、继续/暂停、单步进入/越过/跳出、调用栈、局部作用域、可展开的数组/映射/类/枚举、异常断点、悬停/监视求值和 `setVariable`。launch 接受 `program`、`cwd`、`args`、`stopOnEntry`、`console`、`runtimeProfile`、`permissionFile`、`optimization`、`compilerDebug`、`debugInfo` 以及 lint 设置；旧 `allow*` 授权字段和旧 `entry` 字段均拒绝。Debuggee 强制 `optimization=none` 与 `debug-info=true`，并把同一授权策略绑定到 HostRuntime 后再加载 VM。求值有意保持纯净，只允许字面量、可见变量、运算符、索引/字段访问、容器构造和基础类型转换。远程 attach 通过隐藏入口 `--debuggee --listen <addr> --token <random-secret> --program <file>` 启动终端 debuggee，attach 配置需提供相同的 host、port 与 token；线路协议不提供 TLS，远程机器应只绑定环回地址或使用可信隧道。
- `teal-workbench-core::syntax`：解析接收不可变文本快照，不拥有编辑器修订或线程调度；缓存绑定 `ENGINE_ABI` 与文法哈希，只有解析 IR 或表语义变化才提升 ABI。
- `teal-lsp-client`：负责协议 framing、服务管理、文档同步、UTF-16 坐标转换、诊断和 workspace edit 解析；嵌入应用负责请求触发、修订过期检查、工作区授权和缓冲区事务提交，本 crate 不直接修改磁盘或安装语言服务器。
