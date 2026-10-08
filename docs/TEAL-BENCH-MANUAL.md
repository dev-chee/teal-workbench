# Teal 工作台手册

本文面向 Teal 工作台使用者，说明如何启动并操作终端工作台：命令行选项、键位与鼠标交互、Teal 脚本与能力、Agent 配置与认证、权限与工作区安全边界，以及本地扩展和 SDLC 示例。产品定位与长期约束见 [Teal 顶层架构](TEAL-ARCH.md)，语言语义见 [Teal 语言手册](TEAL-LANG-MANUAL.md)，crate 职责、内部数据流、持久协议与恢复顺序见各工作台包 README。

本手册只描述当前实现；写“必须”“不能”时表示当前宿主会拒绝该操作，写“通常”“建议”时表示使用建议。命令、键位和配置在源码演进时应与实现一起更新，语言规则仍以语言手册和当前测试为准。

## 1. 启动与命令行选项

```text
teal bench [PATH] [选项]
teal-bench [PATH] [选项]
```

`teal bench [PATH]` 只派发到同目录的独立 `teal-bench`。`PATH` 是文件时主窗口是编辑该文件的 Editor 窗口；`PATH` 是目录或省略时主窗口是 Agent 工作窗口（会话 + 输入框），并预选发行版的 `runtime/agents/default.tl`。Bench 把输入送进该持久 Teal 程序；模型选择、问答、Chat、Task 与交付阶段由程序处理。没有默认预设时，程序列出非秘密候选并等待选择。相同程序也可由 `teal run` 或嵌入宿主执行。省略 `PATH` 时编辑器窗口仍按 `bench.session_restore` 恢复。

启动选项：

- `--safe-mode`：跳过启动脚本、Agent、内置文法和会话恢复；
- `--no-config`：同时禁用用户与工作区 Teal 配置；
- `--offline`：在读取凭据或联网前拒绝远程能力、遥测导出和扩展外部副作用，与 OTLP 导出开关互斥，见 [4.2 交互与会话](#42-交互与会话)；
- `--session <FILE>`：使用显式会话 JSON 文件；
- `--no-mouse`：关闭工作台的鼠标处理，把鼠标选择留给终端。
- `--model-protocol-capture <DIR>`：显式把模型请求正文和原始 SSE 写入独立本地目录，供协议排障使用。

工作台自带本地日志选项 `--log-format text|json`、`--log-dir` 和 `--log-filter`（未设置时读取 `RUST_LOG`），并复用 `--otel-logs`、`--otel-traces`、`--otel-metrics`、`--otel-endpoint`、`--otel-trace-filter`、`--otel-log-filter` 和 `--trace-functions` 观测开关。这些开关默认全部关闭，设置 `--otel-endpoint` 本身不会联网；观测规则见 [teal-bench 工作台观测实现细节](../crates/teal-bench/README.md#工作台观测)。

工作台启动时打开用户级持久审计账本；审批、Agent 模型与工具动作、工作区文件事务会在执行前写入意图并在完成后写入结果。账本不可写时相关动作会被拒绝。审计记录可用 `teal audit list --workspace <PATH>` 查看、`teal audit verify` 校验或 `teal audit export` 导出；它与可能丢行的工作台日志用途不同。

旧 `teal bench|b runs|inspect|profile|resume` 已破坏性删除，不保留兼容转发。`:runs` 打开当前工作区 `.teal/runtime/agent.sqlite3` 的 Runs 面板，负责列表、详情、回答、对账、恢复与 profile 导出；导出需要目标文件的独立写入授权与持久审计，拒绝符号链接目标，审计结果失效时会报告文件效果可能未知。Teal 脚本可在宿主显式安装并授权后使用 `std/runs.tl` 的同一管理接口。

## 2. 键位与鼠标交互

两个内置预设使用同一套 `SelectionSet`、`Transaction` 和撤销树。

### 2.1 Vim 优先

- 普通模式：`h/j/k/l`、`w`、`i`、`a`、`v`、`x`、`d` 加移动命令、`dd`、`u`、`Ctrl-r`、`r`。
- 插入模式：文本、Enter、Tab、Backspace、Delete、Escape。
- 可视模式：移动命令扩展选择；`d`/`x` 删除，`c` 修改。

### 2.2 Helix 优先

相同命令仍然可用，但 `v` 进入选择模式，`x` 在选择优先操作前扩展到下一行。即使预设从单光标开始，多选择始终由核心层表示。

使用 `:keymap vim` 或 `:keymap helix`。工作台同时有一个 Frame（整终端）和若干 Window（工作上下文）：
每个 Window 拥有自己的 `Panel` 布局，两个及以上窗口时顶部显示一行标签条，标签显示序号、工作流类型与标题。
Window 分为两种占据方式：`Docked` 独占内容区并在标签条中显示，`Floating` 以自己的矩形叠加显示；
只有 `Docked` 窗口进入标签条与 `Ctrl-w` 轮转。弹出内容（审批、权限、picker、右键菜单、补全、hover）
统一登记为 `Passive` 浮动窗口：它们不进入标签条与会话，随发起者关闭。
`Ctrl-w` 是窗口前缀，只在普通模式下全局生效：`Ctrl-w w` 或 `Ctrl-w 1..9` 切换窗口、`Ctrl-w n/a/d` 新建编辑/Agent/调试窗口、
`Ctrl-w f` 新建浮动窗口、`Ctrl-w m` 在停靠与浮动之间切换、`Ctrl-w h/l` 收窄/放宽浮动窗口宽度、`Ctrl-w k/j` 降低/增高浮动窗口高度、`Ctrl-w c` 关闭活动窗口。主窗口不可关闭。
拖动浮动窗口的标题栏移动，拖动四条边或四个角调整尺寸；缩到 40x10 以下自动停靠。
面板焦点与拆分改用 `Alt-h/j/k/l`、`Alt-s`、`Alt-v`。

`:window` 管理窗口：`:window` 或 `:window list` 列出窗口，`:window new [agent|debug|editor|tests]` 新建，
`:window float [kind]` 新建浮动窗口，`:window dock`/`:window maximize`/`:window minimize` 调整占据方式，
`:window grow`/`:window shrink` 按固定步长放大/缩小浮动窗口，`:window close` 关闭活动窗口，
`:window next`、`:window prev`、`:window select N` 切换。
鼠标单击标签条同样切换窗口；拖动窗口内部的分隔条只调整该窗口的面板。

各工作流各有默认模板，创建后仍可自由拆分：Agent 窗口是「Session 会话面板 + 输入框」，
Debug 窗口是「编辑器/变量与调用栈 + 控制台」，Tests 窗口是「测试列表 + 输出面板」，
编辑窗口是「资源管理器 ｜ 编辑器 + 输入框」。`:scripts` 打开脚本清单窗口，`:delivery` 打开当前流程的交付记录窗口。
Agent 输入框显示当前
Agent、工作区与权限 preset 的 chips，并随内容在 3–10 行之间增长；在输入框内按 Enter 提交，
Shift-Enter 换行，行首 `:` 交给工作台命令行，行首 `!` 执行宿主命令，Esc 清空输入。Agent
窗口的会话区即使当前聚焦，输入 `:` 也会把焦点移回这个输入框，不再打开底部独立命令行。
Debug 控制台按工作台命令执行，`!` 前缀同样执行宿主命令。

### 2.3 命令路径与保存

命令参数支持单引号和双引号。除单引号内部外，反斜杠会转义后一个字符，因此 Windows 路径应写作 `:open 'C:\work\项目\file name.rs'` 或 `:open 'C:/work/项目/file name.rs'`。相对 `:open` 路径以当前工作区为基准；浏览文件浏览器面板不会改变这个工作区。

`:w` / `:write` 保存聚焦缓冲区到现有路径；`:w PATH` / `:write PATH` 使用相对工作区解析的路径另存为。含空格的路径必须使用引号，多余参数会被拒绝。退出会继续排空已接受的保存请求；超出期限时明确报告未完成操作。

### 2.4 鼠标与剪贴板

鼠标捕获默认启用。使用 `--no-mouse` 启动 Teal 工作台，可把全部鼠标处理留给终端。

- 单击面板可聚焦；滚轮作用于指针所在面板但不改变焦点。
- 拖动分屏边框可调整相邻面板大小；每个嵌套面板保留最小可用尺寸。
- 在编辑器中，单击放置主光标，Shift 单击扩展，Ctrl 单击增删光标，拖动选择，双击选择 Unicode 单词，三击选择逻辑行。
- 滚轮每档滚动三行；横向滚轮和 Shift 加滚轮按四个终端单元滚动长行。CJK、表情符号、组合字素簇和四列 Tab 都使用终端显示单元坐标。
- 文件浏览器在同步预览旁渲染惰性文件树。`j`/`k` 在可见行间移动；`l`/右方向键展开目录或进入第一个子项，对文件则执行打开；`h`/左方向键折叠目录或选择父项，在已折叠根节点上再次向左会把树根提升一级。Enter 和双击会切换目录或打开文件。单击选择，单击展开标记切换目录，Ctrl 单击切换选择，Shift 单击选择可见范围。`.` 切换隐藏条目，`a` 编辑树根地址，`r` 刷新已展开目录。创建以及相对复制/移动目标使用光标目录；光标为文件时使用其父目录。上下文菜单保留移动、回收站和永久删除前的确认。
- Markdown、搜索、智能体、消息、输出和文本网页面板支持鼠标文本选择与复制。浏览器链接行可直接打开或复制。
- 右键打开上下文菜单。编辑器剪贴板操作只作用于非空主选区；空选区复制/剪切不执行。中键会定位编辑器或命令行光标并粘贴纯文本。
- 为保持 Vim 优先与 Helix 优先键义不变，不新增剪贴板键盘快捷键。剪贴板错误显示在消息行中，不会终止 Teal 工作台。

### 2.5 LSP

用 `:lsp rust` 或 `:lsp teal` 启动服务器。请求由已协商能力决定，缺失能力会报告。

| 按键 | 命令 | 效果 |
| --- | --- | --- |
| Ctrl-Space | `:lsp-complete` | 补全选择器；展开代码片段文本并定位初始停靠点 |
| K（普通模式） | `:lsp-hover` | Markdown 符号说明 |
| F12 | `:lsp-definition` | 打开定义或选择位置 |
| Shift-F12 | `:lsp-references` | 引用选择器 |
| F2 | `:lsp-rename` | 提示输入新符号名 |
| — | `:lsp-format` | 应用文档格式化 |
| Alt-Enter | `:lsp-code-action` | 代码操作选择器；执行命令仍须确认 |
| — | `:lsp-semantic-tokens` | 刷新完整/增量语义词元 |

结果绑定来源缓冲区修订，等待结果或排队编辑期间的修改会使其失效。目前补全提供代码片段文本展开与初始停靠点，不提供持续的多停靠点代码片段编辑模式。
代码操作同时包含编辑与服务器命令时，用户确认后先等待编辑作业完成；队列拒收或编辑失败会报告错误并阻止命令。无效语义词元增量会清空该缓冲区缓存并重新请求完整词元。

### 2.6 浏览器与自定义面板

`:browse URL`、`:browse-head URL`、`:browse-find TEXT`、`:browse-refresh` 按需使用聚焦浏览器或新建面板。HTTP 重定向只报告规范新 URL，不自动发送第二次请求；对新目标须再次调用并取得对应网络授权。浏览器 `H/L` 导航自己的历史，`f` 输入数字链接提示，`o` 在外部打开。会话离线恢复，只有显式刷新才重新请求。

`:panel NAME` 打开已注册 Teal 面板。Tab/Shift-Tab 切换控件焦点，方向键导航集合，Enter 激活，输入框接收 Unicode 文本。树还使用左方向键/右方向键折叠/展开，全局普通模式命令/窗口路由仍然可用。

### 2.7 Agent、运行时与调试

- `:agent-setup <provider> <auth-profile> <model> <model-profile>` 更新用户层默认模型 profile，并把选择送给 Teal Agent 程序；`bench.toml` 不保存 Agent 指令。provider/model 必须已在 `model.toml` 声明，认证由独立的 `teal auth login <provider> --profile <auth-profile>` 完成。
- 工作台缺少默认模型时按“供应商 → 认证资料 → 模型”选择；已有完全匹配的模型 profile 会直接复用，没有匹配项时要求输入新的 model profile 名称。
- `:diff [PATH]` 打开 Diff 窗口，比较磁盘内容与打开缓冲区（默认取聚焦缓冲区，也可显式给出工作区内路径）：
输出带 `---`/`+++` 头、`+`/`-`/` ` 前缀与增删行数汇总，完全一致时明确说明没有差异。路径必须位于
工作区内；该视图只读，不写回也不修改缓冲区。
`:evidence` 请求当前 Teal Agent 程序重新核对真实工具收据、文件与当前程序摘要；失效时程序把交付标为 `stale`。没有已验收交付时明确说明。
`:runtime` 检查项目锁、内置扩展包 catalog 与用户缓存；`:runtime lock` 经共享工作区事务写入当前 package 身份。锁文件同时记录核心与工作台扩展 catalog 摘要、语言 edition 和 package 格式；catalog 身份或格式变化会拒绝旧锁，`:runtime install <artifact> <https-url>` 下载并校验锁定制品。
- `:packages` 或 `:packages list` 列出当前发行提供的内置扩展包（名称、版本、摘要前缀），`:packages verify` 核对项目锁、发行包与用户缓存：缺失锁、版本不一致、包集合不一致、知识快照缺失或制品缺失都会逐项报告，一致时给出明确确认。
- `:debug <file.tl>` 在入口暂停；`:debug-continue`、`:debug-next`、`:debug-step`、`:debug-out` 和 `:debug-stop` 控制内嵌调试会话。
  `:break [LINE...|clear]` 替换当前调试源码的完整断点集合，`:debug-frame <INDEX>` 选择要检查的调用栈帧，
  `:debug-eval <EXPRESSION>` 在暂停帧上按 DAP 的纯表达式语义求值（拒绝函数与方法调用，只接受
  `int`/`float`/`bool`/`string` 转换）。调试会话绑定到承载输出的 Debug 窗口；
  每次停下默认检查最内层帧，断点、栈、变量与表达式结果都写入同一窗口；关闭该窗口会终止
  调试会话并清除绑定，迟到的结束事件不会再创建新的输出面板，会话正常结束时同样清除绑定。
- `:test [PATH] [FILTER]` 编译聚焦脚本（或显式工作区内 `.tl` 路径）并运行其登记的 `@test`：
  后台线程为每个测试建立独立实例，Tests 窗口按源码文件分组显示 `ok`/`FAIL`/`TIMEOUT`/`EXIT`，
  选中第一个失败项并在输出面板显示失败说明、源码位置与捕获输出；`Enter`（或 `o`）跳到该测试的
  声明位置，`r` 只重跑选中的测试，`j`/`k`、`g`/`G` 与 `PageUp`/`PageDown` 在列表中移动并保持
  选中行可见，移动时输出面板随之刷新。同一时间只允许一次测试运行，重复 `:test` 会被拒绝。
  没有登记测试的脚本只显示说明。Tests 窗口随工作台会话保存：重启后仍保留测试行、分组、选中行、
  滚动位置、过滤器与本次运行的源码路径，可直接继续跳转或重跑。
- `:agent-imports <file.tl>` 只读分析工作区内的 Teal 文件，将直接导入路径和别名投影为 Agent buffer 的 `ModuleImports` block；越界路径、读取失败或未打开 Agent buffer 时只产生诊断，不执行导入模块。
- 活动 Agent 尚未结束时再次执行 `:agent <text>` 会追加 steering，不会启动并发轮次。
- `:memory` 查看自动提取但尚未召回的候选；`find`、`approve` 和 `delete` 子命令用于审计长期记忆。
- `:goal start <目标>`、`:goal continue`、`:goal stop` 与 `:goal status` 作为输入送入当前 Teal Agent 程序；内建程序最多连续尝试三次修订，遇到重复失败则等待用户。
- `:roles` 显示当前源码 Agent 提供的角色；内建 `Reviewer` 可由模型在当前 run 授权与根预算内创建。
- `:plan` 显示当前 Teal Agent 程序保存的计划文本。计划只用于展示，不作为交付证据。

## 3. Teal 脚本与能力

内置文法与排队脚本通过 Teal 的宿主 `ObservationSession` 复用 [teal-bench 工作台观测实现细节](../crates/teal-bench/README.md#工作台观测)和可选 Meter。这不会授予脚本能力或向 Teal 暴露导出器配置；强类型注册表检查与信任指纹保持不变。

配置按层和类别拆分：安装层位于可执行文件旁的 `runtime/config`（或 `TEAL_RUNTIME_DIR` 指向的目录），用户层位于 `TEAL_HOME`，工作区层位于 `<workspace>/.teal`，优先级从低到高。每层可有四个文件——`config.toml`（权限、信任、知识与记忆策略）、`model.toml`（模型选择预设、认证资料与 provider 模型目录）、`bench.toml`（工作台声明）、`runtime.toml`（语言运行时默认值）。安装层只读，只能提供纯默认值：`config.toml` 只声明知识开关与配额，`bench.toml` 只能声明 `session_restore`，`model.toml` 没有任何安装层键，`runtime.toml` 声明运行时默认值；给用户复制的完整模板在 `runtime/templates/`（用户层全部可写，工作区层只能声明 `knowledge.workspace_root`、`model.toml` 的 `providers.<id>.active_auth_profile`/`default`/`profiles`/`providers` 与工作台的行为声明）。工作区层只有在用户配置记录规范路径为 `trusted` 后才生效；秘密不进入 TOML，而按进程环境、工作区 `.teal/.env`、用户 `TEAL_HOME/.env` 的顺序解析。

`teal config list|get|set|unset|paths` 查看和修改这些文件；`--layer install|user|workspace` 选择目标层，`list`/`get` 会显示每个键实际来自哪一层。旧版本配置一律拒绝读取，也不迁移；必须删除或按当前布局重写。

```toml
version = 14

session_restore = "prompt"

[agent_apps.coding]
script = "agents/coding.tl"

[commands.hello]
script = "hello.tl"
```

`agent_apps` 只登记包含 `task main` 的源码文件。旧 `[agents]` 指令、工具和阶段配置在版本 12 被拒绝；旧 Bench Flow/Goal 会话保留原文件并拒绝续接。主模型选择来自 `model.toml` 的预设，实际选用与命名会话状态由源码 Agent 保存。没有显式选择时工作台使用内建 `default` 程序。

命令、钩子和面板使用声明式脚本路径。相对路径以所属配置目录解析，每次执行目标文件自己的 `fn main` 或 `task main`；共享函数可以通过 `import` 复用。用户级配置仅对自身规范目录内的用户源码导入自动信任；导入目录外源码时须批准完整导入闭包和能力集，任何导入内容变化都会使精确批准失效。工作区脚本的精确快照批准同样在目标或导入闭包变化时失效；开发模式可以改用目录和最大能力集合授权。

上述 `hello.tl` 内容为：

```tl
host "std.bench.editor" as editor { message(string); }
fn main() { editor::message("Hello from Teal Bench"); }
```

Teal 工作台启动时若未提供路径且发现已保存的工作区会话，`bench.session_restore` 控制启动行为：`prompt`（默认值）打开确认对话框，`always` 立即恢复，`never` 从当前目录启动。用户配置先应用，受信任的工作区配置可以覆盖它。

宿主导入会静态解析 `editor.read`、`editor.write`、`ui.panel`、`syntax.register`、`network.http`、`process.spawn` 和 `agent.configure` 等能力标签。用户配置及其注册、真实路径位于用户配置目录内的脚本被视为用户代码。目录外与工作区来源的脚本仍需独立批准。`ask` 模式绑定规范路径、源码闭包和精确能力；`develop` 模式可绑定规范目录和最大能力集合，目录内源码变化继续有效，新增能力或目录外导入会重新请求批准。

显式 `:source` 先相对工作区解析并规范化，再按精确用户配置路径或需要批准的脚本分类。相对路径、普通 Windows 路径和 verbatim 路径不能改变信任类别；`:source!` 在 `ask` 模式批准当前完整导入闭包，在 `develop` 模式批准入口目录和当前最大能力集合。安全模式不自动加载配置，但仍允许用户主动执行显式 source 命令。

自动化脚本可用模块为 `std.bench.config`、`std.bench.syntax`、`std.bench.editor`、`std.bench.network` 和 `std.bench.process`。`std.bench.config` 只服务用户显式执行的动态自动化；启动配置的权威入口是 TOML。安全模式跳过总体配置；显式 source 命令仍属于用户主动操作。Agent profile 只在配置中保存秘密变量名，秘密值位于环境或 `.env`。

工作区可以把脚本登记到 `.teal/scripts.toml`（格式 v1）：每个登记项声明名称、相对工作区的
入口 `.tl`、运行时配置档，以及可选的权限文件、测试入口和交付产物。登记项只描述脚本本身，
不授予任何权限，也不会自动运行。`:script` 或 `:script list` 列出脚本，`:script new NAME`
在 `scripts/NAME.tl` 生成带 `fn main() -> int` 的骨架并打开它，`:script open NAME` 打开入口，
`:script profile NAME` 打开配置档选择器（内置 `core`/`native`/`agent` 与配置中声明的自定义
profile），`:script profile NAME PROFILE` 直接指定配置档。名称只接受 ASCII 字母数字与 `-_.`，
入口必须是工作区内的 `.tl`，越界路径与重复名称会被拒绝。

### 3.1 工作区与工具链能力

导入 `bench/tools.tl` 可调用与内置 Agent 工具相同的工作区、工具链和文本网页实现：

```tl
import "bench/tools.tl" as tools;

task main() { workflow().await; }
task workflow() {
    let source = tools::read_file("src/main.tl").await;
    let matches = tools::search("TODO").await;
    print(source.len() + matches.len());
}
```

模块提供 `read_file`、`search`、`write_file`、`replace_file`、`run`、`diagnostics`、
`teal_check`、`teal_fmt_check`、`teal_test`、`teal_run` 和 `fetch`。`diagnostics("")` 以 JSON 字符串返回整个工作区的
诊断，非空路径只返回对应文件。文件路径仍限制在活动工作区；写入和替换进入根应用的共享
事务队列并保留检查点，进程调用仍需对应信任或交互批准。脚本信任基于实际使用推导出的
`editor.read`、`editor.write`、`process.spawn` 和 `network.http` capability，不因导入模块自动授予。
`teal_test(path, runtime_profile, filter)` 运行脚本登记的 `@test` 测试：`filter` 为空时运行全部，
否则只运行名称包含该子串的测试；退出码非零表示存在失败或超时，没有登记测试时返回成功并说明原因；
测试证据与 `teal.check` 使用同一程序摘要。该工具与 `teal.check`、`teal.fmt_check`、`teal.run`
一样在 Task 中默认直接可见，`develop` 档位在工作区内自动放行（与 `teal.run` 同规则）。

这些函数是普通 Teal task，调用方必须使用 `.await`。Agent 目录由同一组
`HostFunctionDefinition` 和工具导出元数据生成，不再维护另一套参数 Schema 或回调；脚本和模型
仍使用各自的授权入口，脚本信任不能代替模型工具批准。

标准包（`git`、`http`、`oauth`、`feishu`）与工作台脚本使用同一授权模型：RuntimeProfile
只决定实现可用性，不授予权限；实际可访问的程序、文件、网络端点和环境变量仍由 capability 与
资源授权决定，工作区信任不会扩大这组授权。接口、依赖与示例见
[Teal 语言手册的标准包](TEAL-LANG-MANUAL.md#标准包)。

`bench.mcp_servers.<name>` 接受 `http` 或 `stdio`；HTTP 的参数数组必须为空，stdio 参数以
TOML 字符串数组声明且不经过 shell。配置不得包含凭证，认证由宿主提供。Agent Hook 还可注册 `agent.before_model`、`agent.after_model`、`agent.before_tool`、
`agent.after_tool`、`agent.before_compaction`、`agent.after_compaction` 和 `agent.finished`。
模型循环内只执行用户配置 Hook，返回非零退出码表示拒绝；工作区 Hook 不会在循环中绕过新批准。

### 3.2 标准终端界面

任意安装内置扩展包 catalog 的 Teal 嵌入环境可导入 `std/ui.tl`，获得类型化 `Node`、`Length`、`Style`、`UiEvent` 和 `Component`。在 Teal 工作台中导入 `bench/ui.tl` 可使用相同词汇以及 `render(root)`、`event()`，宿主能力为 `ui.panel`。

完整组件集合为行、列、网格、文本、按钮、输入框、列表、表格、树、进度条和占位元素，对应枚举变体为 `row`、`column`、`grid`、`text`、`button`、`input`、`list`、`table`、`tree`、`progress`、`spacer`。长度支持自动、单元格、百分比和填充，分别对应 `auto`、`cells`、`percent`、`fill`；语义样式为 `normal`、`muted`、`accent`、`success`、`warning`、`error`、`code`、`heading`。容器拥有子节点，每个节点和树条目须有非空、唯一且稳定的 ID。

用 `bench.panels.<name>.script` 声明工厂/事件入口，然后用 `:panel name` 打开。检查 [ui-demo.tl](../examples/showcases/bench/ui-demo.tl) 及其导入后，从仓库工作目录执行 `:source examples/showcases/bench/ui-demo.tl`，先独立批准 `examples/showcases/bench/ui-demo-panel.tl`，再执行 `:panel demo`。

Rust 在替换当前视图前校验版本 1 JSON 树：至多 2 MiB、2,048 节点、深度 32。无效树保留上一有效视图并报告错误。渲染不访问文件/网络。保留的 ID 继续拥有焦点、输入值、选择和树展开状态；持久化控件状态至多 256 KiB。

事件包括 `mount`、`focus`、`key`、`activate`、`change`、`selection`、`resize`、`tick`。桥接传递拥有所有权的字符串字段，`ui::event()` 校验并解码类型化枚举。Tab/Shift-Tab 循环焦点，方向键改变列表/表格/树选择，左方向键/右方向键折叠/展开树节点，Enter 激活，输入控件编辑 Unicode 文本。鼠标使用最近一次渲染几何。

### 3.3 钩子生命周期与工作线程

`bench.hooks.<name>` 的脚本数组仅接受：

- `app.started`、`app.stopping`、`workspace.opened`；
- `buffer.opened`、`buffer.changed`、`buffer.saved`、`buffer.closed`；
- `panel.focused`、`diagnostics.changed`、`lsp.started`；
- `agent.completed`、`session.restored`。
- `agent.before_model`、`agent.after_model`、`agent.before_tool`、`agent.after_tool`；
- `agent.before_compaction`、`agent.after_compaction`、`agent.finished`。

未知名称使激活失败。缓冲区变化防抖 100 ms；钩子不重入，回调造成的变化不会递归触发相同 changed 钩子。自定义面板定时事件会合并。串行工作线程队列限制为 128 个任务、8 MiB 捕获缓冲区正文。显式脚本绑定捕获的缓冲区 ID/修订，过期界面替换被拒绝。可信脚本可以执行耗时工作；工作线程不是操作系统沙箱，也不提供抢占式 CPU 上限。

交互 `:source`、注册命令、钩子、面板事件在终端循环之外编译/执行，启动配置在交互前执行。信任哈希包含完整导入闭包、规范标识、实际使用的内置扩展包/module/IR 身份和精确能力集。旧的仅根文件授权记录失效，导入文件改变需重新批准。重新加载删除该脚本的过期注册项。

自定义面板 UI envelope v1 仅在注册可用后恢复最近有效树/状态，不会自动执行未注册工厂。
它是工作台会话中的一项面板负载，不是会话文件版本。缺失注册时省略该面板，其他面板
仍恢复；显式会话文件必须与当前工作区匹配。

自定义输入框内可打印按键（含 `:`）编辑输入值。Escape 离开文本输入以使用全局命令，Enter、Tab 或鼠标点击恢复控件交互。

## 4. 工作台 Agent、认证与工作区安全

本地 Teal 扩展、工作模式和 SDLC 示例见 [本手册的 Teal 扩展与 SDLC](#5-teal-扩展与-sdlc)。持久 SDLC 主示例是普通源码 Agent；工作台扩展 SDK 仍用于短事件、命令、Hook 和面板。

Teal 工作台直接使用 teal-agent 的认证、模型协议、类型化工具、权限和取消契约。profile
是 provider 下的认证快捷键，秘密来自进程环境、用户/受信任工作区 `.env` 或外部凭据源；
会话和日志只保留非秘密账户摘要。工作台启动和普通编辑不要求认证，只有使用模型 Agent 时才统一预检。

### 4.1 配置与登录

工作台始终提供内建 `default` 源码 Agent；用户配置和受信任工作区配置还可登记多个源码程序。Agent 行为由 Teal 文件定义，模型选择来自
`model.toml` 的共享默认值：

~~~toml
# model.toml：模型预设选择 provider、认证资料和默认模型
version = 14

[profiles.work]
provider = "codex"
auth_profile = "codex-cli"
model = "gpt-5.6-terra"

[providers.codex.auth_profiles.codex-cli]
method = "codex-cli"
credential_managed = false
available_models = ["gpt-5.6-terra"]

[providers.codex.models."gpt-5.6-terra"]

default = "work"
~~~

~~~toml
# bench.toml：只登记 Agent 程序入口
[agent_apps.coding]
script = "agents/coding.tl"
~~~

工作区配置只有在用户首次确认信任后才能覆盖同名用户配置。配置只接受秘密环境变量名，
不接受密钥值。Agent 的认证 `profile` 可省略；此时创建会话时解析默认预设并固定其认证资料。
显式切换既有会话时若账户改变，会在发送旧历史前确认。如果模型预设目录已经存在但没有 `[default]`，首次从 Agent 输入框提交时会打开
`Select Model` 列表；接受后写入同一配置层的默认选择并自动继续刚才的请求，取消则不发送。

`:agents` 选择活动 Agent 程序（初始为内建 `default`），`:agent-config [user|workspace]` 打开
`bench.toml`。工作台不提供供应商
登录界面；Chat、Task、首次行内补全和扩展模型任务在创建模型、会话或 run 前执行共享预检。
未认证请求停止并提示准确的 `teal auth login <provider> --profile <profile>` 命令，用户在
外部完成认证后可重新发起请求。

### 4.2 交互与会话

`:agent <输入>` 把文本送到当前源码 Agent 程序。内建 `default` 程序以 `std/agent.tl` 的 `receive().await` 等待下一轮输入，用 `emit` 发布文本和状态，并在支持 UI 的 Bench 中用 `render(ui::Node)` 显示视图。同一程序也可由 `teal run runtime/agents/default.tl` 或 Rust 嵌入宿主运行。无终端输入时，原 run 与 VM 检查点进入等待状态；Runs 面板或已授权的 `std/runs.tl` 可以回答并恢复。回答绑定请求 ID 和代次，不等于工具授权。

内建程序的普通输入使用 `self.chat` 的 String/Text 完成。`:agent task <目标>` 创建独立命名会话并使用 `self.think` 的 Explicit/TaskDelivery 完成；自由文本和 JSON 外观文本不能提交 Task。内置扩展包 `agents/delivery.tl` 从真实 ToolCall/ToolResult 核对 `teal.fmt_check`、`teal.check`、`teal.test`、`teal.run` 的顺序、成功退出码、脚本路径、源码哈希、运行配置和程序摘要，再读取当前文件并重新编译导入闭包。文件存在或退出码为零不代表业务结果正确。

`:agent task --with-chat-context <目标>` 先展示有界脱敏的 Chat 投影；确认后才作为低信任背景进入新 Task，来源会话在等待期间变化会拒绝转移。`:agent task-status`、`:agent task-resume`、`:agent task-abandon` 管理源码程序中的交付。`:evidence` 再次运行同一 Teal 验证器，发现文件或程序变化时标记为 `stale`。`:task-commit <说明>` 再次验证并请求绑定脚本哈希、程序摘要和提交说明的批准；可信服务还会重查原批准、真实工具身份与当前文件，再用隔离 Git index 提交精确交付路径。该命令不运行 Git hooks，也不推送。

`:plan` 显示源码 Agent 的计划；`:goal start <目标>`、`:goal continue`、`:goal stop` 和 `:goal status` 由 Teal 程序决定推进。内建程序最多连续尝试三次修订，重复失败后等待用户。`:agent-branch [list|create <name>|switch <name>|save <name>]` 使用 RuntimeStore 中的独立模型会话并绑定当前 Git/工作树及可见缓冲区摘要。通用编辑器的 `:agent-merge` 与 `:agent-merge-verify` 仍提供三方合并候选和 ChangeSet 审查。

Agent 回复中的完整 Rust/Teal fenced code block 可作为编辑器块打开；`:agent-insert <编号>`、`:agent-write <编号> <路径>` 与 `:agent-run <编号>` 使用工作区版本检查和原有审查动作。`:complete word|line|all` 是独立编辑器补全，不共享源码 Agent 的任务会话。工作区工具写入、浏览器、桌面、终端和 MCP 仍由宿主按资源、批准和审计边界提供；配置中的工具目录不会自动授予权限。

`teal bench --offline` 保留本地编辑和不需要模型的 Agent 命令；远程模型与源码 Task 在认证与网络请求之前被宿主拒绝，明确标为本机的预设仍可用于 `self.chat`。旧 Bench Flow、Goal 和对话文件不会被转换成源码 VM 检查点；普通启动明确拒绝续接并保留原文件，安全模式跳过这些状态。当前配置版本 14 只接受 `agent_apps.<name>.script` 登记源码入口。
`:diagnose [test] <crate>` 使用 Cargo JSON 消息而不是终端文本，保留主/附属 span、准确行列与
错误码，并为工作区内源码附加最多七行上下文和 `git blame` 来源。`:agent-context <编号>` 打开
对应源码位置。`:agent-view <ast|types|data|effects> <path>` 使用编译器生成且不进入字节码的
分析图；节点 ID 只在同一源码快照内稳定，容错分析与完整编译分析会明确区分。
源码引用、导入、符号、调用关系和类型查询优先使用当前未保存缓冲区。分析视图的入口与普通导入
使用同一次查询快照；未打开的文件首次从磁盘读取后固定，内置扩展包仍通过可信包目录解析。

`:diagnostics` 打开 Diagnostics 窗口，聚合并行语言服务器与分析器上报的诊断：按 URI、行列与
严重度排序，逐条给出 `uri:line:column [severity] (source) message`，末尾汇总错误、警告与其它
数量；没有记录时明确显示空状态。该视图只读，不触发新的分析请求。
源码查询不会自动保存或执行程序，越出当前工作区的普通导入会被拒绝。


### 4.3 工具与权限

内置工具按职责分为：

- 工作区读取与修改：`workspace.read_file`、`workspace.list`、`workspace.stat`、`workspace.read_range`、`workspace.search`、`workspace.write_file`、`workspace.replace_file`、`workspace.apply_patch`、`workspace.apply_changeset`、`workspace.create_dir`、`workspace.move`、`workspace.delete` 和 `workspace.diagnostics`；其中批量 ChangeSet 使用 JSON 描述至多 64 个 `path` 与 unified diff `patch` 条目，可选的 `expected_sha256` 与 `new_sha256` 在提供时逐个核对，缺省时由宿主绑定。用户可以接受整组、单文件或单 hunk，也可以按同样粒度拒绝；任意拒绝都不写盘，模型必须据此生成新的候选变更。
- 工具链与进程：`teal.check`、`teal.fmt_check`、`teal.test`、`teal.run`、`workspace.run`、`process.exec`、`process.start`、`process.status`、`process.wait` 和 `process.terminate`；
- 网络与浏览器：`web.fetch` 以及 `browser.start/open/allow_network/snapshot/click/fill/close`；受管浏览器在实际请求前逐个核对会话内的精确 URL 许可，未获准的重定向或子资源会报告目标。批准 `browser.allow_network(session, url)` 后须重新打开页面；
- Windows 桌面：`computer.list_windows/snapshot/invoke/set_value/raw_click/raw_keys`。

前三项 Teal 工具链调用直接使用同一发行中的二进制。共享权限策略支持单次、当前任务和
当前工作区三种批准范围；等待期间仍绑定精确参数、资源版本和策略代次。`ask` 模式请求
敏感范围确认；可信工作区的 `develop` 模式自动允许工作区内常规读取与编辑、Teal 工具链
和结构化本地 Git 操作。任意 shell、原始键鼠和工作区外访问不因开发模式自动放行。
`explore` 模式只用于方案探索：工作区内的只读检查（读取、搜索、诊断、`teal.check`、
`teal.fmt_check`）自动放行，写入、执行和其他外部效果直接拒绝，不进入批准提示，因此
探索阶段的模型无法改动工作区。
这些工具从 Teal 工作台的类型化 Host 注册表导出，与 `bench/tools.tl` 脚本模块复用同一实现、
capability、参数和结果契约。`workspace.diagnostics` 的 `path` 必填，空字符串表示整个工作区。
写入和替换的 Host 映射会把模型可见结果与应用检查点分离，检查点不会发送给模型。
工具链结果另外返回规范路径、配置档、退出码以及源码/程序摘要，供任务交付校验；这不会把一次
自由问答自动升级为脚本任务。
只读调用可并行；工作区、进程、浏览器和桌面修改按模型顺序串行。任意进程执行每次都显示精确批准，
未知工具、无效 Schema、工作区外路径和歧义替换均拒绝。

模型另外拥有两个没有任何外部资源的探索工具：`record_proposals` 记录目标下的候选方案与权衡，
`adopt_proposal` 采纳其中一个候选。它们不写工作区、没有可审批资源，因此在 `explore`
只读档位下同样可用；两者与 `update_plan` 一样只服务于展示、恢复与推进判定，不是交付证据。

`:permissions [show|explore|ask|develop|revoke]` 管理工作台当前工作区策略。Agent、脚本和扩展复用
同一判定；`ChannelApproval` 只收集类型化答复。持久授权只写入用户配置，工作区配置不能
批准自身。撤销会递增策略代次并清除目录信任；恢复重新加载当前策略，不恢复旧 grant。

`bench/browser.tl` 只控制工作台自身启动的临时 Edge/Chrome profile。`BrowserPage` 的底层 session
与页面身份保持私有，调用方可通过 `url()`、`title()` 和 `generation()` 查询非秘密元数据。
CDP 接口没有任意 JavaScript 入口；可访问性节点绑定页面 generation，点击或填充后旧引用失效，
密码输入必须由用户在可见窗口中完成。Chromium 自身沙箱保持启用，但它不能替代 Agent 的 Host 授权。

`bench/computer.tl` 在 Windows 上优先使用 UI Automation 的 Invoke 和 Value pattern。`ComputerWindow`
只公开 `process_id()`、`title()` 和 `generation()` 查询，底层窗口句柄保持私有。窗口、元素 RuntimeId
与 generation 共同组成资源身份；快照或交互后旧引用失效。原始鼠标和按键位于
`computer.raw_input` 独立 capability 下，不作为语义操作失败后的自动回退。其他平台返回明确的不支持错误。

打开文档的写入经过共享缓冲区事务，关闭文档使用同一原子写入器；写入前创建检查点。
打开结果和资源修改按规范路径代次协调，旧读取不能覆盖较新的磁盘或缓冲区状态。文件
浏览器可浏览工作区外，但 Agent 与 LSP 的修改权限不会因此扩大。

`:agent` 在已有活动轮次时把文本作为 steering 追加到下一个安全边界，而不是启动并发轮次。
用户级与工作区级记忆保存在不同 SQLite 文件中，召回内容显示来源且不能覆盖 `AGENTS.md`。
自动提取内容先作为不可召回的候选保存；`:memory` 查看候选，`:memory find <query>` 检索已批准
记忆，`:memory approve <user|workspace> <id>` 批准，`:memory delete <user|workspace> <id>` 永久删除。
`:memory show <user|workspace> <id>` 显示记录的证据、适用范围、验证时间和失效原因；批准要求至少
一条宿主可复核证据，用户批准本身会记录为 `user_statement` 证据。工作区或工具链摘要变化后，关联
记录标记为 stale 并停止自动召回。
`:memory maintain` 是显式维护入口：它只从已经完成且有确定证据的记录提取未验证候选，不改写可信
或已抑制记录，也不改变召回开关；重复执行同一来源返回既有账本。模型辅助提取需要用户配置启用
`memory.model_assisted`、工作区已经信任，并只使用一次受限维护预算（默认一轮模型请求、有界候选数）。
用户和工作区 `.agents/skills` 只预载名称与说明，正文按需加载。Teal 配置可以显式登记无秘密
MCP 连接和 Agent 生命周期 Hook；未登记或未获 Grant 的工具不会发送给模型。

`:knowledge` 显示内嵌官方制品身份与用户级、工作区级两层各自的状态（层根、文件数、字节数、缓存
命中与刷新错误）；`:knowledge refresh [user|workspace|all]` 重新读取层根下的文档并按配置写回派生
缓存；`:knowledge find <query>` 搜索各层条目；`:knowledge read <id>` 输出有界正文。
`:knowledge proposals`、`:knowledge show <id>` 与 `:knowledge retry-review <id>` 检查和重试 Agent 知识草稿；
草稿在来源调用的安全边界自动交给内置审核 Agent，通过后由宿主按当前写权限发布。审核模型必须在
用户 `model.toml` 中配置独立的 `knowledge-review` 预设；缺失时草稿保持待审。用户级层位于
`<teal_home>/knowledge`，工作区级层默认位于 `.teal/knowledge`，只在用户层把该工作区记为 trusted 时
装载，且逐文件复用 `workspace.read` 权限。Agent 写入 `.teal/knowledge/` 下的文档后会自动刷新工作区
层，失败只告警。官方知识来自构建期内嵌制品，不受工作区信任影响。

新机器可用 `teal auth login <provider> --profile <auth-profile>` 完成认证和模型目录同步，再使用
`teal model set <provider> <auth-profile> <model> --model-profile <name>` 创建默认模型预设，或使用
`teal model use <name>` 切换已有预设。`:runtime` 检查
`.teal/runtime.lock` 中的当前 Teal 版本、核心与工作台扩展 catalog 摘要、package 格式与制品；`:runtime lock` 经共享工作区事务创建或刷新锁文件。catalog 身份或格式变化会拒绝旧锁。缺少锁定制品时，
用户可执行 `:runtime install <artifact> <https-url>` 下载到用户缓存，工作台会先校验 SHA-256，
不会解压、运行制品或修改系统 PATH。

观测规则见[teal-bench 工作台观测实现细节](../crates/teal-bench/README.md#工作台观测)：对话、代码、工具参数/结果和
凭证不会进入日志或 OTLP，观测数据也不能作为会话恢复来源。

`--model-protocol-capture` 的抓包文件独立于普通日志和 OTLP，每次请求单独建目录；其中可能包含完整提示词、工具结果、图片和私有模型状态，需选择仅本人可访问的目录并自行清理。认证头不会写入抓包。单次请求正文上限为 64 MiB，响应上限为 8 MiB；抓包创建、写入或大小限制失败会使模型请求报错。`status.json` 中的 `complete=false` 表示记录不完整。

## 5. Teal 扩展与 SDLC

工作台使用 Teal 编写命令、Hook、面板与工作模式。当前 SDLC 主示例是 [`examples/showcases/bench/sdlc/agent.tl`](../examples/showcases/bench/sdlc/agent.tl)：它用持久 `task main` 处理目标、意图、规格、计划、业务批准、实现和交付核验；可用 `teal run` 单独执行，或在 `bench.toml` 的 `[agent_apps.sdlc]` 登记并由 Bench 交互驱动。目录中的 `extension.toml` 和其他脚本保留为工作台扩展机制示例，它们不是 Agent 应用的运行事实来源。

### 5.1 加载与使用

在仓库目录启动工作台，先验证简单扩展：

```text
:extension load examples/showcases/bench/hello-extension
:extension-hello Hello
:panel extension-hello
```

首次加载展示扩展身份、源码指纹和能力集合。精确批准绑定清单、SDK、入口与完整导入闭包；开发模式可批准扩展目录和最大能力集合。目录内源码更新会清除绑定旧代码的任务和业务审批，再安装新快照；新增能力或越界导入仍需批准。加载只编译并安装入口，不执行入口程序。

```text
:extension load examples/showcases/bench/sdlc
:mode sdlc
:agent Add a small feature with measurable acceptance criteria
:extension status sdlc
:extension stop sdlc
:extension resume sdlc
```

也可通过 SDLC 面板输入目标和修订要求。`:mode chat`、`:mode task` 返回原生模式；切换模式不取消后台任务。重新打开工作台后，配置中的扩展重新加载，运行必须显式恢复。放弃实例时先停止运行，再执行 `:extension abandon sdlc`；业务状态归档，RuntimeStore 不被删除。

`:extension reload <id>` 重新预检已加载扩展；运行中不替换代码。源码与已保存实例不匹配时拒绝继续，首版没有运行中实例自动迁移。`:extension unload <id>` 注销该扩展的入口并保留状态。

### 5.2 清单与总体配置

扩展目录中的 `extension.toml` 声明 `id`、`version`、`api_version = 1`、`capabilities`，以及 `commands`、`hooks`、`panels`、`modes`、`tasks` 五组名称到入口文件的映射。每个入口执行自己的 `fn main` 或 `task main`，共享函数通过 `import` 复用。入口必须位于包内；未知 API 版本、重复贡献和未声明能力使安装失败，保留上一套有效注册。

总体 TOML 引用扩展并覆盖无秘密配置；相对路径以声明它的配置文件目录解析：

```toml
[bench.plugins.sdlc]
path = "F:/teal/examples/showcases/bench/sdlc"

[bench.plugins.sdlc.config]
agent = "coding"
checks = '["build","test","lint","eval"]'
max_revisions = "3"
environment = "staging"
"command.build" = '{"program":"cargo","args":["check","--workspace"]}'
"command.test" = '{"program":"cargo","args":["test","--workspace","--all-targets"]}'
"command.lint" = '{"program":"cargo","args":["clippy","--workspace","--all-targets","--all-features","--","-D","warnings"]}'
"command.eval" = '{"program":"cargo","args":["test","-p","teal-runtime","--test","language_regressions","documentation_tests"]}'
```

这些命令适用于本仓库；其他项目应填写自己的真实检查。Eval 阈值由项目命令负责，失败必须返回非零退出码。示例不以空命令或 `echo` 伪造通过。

`agent` 引用已有工作台命名 Agent，空值使用任务启动时的活动 Agent；`agent.<任务名>` 可以为验证等任务指定不同 Agent。扩展先异步预检，再重新校验源码、配置与批准，之后才以解析出的具体 provider、profile、model 和固定指令创建 run 绑定。恢复只使用已保存绑定，不重新解析活动 profile。配置不能保存秘密值。

命令配置是 JSON 对象：`program`、`args`、`cwd`、`timeout_ms`、`inputs`、`operation`、`environment`。程序和参数分开传递，默认工作目录为工作区，默认超时十分钟，最大一小时。发布命令应设置 `operation = deploy` 和匹配的环境；未配置 `command.deploy` 时，SDLC 等待外部发布结果。

### 5.3 扩展 SDK

导入 `bench/extension.tl`：

| 接口 | 契约 |
| --- | --- |
| `event(key)`、`config(key)` | 读取捕获事件与无秘密配置；缺失返回空字符串。 |
| `get(key)`、`set(key, value)` | 访问实例业务数据；短事件正常完成后统一提交。 |
| `start(name, input)` | 启动清单声明的后台入口，返回稳定任务键。 |
| `start_bound(name, input, approvals_json)` | 执行前重新验证指定审批与文件摘要。 |
| `task_status(key)` | 返回捕获的运行状态；展示不代替收据。 |
| `cancel(key)`、`resume(key)` | 在事件提交后控制本实例拥有的任务；仍有未知外部效果时保留取消请求，先对账再取消。 |
| `complete(value)` | 后台任务提交候选结果；runtime 正常终止后才交付。 |
| `request_approval(prompt, paths_json, operation, environment)` | 请求宿主审批，路径参数为 JSON 字符串数组。 |
| `approval(id)` | 返回 missing、pending、approved 或 rejected。 |
| `quote(value)`、`message(value)` | 安全编码 JSON 字符串或显示普通消息。 |

短事件负责分发与状态变化；耗时工作放入后台入口，用普通 `task`、`.await` 和 Agent 编排。后台任务内部使用语言自身的 Task/Agent 控制，不通过事件 SDK 控制外层实例。

清单中的 `task_tools` 限制各任务模型可见的工具；`task_write_state` 将写入范围绑定到实例状态中的精确路径 JSON 数组。SDLC 使用已批准计划的文件清单，并拒绝覆盖流程启动前已有的用户改动。

模式事件包含 enter、leave、input、approval、task_completed 和 task_failed 操作。结果事件携带任务名、任务键、运行 ID 和显式结果。面板复用 `bench/ui.tl` 的组件与事件。

导入 `bench/evidence.tl`：

- `capture(paths_json)` 返回文件摘要；明确删除的文件使用 `absent` 标记。
- `command(name, approval).await` 返回真实退出码、输入摘要、输出摘要及捕获输出；配置不是执行授权。
- `git(action, paths_json, message, approval).await` 支持 begin、status、commit、commit_verified。

`commit` 要求文件集合与审批相同。`commit_verified` 的第二个参数是含 `paths` 和 `checks` 数组的对象：宿主读取自身保存的收据，核对全部配置检查成功、配置未变、源码摘要仍相同，再提交。已批准制品所在目录保存流程报告，不作为待测试业务源码。默认检查输入为 Git 跟踪及未忽略的新文件，排除运行状态和已批准制品目录；大型仓库可设置明确的 `inputs`。

Git 使用独立参数。初始脏文件、非空用户暂存区、分支变化、无关暂存内容或陈旧证据阻断自动提交。推送、合并和发布不由本地提交隐式执行。

### 5.4 SDLC 闭环

1. 提出目标，确认建立专用本地分支。
2. 连续生成 intent.md、spec.md 和 plan.md；一次评审最终计划、修改范围与验证命令，批准后本地提交三份制品。
3. 按计划文件范围构建，运行配置检查，再以独立上下文进行逻辑、安全和合规评审。
4. 验证失败返回构建，默认最多自动修订三轮；超限等待人工输入。扩大计划时重新审批。
5. 宿主核对真实检查收据后提交本任务文件，保存 delivery.json。
6. 独立批准发布动作，或输入 `external-deployment <外部记录>`，确认对应提交与环境。
7. 输入 `import-alert <证据>` 只读诊断；分诊接受后进入新循环，要求增加永久回归或 Eval。

输入 `revise intent ...`、`revise spec ...`、`revise plan ...` 修订对应阶段。`close` 结束业务实例；后台任务应先停止或结束。部署失败不自动回滚，用户依据实际证据选择恢复操作。

制品位于 `intent/<flow-id>/`。示例不提供常驻监控、Git 托管平台连接器或生产探测；外部确认保留用户提供的来源，不伪装为工作台实际执行。

### 5.5 扩展的持久化与恢复

RuntimeStore 是 VM、模型与外部操作的唯一运行账本。扩展状态保存业务值、审批、任务引用和事件确认；活动文本、面板和模式属于展示状态。旧 `:source` 与声明式脚本通过 legacy 适配执行，原自动化状态与扩展实例隔离。

稳定启动键由实例、事件和任务序号构成。先分配并保存运行身份，再执行。runtime 已成功终止而业务事务未完成时，只重投递原结果；业务状态与任务交付标记一起提交，重复结果不再推进阶段。

失败运行中的候选结果不能作为完成。未知 Host、模型或命令结果不会自动重做，需要当前授权下的查询证据。`std/user.tl` 的回答只提供信息，不签发业务或工具授权。缺少证据、代码改变、绑定不一致或不支持恢复时，保持阻断并显示原因。

工作线程和工作区路径限制不是操作系统沙箱。终端输入与模型等待不互相阻塞，但无限 CPU 脚本或第三方子进程的退出仍受运行时和平台约束。验收应分别说明本地协议夹具、真实终端、真实模型和外部业务的验证范围。

实现机制、存储版本和 crate 边界见 [teal-bench 工作台实现细节](../crates/teal-bench/README.md#teal-工作台实现)。

## 项目指令与协作角色

受信任工作区的 `AGENTS.md` 会按工作区根到当前目标目录加载。来源、目录和内容摘要进入
Agent 上下文；新目录规则生效前，旧响应中尚未执行的工具会跳过并重新决策。撤销信任后
停止加载项目规则，原有工具审批不会因此扩大。

角色由源码 Agent 的 `@role` 声明和 Agent 运行时角色目录定义；Bench 配置只登记
`agent_apps.<name>.script`，不再维护第二套角色字段。角色创建和模型访问复用既有批准界面，
子角色问题使用相同问答队列。角色任务状态可由模型协作工具查询；在子任务仍运行或需要处理
未知效果时，父调用不能提前提交完成。

### 交互终端与图片工具

Agent 可以发现 `terminal.start/read/write/wait/resize/close`。启动给出程序、独立参数、工作目录、环境、行列与缓冲大小，返回一次性会话 token；后续读取使用返回的 cursor，避免重复回传历史。stdin 写入再次走现有审批。退出工作台会回收自有进程，重启不会重新运行旧命令。用户问答继续使用原有问题面板，不使用终端输入代替。

`workspace.read_image` 读取获准工作区内的 PNG/JPEG；`browser.screenshot` 截取已有受管页面的视口，需要最新 snapshot 的 generation。所选模型须在模型配置中明确声明 `images`，图片不支持时在下一请求前停止。历史保存图片引用并验证摘要，不隐式下载远程图片。

### 计划与持续目标

内建源码 Agent 将 `agent.update_plan` 顶层 task 导出为模型工具，计划文本由 Teal 程序保存并通过 `emit` 展示。计划保存在同一源码 run 中，不能充当交付证据；Task 仍需通过 `agents/delivery.tl` 验证。

主模型请求在上下文容量允许时包含一份有界、版本化的运行状态 JSON：模型提交的计划步骤状态、宿主已确认的阶段与 Graph 摘要、本次调用的模型轮次和工具调用数，以及根预算的已结算、预留、未知用量。两种计数口径分开显示。工作台在后台观察本任务已经访问的文件及打开缓冲区版本、作用域内项目指令、工作区与用户级配置、权限版本和同目录 Teal 工具链；文件监测只读取元数据。外部变化在下一安全边界通知 Agent，与待执行资源相关时未开始的工具会成对标记为跳过并交给模型重新决策。Agent 自己的文件修改照常通过工具结果反馈；监测状态不构成授权或交付证据。重启后会重建监测基线，停机期间的变化不能被声称为已逐项观察。

`:goal start <目标>` 由源码程序创建目标并启动 Task；同一 run 的根预算跨命名会话累计。内建程序连续修订最多三次，重复失败或交付不通过时标记为等待。重开 Bench 时，从 RuntimeStore 的原 VM 检查点续接；`:goal continue` 是显式继续输入，不会因启动自动新发模型请求。

一次调用用完本地模型轮次额度时会保存包含全部工具结果的安全边界：`:agent task-resume` 或目标推进
会用同一根预算身份发起新的调用并携带完整历史，本地轮次重新计数，而根预算用量继续累计；根预算
用尽、等待审批或出现未知副作用时不会继续。保存的安全边界不包含在途请求，因此不会重放未知效果。
