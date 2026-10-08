# teal-workbench-automation

Teal 工作台中受能力约束的配置、文法和自动化宿主：把 `teal-runtime` 的编译执行与类型化工作台 Host 函数连接，并保存准备结果、导入闭包、内置扩展包/module/IR 身份、能力集和信任指纹。

本 README 后续章节说明自动化边界。

脚本执行和状态持久化在开始前需要审计接收器；嵌入宿主通过 `WorkbenchAutomationRuntime::with_audit_context` 指定接收器，并将其传入观测执行会话与 VM。

持久的工作台 Agent 自动化通过 `teal-runtime` 的 `prepare_execution`、`PreparedExecution::bind_with_access` 和 `PersistentExecution::execute_step` 装配；交互与外部对账继续使用同一持久执行对象，不直接创建第二套 Agent 运行账本。automation 编译入口显式允许 Core、Native、FFI、Agent 和 Workbench domains，LSP 的 unrestricted catalog 读取不属于执行入口。

## 实现细节

### Teal 工作台实现

本章面向实现者说明工作台的 crate 边界、生命周期、语言工具接入、会话持久化与观测机制。面向使用者的启动选项、键位、命令、脚本、Agent 配置、权限、扩展和 SDLC 用法见 [Teal 工作台使用手册](../../docs/TEAL-BENCH-MANUAL.md)。

#### 包边界

产品、命令与 crate 已统一使用 `teal bench` / `teal-bench` 命名，并保持以下单向组合：

`teal-bench` package 拥有工作台启动参数；本 package 的 `teal-knowledge` bin 拥有知识层查询、校验、导出与写入参数，自动化库本身保留无 CLI 的能力边界。

```text
teal-bench：事件循环、生命周期与装配
 ├─ 内部终端 UI ──────────── teal-workbench-core (syntax feature)
 ├─ teal-workbench-services::agent_algorithms ─── teal-agent, teal-runtime
 ├─ teal-workbench-services ─ teal-workbench-core, teal-agent, teal-web
 ├─ teal-lsp-client
 ├─ teal-workbench-automation ─ teal-workbench-core (syntax feature), teal-runtime
 ├─ teal-agent
 └─ teal-web
```

`teal-workbench-core` 拥有缓冲区、文档、选择、事务、撤销、布局、命令、消息、窗口和恢复领域模型，不依赖终端、协议、网络或模型供应商，可由未来 GUI 复用。布局以 `SplitAxis`、`Length` 和 `PanelSize` 表达槽位长度与上下限，`LayoutSpec`/`NodeSpec` 供模板一次性构造布局树；焦点导航、分隔条命中和几何分配共用同一棵树，公开的 `split_rectangles` 让组合 Widget 复用同一套长度语义与铺满规则。

窗口模型把每个工作上下文表达为 `Window`：`placement`（`Docked` 独占内容区或 `Floating{rect,z}` 叠加）、`chrome`（是否绘制标题框）、`interaction`（`Interactive`/`Passive`）、`lifetime`（`Persistent`/`Transient`）、`anchor`（`Frame`/`Cursor`/`Rect`）与 `modal`。`WindowSet` 保留标签顺序 `order` 与活动窗口 `active`，另以 `stack` 记录由底到顶的绘制顺序：标签条与 `Ctrl-w` 轮转只覆盖 `Docked` 窗口，浮动窗口获得焦点不会打乱标签；只有 `Persistent` 窗口进入会话；`Passive` 窗口不接收焦点、不参与轮转，由锚点定位，并在宿主窗口关闭时一并丢弃。浮动窗口的矩形随会话保存，缩到最小尺寸以下时确定性转回停靠。

终端 UI 只渲染和产生类型化动作，且仅位于 `teal-bench` 内。`render_frame` 按 z 序合成：先绘制活动停靠窗口，再按绘制栈叠加浮动窗口，最后绘制全局命令行；返回的 `FrameGeometry` 同时记录每个窗口与每个面板的归属，以及浮动窗口标题栏与四条边、四个角的缩放手柄命中区（命中优先级为角 > 边 > 标题栏），鼠标命中、拖动与缩放共用这份几何。其中面板是布局叶节点，正文与输入由实现 `Widget` 契约的组件承担（`WidgetChrome` 描述宿主绘制的边框与状态行）。组合容器族包括 `Split`（按 `Length` 约束并排/上下排列）、`Border`（绘制边框并把子级放入内侧）、`Padding`（按行列裁剪）与 `Stack`（在同一区域叠放并把输入交给聚焦层），它们在内部转发绘制、输入、鼠标命中、光标与 Tab 焦点。具名 Widget 覆盖 `TestsWidget`、`PickerWidget`、`AgentComposerWidget`、`DebugConsoleWidget`、`VariablesWidget`、`CallStackWidget` 与 `HoverView`；`PickerWidget` 建在 `Menu` 模糊过滤之上，把接受动作表达为类型化命令而不执行打开或写回，真实 picker 浮层即由它承载内容与输入；`Ctrl-w h/l/j/k` 与 `:window grow|shrink` 以固定步长调整浮动窗口尺寸，与拖动共用同一锚点与钳制规则。

弹出内容统一为带属性的窗口：审批、权限、秘密输入、picker、右键菜单、信息问答与补全/hover/toast 都登记为 `Passive` 瞬态浮动窗口，不再存在独立的浮层实体；浮层输入仍按栈优先消费，关闭时同步移除其窗口。共享原语供全部工作流复用：`Menu` 提供模糊过滤与选择语义（补全、picker、Debug 变量/栈、模型与权限切换），`Cmdline` 与 `Prompt` 承担 `:`/`!` 命令、补全与历史，`GutterType`/`Decorations` 供编辑器与会话面板共用行首标记与行内高亮，状态行与 spinner 复用同一套元素模型。

工作台自动化使用无界面通用算法；新的 Chat/Task/Goal 与交付规则由 Teal 程序及 `agents/delivery.tl` 执行。Bench 只显示 RuntimeStore 源码 run 的投影。旧 Bench Flow/Goal 文件和含旧 Agent 对话的会话保留原文件并拒绝作为源码 run 续接。

`teal-bench` 内部按终端适配、调试器适配与呈现分层：

- `app` 保存终端会话状态、命令路由和事件分发。
- `workbench::ui` 只包含 Ratatui/Crossterm 呈现组件；它不启动模型、不执行文件事务，也不拥有工作区策略。
- `integration` 只保留终端专属的调试器适配；工具目录与审批提示由 `teal-workbench-services::tools` 提供，`runtime_bridge` 负责桥接，`presentation` 负责呈现批准状态。
- `teal-workbench-services::observability` 负责进程级观测配置、脱敏记录和导出；各产品 package 负责 CLI 参数适配，工作台库保留生命周期编排。
- `teal-workbench-services::tools` 负责工作区工具、审批、ChangeSet、Mutation、工具链及浏览器/桌面 Host；工作台只消费事件并更新终端状态。
- `teal-workbench-core`、`teal-workbench-services::agent_algorithms`、`teal-workbench-services` 和 `teal-workbench-automation` 提供不依赖终端 UI 的工作区、Flow、服务和自动化能力；工作台通过类型化接口组合这些能力。

`teal-workbench-services` 还拥有工作区脚本清单（`.teal/scripts.toml`）：它登记脚本入口、运行时配置档、可选权限文件、测试入口与交付产物；读取时校验版本、名称、工作区内相对路径与唯一性，写入使用原子替换，但不授予权限也不执行脚本。

工作区事务、检查点、文本保存和资源操作由 `teal-workbench-core` 与 `teal-workbench-services` 实现；`app::agent_app` 只将终端输入、UI 事件和打开缓冲区版本交给源码 Agent。运行身份、VM 检查点、工具和模型收据由 RuntimeStore 保存，不再建立 Bench 专属执行事实。`runtime_bridge` 只负责 Tokio 后台任务、事件信封和 tracing 上下文恢复；`tracing_context` 位于 `teal-workbench-services`，可被工具、脚本和其他宿主共用。

#### 生命周期与修改边界

入口解析选项并初始化进程观测，再创建 Tokio 运行时和 `App`，最后进入终端原始模式。退出顺序是应用清理、终端恢复、Tokio 关闭、观测提供器关闭和日志刷新。运行错误优先保留；清理错误只在主流程成功时成为返回错误。强制终止不能保证终端或遥测最终恢复。

根事件循环是打开文档的唯一修改提交边界。键盘、鼠标、剪贴板、LSP workspace edit、Agent 工具和 Teal 自动化都转换为相对同一旧快照的 `Transaction`。`BufferStore` 让同一路径的视图共享文本与历史，视图各自保存选择和滚动。

Agent 提出的文本修改先构造成 `teal-workbench-core::ChangeSet`。该对象绑定来源 Task、请求代次、工作区相对路径、修改前后 SHA-256、完整 unified diff 以及每个 hunk 的稳定身份；旧会话中的简化 ChangeSet 展示块仍可读取。授权与实际写入不由该数据对象执行。批量工具 `workspace.apply_changeset` 在生成候选时按当前文本绑定修改前摘要，并按 hunk 应用结果绑定修改后摘要；调用方给出的摘要只会被核对，不会被直接采信。用户在审查浮层中可以应用整组、单个文件或单个 hunk，未选中的文件不进入事务；接受后立即建立整组检查点，使 `:agent rollback` 能一次撤销该批量写入。单文件 `workspace.apply_patch` 的 `expected_sha256` 允许为空，此时同样由宿主绑定当前规范化文本。

`WorkspaceMutationCoordinator` 串行化文件任务，在工作线程副本上执行阻塞 I/O，再按原缓冲区 ID 和修订提交。构造器返回 `Result` 并固定工作区身份；操作中的相对路径以工作区为基准。读取、写入和复制消费不透明 `ResolvedTarget`，删除与重命名消费保留最终目录项的 `ResolvedEntry`，执行前复核规范路径和文件身份。链接操作不误解除真实目标文件的缓冲区绑定；文本编辑与保存仍保留版本、编码和冲突检查。多资源操作逐项提交，不承诺跨文件原子性或操作系统沙箱；知识导出的权威层保护仍由 `ExportTarget` 自己负责。

#### 语法、LSP 与自动化

本地扩展清单由 `teal-workbench-automation::extension` 编译预检，完整入口与导入闭包形成指纹。激活原子安装贡献，事件执行不清空注册；legacy 与扩展事件经过同一串行调度入口，后台阶段任务独立运行。扩展 API/SDK 和实例状态与语言标准库、Chat/Task Flow 分别版本化。

后台任务以实例、事件和序号形成稳定启动键，通过 runtime 的 prepare/bind/execute/restore 门面接入现有 AgentRuntime。RuntimeStore 的 `prepared_runs` 只保存启动元数据与候选结果；外部操作继续走原有账本。准备阶段固定程序、输入和绑定，保存根 VM 零指令检查点后才标记已启动。实例状态提交同时确认结果事件和任务交付；终态结果重投递不重跑入口。

`ExecutionBindings::source_agents` 按已编译模型 Agent 的名称选择运行级 provider、具体 profile、model、可选的 `compaction_profile` 和补充指令；它优先于源码 `@model`，认证预检必须检查覆盖后的实际需求。摘要预设未配置或已确认不可用时沿用确定性压缩，独立摘要账户发送历史前仍需权限与披露确认。空映射保留源码模型绑定；恢复使用已保存的具体绑定，不能重新采用后来变化的活动 profile。业务批准由工作台签发并绑定文件摘要；工作区写入仍经过共享事务，命令和 Git 收据不会由模型文本构造。

`teal-workbench-core::syntax` 提供声明式文法 IR、最长匹配词法、规范 LR(1) 表、有界 GLR、错误节点、增量 token cache 与捕获。缓存表加载时核对文法产生式和全部状态、动作、跳转引用，工作台在校验失败后重建。编辑器将快照交给有界工作线程，只接收与当前修订匹配的结果。注入遍历限制深度 32、区域 2,048 和源码 8 MiB；未知语言安全跳过。

内置文法覆盖 Rust、Teal、Markdown、JSON、TOML、C、C++、Python、JavaScript、TypeScript 和 Go。Teal 文法脚本只获得 `syntax.register`；普通工作区自动化的其他能力需要独立批准。解析表缓存绑定文法哈希与 `ENGINE_ABI`。

`teal-lsp-client` 是通用 stdio 客户端，负责 framing、文档同步、基于 `teal-text` 的坐标适配和 workspace edit 解析。它只公开中立文本编辑；根应用负责请求触发、修订选择器、授权和转换后的事务提交。Teal server 优先使用同一构建/分发目录中的 `teal-lsp`，再查 PATH。

工作台保存源码 Agent 的活动输入通道和事件显示位置；后台事件携带 run、任务和代次身份。工作区、浏览器、桌面和 Teal 工具链继续共享同一授权与 Host 桥接，其中平台注册按独立模块维护。

`teal-config` 以 TOML 登记 `agent_apps.<name>.script`、MCP、命令、Hook、面板与会话恢复策略；Agent 指令、工具和阶段规则不能放入 `bench.toml`。`teal-workbench-automation` 负责文法和自动化的类型化 Host、能力与信任边界，不再执行 `config.teal` 作为配置入口。

#### Agent、文本网页与会话

工作台复用 `teal-agent` 的供应商协议、认证、工具和命名会话。`WorkspaceToolRuntime` 持有工作区 HostRuntime 与授权身份；`register_host_tools` 从显式导出生成模型目录，脚本 Host 绑定则使用脚本自身的精确操作集合。Agent 程序执行时传入当前程序的名义类型目录，工具因此可以按源码声明生成精确的参数与返回契约；工作台自身的目录没有程序目录，只导出全部使用擦除类型声明的工具。只读工具可并发，修改仍通过工作区协调器。检查点型 Host 工具把模型值与应用检查点分离，后者不进入模型历史。用户输入、工具批准和认证使用不同的类型化界面，彼此不能替代。

LSP CodeAction 同时包含工作区编辑与服务器命令时，确认后把编辑送入共享文件作业队列，等待完成通知成功才发送 `workspace/executeCommand`；队列拒收、缓冲区修订失效或编辑失败都会阻断命令。

发行版默认 Agent 是 `runtime/agents/default.tl`，以 `task main` 进入；`receive()` 等待输入，Chat 使用 `self.chat`，Task 使用独立命名会话的 `self.think`。计划、目标、分支、交付状态都存在 Agent 字段和 RuntimeStore 检查点中；Bench 只发送输入并显示 `emit` 与可选 `render` 事件。Bench 通过 `teal-runtime` 的 `prepare_execution`、`restore_execution` 和 `PersistentExecution::execute_step` 结算同一 run，重启后寻找同摘要未完成程序并从原检查点恢复。

内建 Teal Agent 的 `:task-commit` 先再次执行 `agents/delivery.tl` 验证，再请求绑定脚本哈希、程序摘要和提交说明的业务批准。`teal-workbench-services::VerifiedCommitService` 作为 CLI 与 Bench 可复用的可信服务，重新核对 RuntimeStore 的原批准、模型工具调用身份、当前文件和程序摘要；随后以隔离的 Git index 验证精确路径与暂存 blob，构造提交对象并用原 HEAD 作为比较值更新引用。并发改写或其他暂存内容不会混入该提交；此入口不执行 Git hooks，也不推送。未知效果保留在 run 操作账本中，不能自动重放。

Task 交付引用本轮真实 ToolResult。`agents/delivery.tl` 要求格式、编译、测试和授权运行按序成功，具有相同脚本路径、配置档与程序摘要；验收及 `:evidence` 后续查询重新核对入口、导入闭包和产物文件。Chat 上下文只能经 `AgentContextProjection` 生成、展示和确认；投影排除工具参数、原始结果和私有状态，也不携带权限。Ready 可显式续接，InFlight 或未知结果阻断且不重放。

`teal-web` 只支持 HTTP/HTTPS GET/HEAD，把 HTML 转为结构化文本；3xx Location 仅返回规范化 HTTP/HTTPS 目标，调用方须对新 URL 另行授权并请求，当前调用不自动跟随也不写入浏览历史。它不执行 JavaScript，不实现 CSS 布局、表单、Cookie、上传、媒体或嵌入式 WebView。响应默认限制 5 MiB；每个面板有独立请求代次和历史。会话只保存有界离线页面，恢复不发起网络请求，显式刷新才重新加载。

工作台会话保存窗口集合、带槽位约束的拆分树、面板状态、浏览历史和按名称隔离的 Agent 会话；Agent 面板额外保存结构化 block，使代码、ChangeSet、审批、证据和 Graph 摘要在重启后不退化为普通文本；Tests 面板保存测试行、选中行、滚动偏移、过滤器、摘要与本次运行的源码路径（不保存编译程序或 VM），恢复后仍可跳转、重跑与滚动，并受行数与输出总量上限约束。窗口集合按显示顺序保存每个窗口的类型、标题、布局、活动窗口与窗口属性（占据方式、界面属性、交互类别、生命周期、锚点与模态标记）；单窗口时不占用标签条，两个及以上停靠窗口时标签条占用内容区顶部一行，浮动窗口的矩形随会话恢复。布局槽位由 `SplitAxis::{Rows, Columns}` 决定上下或左右排列，由 `Length::{Fill, Cells, Percent}` 与 `min`/`max` 约束长度。拖动分隔条只改写可伸缩一侧：固定单元格或百分比的一侧保持约束，另一侧改写为 `Fill` 权重。约束无法同时满足时布局优先铺满父容器，不会在父容器内留下空洞。保存时会清理工具参数和 JSON 结果中的常见敏感字段；仍处于待定的审批只恢复为 `interrupted` 展示状态，不恢复响应通道或执行。旧版本会话一律拒绝，不做迁移。会话只保存持久窗口；瞬态浮窗不进入会话。会话不能替代编辑器恢复日志。恢复日志仅追加并带序列与 SHA-256，只有磁盘正文仍匹配基线哈希时才回放。

具体用户操作、脚本能力、Agent 安全及扩展用法见 [Teal 工作台使用手册](../../docs/TEAL-BENCH-MANUAL.md)。实现入口见各工作台包 README。

#### 工作台观测

`teal-security` 与诊断日志分开：原生命令使用用户级独立 SQLite 账本，事务提交后才返回意图写入回执；事件按递增序号和 SHA-256 哈希链连接，并在独立状态行保存链尾。工作区只记录路径摘要，动作正文和外部载荷不入账。写入回执的 `audit.sequence` 也进入当前 tracing 范围的脱敏日志，用于关联一次动作与其跨度。`teal audit list` 支持时间、工作区和操作筛选；`export` 输出 NDJSON，`verify` 检查整链，`clear --confirm` 删除旧链并记录新链起点。默认不自动删除。Agent 运行账本仍负责恢复事实，审计账本通过 run 与操作身份关联，不替代其 Prepared/Unknown 状态机。嵌入宿主通过 `AuditContext` 注入可持久确认的接收器。

根 `teal-bench` 拥有唯一进程 subscriber、本地日志写入器和可选 OTLP logs/traces/metrics 提供器。各库只产生 `tracing` 事件；`teal-workbench-core` 不依赖 SDK。只有命令行显式开关启用导出，endpoint 配置本身不会联网。

本地日志使用容量 8,192 行的有损队列，过载或写入失败通过有界指标和限频提示报告。字段策略允许操作、结果、计数、耗时和工作区相对路径；外部路径、URL、凭据及敏感 JSON 键会处理。自由文本、代码、提示词、工具参数/结果和请求头不输出。日志是诊断元数据，不是无损安全审计。

模型链路按选择、认证预检、登录/目录发现/提交、凭证刷新、激活、物理请求和组装记录脱敏跨度。`teal-bench --model-protocol-capture <DIR>` 另启用本地原始协议抓包；抓包正文与日志和 OTLP 隔离，认证头不进入抓包，接收器故障会停止模型请求。嵌入宿主可向 `teal-model` 注入自己的 `ProtocolCapture`。

排队任务携带来源 tracing 上下文；长期转发任务不继承启动跨度。工作台不向 Agent 或网页请求注入原生追踪头。关闭提供器共享五秒尽力期限，之后刷新本地日志；强制退出不保证最终投递。

工作台源码查询由 `teal-workbench-services::tools::SourceQuerySnapshot` 接收工作区根与打开文档文本，
不持有 UI 句柄。单文件容错索引和完整编译分析使用同一读取边界；普通导入不得越界，内置扩展包由可信目录解析。
Bench 的源码 Agent 适配只处理程序选择、输入、UI 事件和展示；工作区工具登记使用私有操作枚举，
名称、签名、授权能力、效果和恢复元数据只在目录中维护。批准与事务的执行规则不属于查询服务。

#### 工作台包级契约

- 发行版的默认 Agent 入口与 SDLC 示例是普通 Teal 程序；Chat/Task、计划、目标和交付状态由 Agent 字段与 RuntimeStore 保存。Bench 只登记源码入口、发送输入并显示应用事件，模型、审批和文件操作使用 `teal-agent` 与可复用宿主服务。
- `agents/delivery.tl` 从当前命名模型会话读取真实 `teal.fmt_check → teal.check → teal.test → teal.run` 收据，核对调用身份、退出码、路径、源码哈希和程序摘要；随后用 `FileEvidenceProvider` 与 `ProgramDigestProvider` 复查当前文件及导入闭包。未知效果不能当作失败重试，旧收据不得重绑定到新调用。
- `std/agent.tl` 的工作区快照与替换接口复用共享 Mutation 队列；准备阶段固定规范路径、文件 SHA-256 和打开缓冲区版本，授权后重新准备，写入前后都有运行账本和审计边界。CLI 使用 `HeadlessWorkspaceTools`，Bench 额外提供打开缓冲区版本。宿主缺少能力时明确拒绝，不执行其他效果。
- `WindowKind::Tests` 与 Tests 窗口由根应用装配：`TestsWidget` 只保存一次运行的显示投影（按源码分组的 `TestRow`、选中行、过滤器与摘要），跳转与单测重跑以 `AppCommand::Command` 交回命令行（`test-jump`、`test-rerun`），组件本身不编译、不执行也不读写工作区。`:test` 在阻塞线程上编译并运行 `run_program_tests`，结果经 `RuntimeEvent::TestsFinished` 回到事件循环后写入列表与输出面板；窗口只保留显示状态，编译程序与 VM 实例不进入会话。
- 自动化层只使用 `ThreeWayMerge`、工作区/Git 基线和 `ConnectivityMode` 等通用算法；新分支、计划和持续目标使用同一 RuntimeStore run 与 Teal 字段。
- `teal-workbench-automation` 中仅有 capability 的脚本可使用 `execute`；注册了动态路径、网络或应用资源的宿主应使用 `execute_with_grant` 或对应事件入口，把调用方拥有的资源范围与脚本的精确 capability 一起交给运行时。脚本分类由嵌入应用提供：用户配置只自动信任入口规范目录内的用户源码，目录外导入按编译时冻结的实际身份及完整闭包取得精确批准；工作区自动化与内置文法使用各自的信任策略。`extension` 模块提供本地清单、编译快照、独立实例事务与工作台 Teal SDK，后台任务复用 runtime 持久执行门面而不建立第二套副作用账本。
- `teal-web` 只支持 HTTP/HTTPS GET/HEAD，不执行 JavaScript、不保存 Cookie，也不实现 CSS 布局、表单、认证、上传、媒体或 WebView，成功解码后才提交历史变化。`teal-browser` 在导航前为页面启用 CDP Fetch 拦截，并递归附加暂停中的相关 popup、独立 iframe 与 worker target，先为子会话启用 Fetch 再运行；按会话内精确 URL 许可逐请求核对导航、重定向及子资源。未获准请求失败关闭并报告目标，独立批准 `browser.allow_network` 后须重新打开页面；拦截连接丢失则终止受管浏览器进程。每个放行请求审计交给 Chromium 的动作，不在账本保存 URL。`BrowserManager::screenshot` 对当前受管页面视口执行 PNG 截图，要求最新可访问性快照的 generation 并在捕获前后复查页面状态，结果最多 8 MiB，工作台通过 `HostToolImage` 元数据把图片接入 Agent。`teal-computer` 在 Windows 上优先使用 UI Automation 的 Invoke 与 Value pattern，同一窗口快照和动作串行，输入前消费代次；输入开始后的失败报告效果未知。密码元素和过期 generation 失败关闭，其他平台明确返回不可用而不伪装为空窗口集合或成功操作。
- `teal-workbench-core` 不依赖终端、网络、文件服务、LSP 传输或模型供应商；调用方必须先把外部字节/UTF-16 坐标转换为 Unicode 字符位置，并通过 `Transaction` 提交打开文档修改。
