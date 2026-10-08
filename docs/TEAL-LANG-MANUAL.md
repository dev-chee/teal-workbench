# Teal 语言手册

> **适用版本：** 当前仓库基线（Alpha，语言 edition 2026）
> **规范性质：** 使用者参考；示例以 `.tl` 文件表示

本手册是 Teal 的语言参考和入门教程的合体版本。它采用“先运行一个程序，再逐层说明
词法、类型、表达式、控制流、声明、标准库和工具”的顺序；每一节都尽量给出可复制的
代码、成功条件和失败边界。文中写“必须”“不能”时，表示当前编译器应拒绝或当前运行时
不会提供该行为；写“通常”“建议”时，表示风格或可移植性建议。

本手册只描述当前实现，不把路线图、内部 crate、字节码字段或实验分支当成语言能力。
需要了解编译器、VM、Host、Agent 和工作台内部契约时看 各相关 package 的 README；需要操作 Teal 工作台
时看 [Teal 工作台使用手册](TEAL-BENCH-MANUAL.md)。如果手册、测试和实现
不一致，应记录最小复现并修正文档或实现，而不是依赖未说明的行为。

## 如何使用本手册

* **第一次学习：** 依次阅读第 1、2、3、5、6、7、8、9 章，然后按需要查阅集合、类、
  枚举和错误处理章节。
* **查语法：** 先看对应章节的“合法形式”和示例，再看第 23 章的摘要。摘要是便于检索
  的近似文法，不替代语义规则。
* **查运行环境：** 第 16–21 章说明内建函数、文件系统、Host、动态库、CLI、LSP 和 DAP；
  它们的可用性还取决于宿主操作目录、RuntimeProfile、平台和授权。
* **判断是否可用：** 本手册把“当前可执行”“仅能解析”“未实现”分开标记。解析器接受
  某个 AST 形状，并不等于类型检查、加载或运行时接受它。

示例中的 `// 编译错误` 表示有意展示拒绝边界；示例中的 `cargo run -p teal` 假定在
仓库根目录执行。命令行帮助和标准包目录可能随版本增加内容，语言规则以本手册与当前
测试共同为准。

## 目录

- [1. 引言](#1-引言)
- [2. 第一个 Teal 程序](#2-第一个-teal-程序)
- [3. 词法约定](#3-词法约定)
- [4. 程序结构、入口和作用域](#4-程序结构入口和作用域)
- [5. 值与类型](#5-值与类型)
- [6. 变量、赋值和初始化](#6-变量赋值和初始化)
- [7. 表达式和运算符](#7-表达式和运算符)
- [8. 控制流](#8-控制流)
- [9. 函数和闭包](#9-函数和闭包)
- [10. 数组、映射和字符串](#10-数组映射和字符串)
- [11. 类](#11-类)
- [12. 枚举和匹配](#12-枚举和匹配)
- [13. 错误处理](#13-错误处理)
- [14. 静态变量](#14-静态变量)
- [15. 模块、导入和命名空间](#15-模块导入和命名空间)
- [16. 内建函数](#16-内建函数)
- [17. 内建方法](#17-内建方法)
- [18. 文件系统命名空间](#18-文件系统命名空间)
- [19. 安全宿主函数](#19-安全宿主函数)
- [20. 动态库接口](#20-动态库接口)
- [21. 命令行和宿主差异](#21-命令行和宿主差异)
  - [21.5 LSP 语言服务器](#215-lsp-语言服务器)
  - [21.6 DAP 调试适配器](#216-dap-调试适配器)
- [22. 诊断、限制和编写建议](#22-诊断限制和编写建议)
- [23. 语法速查](#23-语法速查)
- [24. 示例索引与综合示例](#24-示例索引与综合示例)

### 参考约定

本手册把一个 Teal 程序的生命周期分成四个可观察阶段：

1. **解析**：字符被读成词法单元和语法树；这一阶段只判断形状是否像 Teal。
2. **编译**：名称、类型、控制流、能力和入口约束被检查，并生成可校验的程序。
3. **加载**：VM 校验程序格式、类型目录和宿主边界；不满足契约的程序不会执行。
4. **执行**：入口、全局初始化、函数、任务、Host 或 Agent 调用按宿主策略运行。

因此，“语法上可以写”不自动表示“可以编译”，而“可以编译”也不表示当前宿主已经
授予所需资源。每个涉及外部效果的章节都会分别说明这四层边界。

语法片段使用以下记号：大写单词表示非终结符，反引号中的文字表示关键字或标点，`?`
表示可选，`*` 表示重复，`|` 表示选择。完整片段只在描述该节边界时展开；跨章节规则
（例如名称解析、类型兼容和 capability）以正文为准。

## 1. 引言

Teal 是一门小型、静态类型、表达式导向的脚本语言。源码在执行前完成解析、类型检查和
程序校验；`teal` 提供命令行运行环境，`teal-fmt` 提供格式化，`teal-lsp` 提供编辑器
静态分析，`teal-dap` 提供源码级调试。格式化和语言服务不会执行程序，调试器执行时仍
遵守与普通运行相同的类型、Extension 和 Host 授权边界。

Teal 已支持与 `fn` 统一声明的 CPU `task`、源码 `agent`、类型化消息、Graph 委派与 `self.think`。Agent
实例拥有独立私有状态和串行邮箱，不同实例可以并行；认证、模型调用和持久恢复由宿主装配。
具体用法见本手册 9.5 节；内部模型循环和恢复协议见 `crates/teal-agent/README.md`。

与 Lua 手册类似，本手册从词法、值和类型讲到完整语言结构，再列出标准运行环境提供的
函数和独立运行程序。两门语言有一些重要区别：

- Teal 是静态类型语言，变量的类型由标注或推演固定；
- Teal 根源码必须声明私有、无参数的顶层 `fn main` 或 `task main`；
- `none` 是可显式书写、存储和传递的单值类型；
- Teal 用同质数组、带类型的映射、名义类和枚举，而不是用一种 table 表示所有数据；
- Teal 没有 Lua 的真值转换规则、元表、协程、多返回值或可变参数语法。

当前版本是 Alpha。文件扩展名不由编译器强制；仓库示例使用 `.tl`，CLI 也接受
`.teal`。语言表面可能比语义层更早接受某些写法；本手册会明确标出“仅解析”边界。
动态库依赖本机平台和 C ABI。

根源码和所有导入模块必须在文件头显式声明 `@edition("2026");`，并且必须使用相同
edition；缺失声明时编译器直接报告迁移诊断 `teal::edition`。

### 1.1 章节地图

| 目标 | 先读 | 关键问题 |
| --- | --- | --- |
| 写出可运行脚本 | 2、3、4 | 文件如何启动、名称在哪里可见、哪些字符是合法源码 |
| 理解类型和表达式 | 5、6、7 | 类型如何推演、`none` 与未初始化有何区别、运算和转换何时失败 |
| 编排程序 | 8、9、12、13、14、15 | 控制流、函数、任务、枚举、错误、静态值和模块如何组合 |
| 使用标准能力 | 10、16、17、18、19、20 | 集合、内建函数、文件系统、Host 和动态库的边界 |
| 接入工具链 | 21、22 | CLI、嵌入、序列化、观测、LSP、DAP 和诊断如何工作 |

示例优先展示最小完整程序；省略的 `main` 只表示该片段应放入一个已有函数中。需要
复制运行时，请把片段放进文件并补上根入口、导入和宿主授权。

## 2. 第一个 Teal 程序

保存为 `hello.tl`：

```tl

task main() {
    let message = "Hello from Teal";
    print(message);
}
```

运行或只检查：

```powershell
cargo run -p teal -- run hello.tl
cargo run -p teal -- c hello.tl
cargo run -p teal -- run --code ' task main() { print(42); }'
```

编译器直接把根文件 `main` 编译为入口帧。命令行参数通过 `argv()` 取得。
每个文件的顶层 `fn main` 或 `task main` 是该文件专用入口，必须无参数且不能声明 `pub`、被调用、取得函数值或重导出。只有根文件的入口自动执行，导入文件的入口不执行。两种入口都使用整数退出码契约：省略返回类型等价于 `int`；显式 `-> int` 也允许自然结束和裸 `return;`，两者统一返回 `0`，显式返回值必须是整数。`task main` 可以直接使用 `.await`，不需要额外的入口包装任务。

```powershell
cargo run -p teal -- run hello.tl -- first second
```

```tl

task main() {
    print(argv());
}
```

## 3. 词法约定

### 3.1 源文件、空白和标识符

源码是 UTF-8 文本。空格、制表符和换行用于分隔词法单元；代码块由花括号界定，Teal
不是缩进敏感语言。

标识符目前只接受 ASCII：

```text
[a-zA-Z_][a-zA-Z0-9_]*
```

`answer`、`_private`、`item2` 合法，中文变量名会在词法阶段失败。单独的 `_` 是保留
词法单元，不是标识符：它只可用作丢弃绑定、`_ = expression;` 的丢弃目标或 `match`
通配模式。`_private` 等带下划线前缀的名称仍是普通标识符。函数、static、类型、字段、
枚举、模块别名、限定名称和普通值表达式需要名称时都不能使用单独的 `_`。

### 3.2 注释

`//` 开始单行注释，直到换行。当前没有块注释或嵌套注释；字符串中的 `//` 是普通文本。

```tl
// 文件注释

fn main() {
    let value = 1; // 行尾注释
}
```

### 3.3 关键字

| 类别 | 关键字 |
| --- | --- |
| 声明与可见性 | `fn`、`task`、`agent`、`let`、`const`、`static`、`class`、`struct`、`newtype`、`enum`、`dylib`、`import`、`as`、`pub` |
| 保留声明字 | `struct`、`type` |
| 控制流 | `if`、`else`、`match`、`for`、`in`、`loop`、`break`、`continue`、`return`、`try`、`catch` |
| 值 | `true`、`false` |
| 保留状态字 | `none` |
| 基础类型 | `int`、`float`、`bool`、`string` |

`struct`、`type` 和 `newtype` 用于值类型、别名和名义包装；`agent` 与 `task` 已有独立声明身份。
上述关键字均不能作为标识符，带前后缀的名称（如 `struct_value`、`task2`）仍是普通标识符。
`while` 不是关键字，可以作为函数名、参数名或变量名；条件循环使用 `loop` 配合
`if` 和 `break` 表达。
循环标签也不是关键字：`'outer` 是独立标签词法单元，标签名遵循普通 ASCII 标识符规则，
因此不能使用精确 `_` 或关键字拼写。

### 3.4 逗号与分号

函数参数、调用参数、数组元素、类字段、枚举变体和映射项用逗号分隔，很多位置允许末尾
逗号。普通语句必须以分号结束：

```tl

task main() {
    let numbers = [1, 2, 3,];
    numbers.push(4);
    print(numbers);
}
```

`if`、`for`、`loop`、`try`、`match` 和裸块本身不跟分号。表达式形式的 `if` 位于
声明、赋值、调用或 `return` 中时，外层语句仍需分号。

### 3.5 数字字面量

`int` 是有符号 32 位整数，范围 `-2147483648` 到 `2147483647`。负号是单独的单目
运算符；词法器为最小整数拼写保留了专门边界处理。

`float` 是 64 位 IEEE 754 浮点数。当前只支持十进制点形式，不支持指数、十六进制或
类型后缀：

```tl
let integer = 42;
let minimum = -2147483648;
let first = 1.25;
let second = .5;
```

`1.` 会被读作整数 `1` 后跟点号，应写 `1.0`。所有经过 Teal 值构造边界的浮点
`-0.0` 会规范化为 `0.0`；负零位型用于合法的 `none` 值，未初始化使用独立内部标记；脚本不能
依赖或观察该表示。所有 NaN（无论来源、符号或载荷）也规范化为同一个负 quiet
NaN 值，因此 Teal 中 NaN 彼此相等并具有一致哈希；正静默 NaN 位区间专供内部
NaN 装箱标签。

### 3.6 字符串字面量

字符串用双引号包围，是 UTF-8 文本：

```tl
let escaped = "line\nnext\t\"quoted\"\\path\r\0";
let language = "Teal";
let version = 1;
let message = "Welcome to ${language} ${version}";
```

已定义转义为 `\n`、`\t`、`\r`、`\\`、`\"` 和 `\0`。未知转义会原样保留反斜杠和
后继字符，例如 `"\q"` 的内容仍是 `\q`。当前没有单引号字符串、原始字符串或多行
字符串。

表达式位置的字符串可以用 `${变量路径}` 插值。路径允许普通变量、`::` 限定变量和类字段链，
例如 `${name}`、`${config::version}`、`${user.address.city}`，也允许路径首尾有空白。
占位符不接受运算、调用、索引或格式说明符；这些值应先存入变量：

```tl
let sum = left + right;
let text = "${left} + ${right} = ${sum}";
```

每个占位值按照 `string(value)` 的现有规则转换，因此整数、浮点、布尔、字符串、数组、
映射、类和枚举都可使用。未初始化的值仍会产生编译错误；合法 `none` 显示为 `none`。多个占位符按
源码顺序转换和连接；插值结果的类型始终为 `string`。

`${` 开始占位符，单个 `}` 结束占位符。用 `{{` 和 `}}` 输出字面花括号，
例如 `"{{${name}}}"` 的结果是包围在花括号中的 `name` 值。`import` 路径、`dylib` 路径和
`catch` 错误过滤器是静态字符串，不能插值，但其中的字面花括号同样需要双写。Attribute
参数和映射键原有的编译期字面量限制不会因插值而放宽。

## 4. 程序结构、入口和作用域

### 4.1 顶层项目

源文件顶层只允许：

- 文件头构建选项及检查项级别；
- `import "path";`
- `pub namespace::member (as alias)?;`
- `pub? static name = expression;`
- `pub? const NAME: Type = expression;`（类型标注可省略）
- `pub? class Name { ... }`
- `pub? enum Name { ... }`
- `pub? trait Name { ... }`
- `impl Type { ... }` / `impl Trait for Type { ... }`
- `dylib "library" { ... }`
- `pub? fn name(...) (-> Type)? { ... }`

顶层不能直接放 `print`、`let` 或循环。根源码必须提供自己的 `fn main` 或 `task main`，库文件可以没有入口。

编译器先收集模块级类型、函数和导入，再编译函数体，因此顶层函数可在声明位置之前调用，
递归和互相引用也能进入类型推演。静态初始化仍按收集到的源顺序执行，推荐写清依赖关系。

`fn`、`static`、`const`、`class`、`enum` 和 `trait` 默认只在当前文件可见；紧邻声明关键字前的 `pub` 将其
直接导出给导入者。Attribute 必须写在可见性之前，例如 `@tag pub fn helper() {}`。
顶层 `pub namespace::member;` 或 `pub namespace::member as alias;` 可选择性重导出已导入模块
的一个 public 成员。普通 class 字段可以单独声明 `pub`；`pub` 不能修饰导入、动态库、
impl 容器、局部声明、Agent 字段、枚举变体或枚举载荷字段。

### 4.2 Entry

每个文件的顶层 `fn main` 或 `task main` 是该文件专用入口，必须无参数且不能声明 `pub`、被调用、取得函数值或重导出。只有根文件的入口自动执行，导入文件的入口不执行。两种入口都使用整数退出码契约：省略返回类型等价于 `int`；显式 `-> int` 也允许自然结束和裸 `return;`，两者统一返回 `0`，显式返回值必须是整数。`task main` 可以直接使用 `.await`，不需要额外的入口包装任务。

`@entry`、`@entry_point`、CLI `--entry` 和 Rust `with_entry` 已删除，旧入口选择器不再接受。

### 4.3 Extension、RuntimeProfile 与 Lint

编译器用统一 registry 描述选项的类型、默认值、来源权限、作用域、合并方式、阶段和帮助
文本。`teal-c options|profiles|lints` 显示当前构建的编译目录；
`teal-perm operations` 显示统一权限操作目录。

源码不声明运行时功能。编译器根据最终可达代码自动推导功能扩展：

- 文件、环境、时钟、进程、网络和纯数据调用分别使用对应功能扩展；
- 动态库 C ABI 调用使用 `dylib`；安全 Host 调用绑定可信目录中的功能；
- Agent 元数据和 Agent 调度使用 `agent`；普通 CPU `task` 属于 core。

`RuntimeProfile` 按能力递增分三档，只回答运行环境能否提供这些实现机制：

- `core`：空扩展，只运行纯 CPU 程序；
- `native`：显式组合当前全部本机功能扩展，但不包含 Agent；
- `agent`：在 native 的显式成员上增加 `agent`，也是配置模板的默认档。

未调用函数和优化移除的外部调用不会扩大程序的最小 Extension 集合：

```tl
import "std/fs.tl" as fs;


task main() {
    print(fs::exists("config.tl").await);
}
```

```text
teal run program.tl --runtime-profile native --permission-file permissions.toml
```

```rust
use teal_compiler::{CompileOptions, RuntimeProfile};

let options = CompileOptions::default().with_runtime_profile(RuntimeProfile::native());
```

编译结果记录最终可达代码需要的最小 Extension 集合及首次使用位置。`RuntimeProfile`
只检查所选宿主是否支持这些功能，不授予操作、dylib 库/符号、模型、工具或资源权限。

根源码还可在文件头设置构建标量，重复时后写覆盖前写，CLI/宿主是最终层：

```tl
@profile("release");
@compiler_debug(false);
@debug_info(true);
```

这些指令写在导入文件时仍完整校验，但不改变构建结果；每条产生默认 warn 的
`ignored-compiler-option`。导入文件可以用文件级检查项调整它，CLI 仍可最终覆盖。

Lint 级别为 `allow < warn < deny < forbid`：

```tl
@warn("unused-variable", "unused-import");
@deny("unreachable-code");

@allow("unused-parameter")
fn callback(value: int) {}
```

优先级是 registry 默认、文件级、声明级、CLI/宿主。声明不能降低祖先源码的 `forbid`，
但最终 CLI 可降低。支持 `configuration`、`unused`、`correctness`、`style`、`naming`、`all`
分组；同一作用域后写覆盖前写。声明级检查项支持模块命名函数、static、类、
枚举、特征、动态库、impl 容器及 impl 方法，不支持导入、重导出、字段、变体、let 或
匿名函数。Warn 随成功结果返回；deny/forbid 收集全部违规，第一个为主错误，其余为 related。

命名默认 warn：类、枚举、特征使用 PascalCase，const 使用 SCREAMING_SNAKE_CASE，其他 Teal 名称使用 snake_case，允许
前导 `_`。前导 `_` 同时抑制对应 unused 绑定；精确 `_` 仍是 discard。`@feature` 和
`FeaturePolicy` 已删除，不提供兼容层。

首批检查项完整目录：

- configuration（默认 warn）：`ignored-compiler-option`；
- unused（默认 warn）：`unused-import`、`unused-variable`、
  `unused-parameter`、`unused-private-function`、`unused-private-static`、`unused-private-const`、
  `unused-private-type`、`unused-private-method`、`unused-label`、`unused-pure-result`；

其中 `unused-private-method` 只报告**外部不可达**的成员：固有成员需要类型与成员都 `pub`，
trait 实现成员的可见性继承自 trait，导入的 trait 一定是公开的；trait 声明里的成员与参数是
契约（没有函数体、没有调用点），不参与未使用检查，未实现且私有的 trait 只由
`unused-private-type` 报告。
- correctness（默认 warn）：`unreachable-code`、`constant-condition`、`duplicate-branch`；
- style（默认 allow）：`shadowed-binding`、`redundant-type-annotation`、
  `needless-conversion`、`redundant-else`、`needless-bool-comparison`、`empty-block`、
  `single-arm-match`；
- naming（默认 warn）：`non-snake-case-function`、`non-snake-case-method`、
  `non-snake-case-variable`、`non-snake-case-parameter`、`non-snake-case-field`、
  `non-snake-case-variant`、`non-snake-case-static`、`non-snake-case-import-alias`、
  `non-snake-case-label`、`non-pascal-case-type`、`non-upper-case-const`。

### 4.4 Attribute

Attribute 是附着在源码声明上的编译期元数据，写在目标声明之前：

```tl
@attribute
class Route {
    method: string,
    path: string,
}

@Route { method: "GET", path: "/status" }
fn main() {}
```

`@attribute` 只能无参数标注顶层普通 `class` 或 `enum`，用于声明可作为自定义属性的名义
类型。Class 属性以 `@Type { field: value }` 构造；enum 属性以 `@Type.variant` 或
`@Type.variant { field: value }` 构造。字段检查、嵌套 class/enum、函数调用以及 Host 调用
完全复用普通 global initializer；global init 当前不能处理的表达式在属性中也同样拒绝。

每次自定义属性应用都会编译成一个不进入源码作用域的隐藏 global。VM 启动时按普通全局
初始化顺序构造真实的 class/enum 实例；初始化失败会在进入 `main` 前终止执行。该实例是
普通 GC 根，同一 VM 内的多次投影共享其对象身份和可变字段，不同 VM 各自拥有独立实例。

运行时代码使用类型投影取得实例。`reflect::item(name)` 中的裸名称只在当前可见声明中
唯一时成立；局部 `let` 变量不参与查找。同名声明必须使用声明式类别前缀，例如：

```tl
let route = reflect::item(main).@Route;
print(route.path);
route.path = "/health";

let route_from_static = reflect::item(static main).@Route;
let route_from_function = reflect::item(fn main).@Route;
let route_from_method = reflect::item(fn Router::route).@Route;
let route_from_associated = reflect::item(static fn Router::default).@Route;
```

投影中的 item 与属性类型都在编译期解析，结果保持具体 `Route` 类型，因此可直接访问字段、
调用固有或 trait 方法以及继续访问嵌套自定义实例。同一 item 对同一自定义属性类型最多关联
一个实例；不同属性类型可以组合。可用 `fn`、`task`、`static`、`class`、`agent` 和 `enum`
前缀选择顶层目标；实例方法使用 `fn Type::method` 或 `task Type::method`，关联方法使用
`static fn Type::method` 或 `static task Type::method`，trait 方法可用
`fn <Type as Trait>::method` 形式。`reflect::item(...)` 不是可独立保存的一等运行时句柄，
必须紧接属性类型投影。

运行时 Attribute 可标注 `static`、`class`、`enum`、`dylib`、顶层 `fn`，以及 impl 中实际
编译为函数的实例方法和关联函数。`import` 和选择性重导出不能附加任何属性。
编译器检查项属性另可标注特征和 impl 容器，但不会作为运行时自定义属性保留。
Trait 方法签名、字段、枚举变体、let 和匿名函数不能标注。
导入模块定义的属性类型使用限定名，例如 `@metadata::Route { path: "/" }`；投影同样使用
`reflect::item(main).@metadata::Route`。编译结果保留属性目标源码范围，并把自定义属性关联到
保存实例的隐藏 global。内建属性继续使用各自的位置参数语法，不产生运行时属性实例。

内建 `@test` 把顶层普通函数登记为可执行测试：

```tl
@test
fn addition_is_commutative() -> int {
    assert_equal(1 + 1, 2);
    return 0;
}
```

测试函数必须是声明为无参数的普通 `fn`，不能是 `task`，也不能标注 `static`、`class`、
`enum`、`dylib` 或 impl 成员等其他目标；返回类型必须显式写成 `-> int`，其值就是该测试
实例的退出码，`0` 表示通过。编译器在函数签名解析完成后校验这些条件并给出诊断，通过的
应用以 `test` 为名的内建属性元数据保留在程序中，供工作台与根命令行测试运行器枚举。

独立 `teal-test [PATH] [--filter <SUBSTRING>] [--format text|json] [--profile <PROFILE>]` 运行这些
测试：每个测试在独立 VM 实例上调用一次，按源码声明顺序报告结果，退出码 `0` 视为通过；
非零退出码、未捕获错误或超出指令预算分别报告为失败或超时。断言可直接使用标准包
`std/test.tl`：

```tl
import "std/test.tl" as test;

@test
fn additions_hold() -> int {
    test::assert(1 < 2);
    test::assert_equal(1 + 1, 2);
    test::assert_not_equal("left", "right");
    return 0;
}
```

断言失败会抛出普通可捕获错误（`assertion_failed`、`assert_equal_failed` 等前缀），因此测试
内部仍可用 `try`/`catch` 检查失败；未捕获时测试运行器把它报告为该测试的失败，而不影响
同一批次的后续测试。

`attribute`、`test`、`model` 和 `options` 是保留的内建名称；编译控制指令和四种检查项级别
也不能用作自定义属性。

### 4.5 词法块作用域

每对花括号创建词法块。`let`、循环变量和 catch 绑定只在相应作用域可见；闭包通过 `let` 获得的名称遵守同一规则：

```tl

task main() {
    let outer = 1;
    {
        let inner = outer + 1;
        print(inner);
    }
    // print(inner); // 编译错误
}
```

绑定从声明之后开始可见；同一名字可由更内层或更晚的声明遮蔽，离开块后恢复外层名字。
声明初始化器在新绑定进入作用域之前求值，因此 `let value = value + 1;` 的右侧读取外层
`value`。闭包捕获的是创建时解析到的绑定身份，后续同名声明不会改写已有捕获。

丢弃绑定 `_` 不进入任何作用域，不能读取、赋值、shadow 或捕获。它只要求相关表达式
按正常顺序求值并丢弃结果。

## 5. 值与类型

### 5.1 类型一览

| 类型写法 | 值示例 | 说明 |
| --- | --- | --- |
| `int` | `42` | 32 位有符号整数 |
| `float` | `3.14` | 64 位浮点数 |
| `bool` | `true`、`false` | 条件只接受布尔值 |
| `string` | `"hello"` | UTF-8 字符串 |
| `T[]` | `[1, 2]` | 同质数组 |
| `{K: V}` | `{"a": 1}`、`{}` | 带键和值类型的映射 |
| `Point` | `Point { x: 1 }` | 名义类 |
| `struct Point` | `Point { x: 1 }` | 字段不可重新赋值的值记录 |
| `Box<T>` | `Box { value: 1 }` | 编译期单态化的泛型 struct |
| `newtype UserId(string)` | `UserId("user-1")` | 不允许隐式转换的名义包装 |
| `State` | `State.ready` | 名义枚举（需声明 `enum State { ready }`） |
| `fn(A) -> R` | 匿名函数 | 函数/闭包类型 |
| `A \| B` | — | 编译期联合类型 |
| `none` | `none` | 正常完成但无具体结果时的单值类型 |

### 5.2 静态类型、推演与专门化

变量类型来自显式标注或首次成功推演：

```tl
let explicit: int = 1;
let inferred = "text";
```

函数也可以声明显式泛型参数；调用点按实参类型单态化：

```tl
fn print_identity<T>(value: T) {
    print(value);
}


task main() {
    print_identity(1).await;
    print_identity("one").await;
}
```

编译器可为两次调用生成不同实现；这不是运行时动态类型，每个专门化仍经过类型检查。
泛型参数可以带静态 trait bound，例如 `fn render<T: Printable>(value: T) -> string`。

值类型也可以声明泛型参数，字段中的 `T` 在构造和字段读取处替换为实际类型：

```tl
struct Box<T> { value: T }
fn unwrap(value: Box<int>) -> int { return value.value; }
```

泛型 struct 使用名义类型身份和精确类型实参；`Box<int>` 与 `Box<string>` 不能互相赋值。

同一默认规则适用于每个表达式。表达式能推演出具体类型时保留该类型；一次推演内部可为
递归、泛型或尚未确定的容器部件暂存未知信息，但单个表达式最终仍无法形成具体结果时，
报告无法推导类型，不能把未知类型当作 `none`。空数组和空映射已经具有可继续细化的容器类型，不因部件
尚未确定而把整个容器视为 `none`；显式类型上下文也仍可在表达式被消费前确定其具体类型。

### 5.3 联合类型

`A | B` 表示一个槽位在静态上允许多种候选；括号可分组类型，例如 `(int | none)[]` 表示每个元素可为 int 或 none 的数组：

```tl
let value: int | string;
value = 1;
value = "one";
```

联合也会由控制流返回、静态初始化或容器元素推演产生。它是编译期概念，VM 的本地/dylib
类型系统没有通用联合值，因此联合不能直接作为动态库 ABI 类型。

目标为联合时，传入值只需被一个候选接受；把联合值赋给非联合槽位时，所有可能成员都
必须被目标接受。

### 5.4 `none`

`none` 是只有一个值的正常类型。任何正常完成的表达式都有结果；没有具体结果时返回
`none`。它可以显式书写、绑定、传参、返回，并存入字段、数组和映射：

```tl
fn complete() -> none {}
task main() {
    let value: none = complete();
    let optional: int | none = value;
    let values: none[] = [none];
    print(value, optional, values[0]);
    print(none == none); // true
    _ = value;
}
```

VM 常量池中的值必须完整初始化，包括合法 none；动态聚合通过运行时构造填充，未初始化标记只属于存储槽位。

`none` 不表示未初始化。`let value: none;` 在赋值前不能读取，`let value = none;` 则已初始化。
`none` 支持相等比较、哈希、打印、字符串转换和插值，文本为 `none`；不支持算术、排序或
隐式布尔转换。抛错或控制流终止不产生正常结果。动态库仍通过省略返回类型声明 C `void`，
不能将 `none` 用作 C 参数类型。安全 Host 可以传递 none；JSON 和模型数据将其编码为 `null`，
包括模型根结果。字段缺失仍与字段为 null 不同，JSON 对象键仍须是字符串。

### Result 与 `?`

`Result<T, E>` 表示成功值或结构化错误值。`Result::ok(value)` 和 `Result::err(error)`
要求调用点能够确定两个类型参数；`?` 只接受 `Result<T, E>`，成功时取出 `T`，失败时把
同一个 `E` 返回给当前函数。它不会隐式转换错误类型，跨类型转换必须显式完成。

```tl
fn read() -> Result<string, string> {
    return Result::ok("ready");
}

fn forward() -> Result<string, string> {
    let value: string = read()?;
    return Result::ok(value);
}
```

`Result` 的成功和错误载荷可以用模式匹配读取：

```tl
fn show(result: Result<string, string>) -> string {
    match result {
        Result.ok { value } => { return value; }
        Result.err { error } => { return error; }
    }
}
```

### 5.5 `typeof`

`typeof(expr)` 返回稳定静态类别，如 `"int"`、`"array"`、`"function"` 或命名类/枚举
名称。它不执行参数表达式，可查询未初始化绑定。保留字 `type` 不是可调用函数。

```tl

task main() {
    let inferred;
    let typed: int;
    // inferred 尚无类型，不能观察为 none；首次赋值后才确定类型。
    inferred = none;
    print(typeof(inferred)); // none
    print(typeof(typed));    // int
}
```

二者都是编译期类型观察，不是运行时反射；查询表达式不会为了类型结果执行副作用。

### 5.6 `T | none` 的收窄

当绑定的静态类型是"一个具体成员加 `none`"的联合时，证明它不是 `none` 的写法会把该绑定
在对应区域里按具体类型对待，不需要额外的解包函数或强制转换：

```tl
fn value(flag: bool) -> int | none {
    if flag { return 7; }
    return none;
}

fn doubled(input: int | none) -> int {
    if input == none {
        return 0;            // 提前离开：之后的 input 是 int
    }
    return input * 2;
}

fn describe(input: int | none) -> string {
    if input != none {
        return string(input + 1);   // 成立分支内 input 是 int
    }
    return "missing";
}
```

规则：

- `if x != none { A } else { B }`：A 内 `x` 是具体类型；
- `if x == none { A } else { B }`（没有 else-if 时）：B 内 `x` 是具体类型；
- `if x == none { A }` 且 A 不可能继续（以 `return`、`break`、`continue`，或所有分支都如此的
  `if`/`else` 链结束）时，该语句之后的同层代码里 `x` 是具体类型，因此 `loop` 里的
  `if value == none { break; }` 之后可以直接使用 `value`；
- 比较写在任一侧都可以（`none == x` 等价于 `x == none`）；
- `match x { none => …, _ => … }`：`none` 字面量臂之后的各臂内 `x` 是具体类型，`none`
  臂自身和它之前的臂保持联合类型（穷尽规则不变，联合类型仍要求最终 `_`）；
- 收窄只作用于简单变量，不作用于字段、索引结果或调用结果；多成员联合（例如
  `int | string | none`）不参与收窄；
- 在收窄区内给该变量赋值会撤销收窄：赋值按绑定的真实联合类型检查，之后重新按联合使用；
- 收窄只影响类型检查，不改变绑定存放的运行时值，也不跨越函数或闭包边界。

需要按 "没有值 / 有值" 分支时可以直接 `match`：`none` 臂之后的臂内该变量已按具体成员使用
（`match` 的模式仍是表达式比较，不会绑定 `T` 本身）：

```tl
let current = value(true);
match current {
    none => { print("missing"); }
    _ => { print(current + 1); }   // `none` 臂之后：current 是 int
}
```

## 6. 变量、赋值和初始化

### 6.1 命名声明与丢弃声明

```tl
let inferred;
let typed: int;
let initialized = 1;
let typed_and_initialized: int = 2;
```

- `let x;`：处于未初始化状态，第一次赋值确定类型；
- `let x: T;`：类型固定为 `T`，值仍未初始化；
- `let x = expr;`：由初始化器推演并固定类型；
- `let x: T = expr;`：检查初始化器能否赋给 `T`。

命名位置也可写为 `_`，但丢弃绑定必须有初始化器：

```tl
let _ = compute();
let _: int = compute_int();
let _ = print("kept side effect");
```

这些写法都对初始化器恰好求值一次并立即释放结果，不创建变量。类型标注仍按赋值兼容
规则检查；结果为 `none` 的表达式也可以直接丢弃或通过 `: none` 标注。`let _;` 和 `let _: T;`
在解析阶段失败。丢弃未初始化变量和类型不兼容仍会失败。

只有沿所有可达路径都赋值后，变量才可求值：

```tl

task main() {
    let value: int;
    if input() == "yes" {
        value = 1;
    } else {
        value = 2;
    }
    print(value);
}
```

缺少 `else` 时不能证明初始化。循环可能执行零次，因此只在循环体内赋值通常也不足以
证明循环后的变量已初始化。

### 6.2 类型固定与 shadow

普通赋值不能改变绑定类型。需要新类型时，声明同名新绑定：

```tl
let value = 1;
// value = "text"; // 编译错误
let value = "shadowed";
```

已创建的闭包继续捕获旧绑定，不会因后续 shadow 改为捕获新绑定。

### 6.3 赋值目标和复合赋值

合法目标包括局部/静态变量、导入模块静态变量、数组元素、字符串字符位置、类字段及嵌套
路径。函数调用结果、字面量和切片不是合法目标。

`+=`、`-=`、`*=`、`/=`、`%=`、`^=` 等价于读取目标、运算、写回：

```tl
counter += 1;
numbers[0] *= 2;
point.x -= 3;
```

`_ = expression;` 是独立的丢弃语句：表达式恰好求值一次，但结果不绑定名称。它只支持
普通 `=`；`_ += expression;` 等复合形式和嵌入其他表达式的 `_ = expression` 都非法。

## 7. 表达式和运算符

### 7.1 表达式与无结果语句

字面量、变量、调用、索引、切片、字段访问、匿名函数和表达式式 `if` 都是表达式。
正常完成的同步调用始终产生结果，省略返回类型时产生 `none`；任务调用产生 `Task<T>`，
等待后取得任务结果。声明和循环等语句不能放入表达式位置：

```tl
// 合法：调用完成后 result 的类型和值均为 none
let result = print("hello");

// 合法：保留调用副作用并丢弃无结果值
let _ = print("hello");
_ = print("again");
```

函数参数、数组元素、映射键值和类字段初始化器按源码从左到右编译和求值。

### 7.2 优先级

| 优先级（高到低） | 运算符 | 结合性 |
| --- | --- | --- |
| 1 | 调用 `()`、索引/切片 `[]`、成员 `.` | 左到右 |
| 2 | 单目 `-`、`!` | 右到左组合 |
| 3 | 幂 `^` | 右结合 |
| 4 | `*`、`/`、`%` | 左结合 |
| 5 | `+`、`-` | 左结合 |
| 6 | `<`、`<=`、`>`、`>=` | 左结合 |
| 7 | `==`、`!=` | 左结合 |
| 8 | `&&` | 左结合、短路 |
| 9 | `||` | 左结合、短路 |

`2 ^ 3 ^ 2` 等价于 `2 ^ (3 ^ 2)`。

### 7.3 算术、比较和逻辑

`+` 支持同类型 `int`/`float` 加法、字符串连接和数组连接。`- * / % ^` 支持同类型
数字；单目 `-` 支持数字；`!` 只支持 `bool`。Teal 不做隐式 int/浮点数混合提升，
需要显式 `float()` 或 `int()`。整数除法得到整数，负整数指数会被拒绝。

除零和模零在可静态确定时是编译错误，在运行期是可捕获错误。大小比较要求两边为相同
数字类型。数组、类和枚举使用对象内容相等；类会递归比较字段。映射底层是引用对象，
不要依赖映射内容相等或迭代顺序作为协议。

条件和逻辑运算只接受 `bool`。零、空字符串和空数组不会隐式转换为假。`&&`、`||`
短路，仅在需要时执行右侧。

## 8. 控制流

### 8.1 语句式和表达式式 `if`

```tl
if score >= 90 {
    print("excellent");
} else if score >= 60 {
    print("pass");
} else {
    print("retry");
}
```

语句式 `else` 可省略。条件必须为 `bool`，编译器会合并分支后的类型和初始化状态。

表达式式 `if` 每个分支恰好一个表达式，最终 `else` 必须存在，分支表达式不写分号：

```tl
let label = if value < 0 {
    "negative"
} else if value == 0 {
    "zero"
} else {
    "positive"
};
```

### 8.2 `loop` 与跳转

```tl
loop {
    if finished {
        break;
    }
    step();
}

'outer: for row in rows {
    for cell in row {
        if should_stop(cell) {
            break 'outer;
        }
        if should_skip_row(cell) {
            continue 'outer;
        }
    }
}
```

Teal 没有独立的条件循环语义。需要在每轮开始判断条件时，使用
`loop { if !condition { break; } ... }`。`break;` 退出最近循环，`continue;`
进入最近循环下一轮。

`loop`、集合 `for` 和整数范围 `for` 都可用 Rust 风格的 `'name:` 前缀声明标签；
`break 'name;` 退出指定的外层循环，`continue 'name;` 进入指定循环的下一轮。

`loop` 也可用作表达式。`break value;` 把 value 作为目标循环的结果，裸 `break;` 返回 none。
多个退出路径的结果按联合类型合并；嵌套循环只收集指向自己的 break。标签形式为
`break 'name value;`。`return value;` 则把结果传给函数调用方，并终止该函数的当前路径。

```tl
fn answer() {
    return loop { break 42; };
}
task main() {
    let value = 'outer: loop {
        loop { break 'outer answer(); }
    };
    print(value);
}
```

标签只在当前函数的词法循环作用域内有效，并与变量、函数使用不同命名空间。内层同名
标签会遮蔽外层标签；未知标签、循环外跳转和跨函数跳转都是编译错误。标签不能修饰
普通块、条件或函数，也不支持任意 `goto`。

### 8.3 集合、映射和整数范围 `for`

`for` 可遍历数组、字符串或映射；字符串按 Unicode 标量值产生单字符字符串：

```tl
for value in [1, 2, 3] {
    print(value);
}
for character in "Teal" {
    print(character);
}
for key in scores {
    print(key);
}
for key, value in scores {
    print(key, value);
}
```

映射单绑定形式产生键，双绑定形式依次产生键和值；任一绑定都可写 `_`。映射使用哈希
存储，遍历顺序不保证。每次进入循环时固定当时的键集合，循环体新增的键不会在本轮被
访问；每个值在对应一轮开始前从原映射读取，因此此前对已有键的覆盖可见。数组、字符串
和整数范围不接受双绑定。

整数范围左闭右开，省略下界从零开始：

```tl
for i in ..3 { print(i); }  // 0, 1, 2
for i in 3..5 { print(i); } // 3, 4
```

上下界必须为 `int`。当前没有 `..=`、步长或省略上界的无限范围。所有 `for` 形式都可
使用 discard；映射还可写 `for _, value in ...`、`for key, _ in ...` 或
`for _, _ in ...`。discard 不创建可读取的源级绑定，但循环次数与副作用不变。

### 8.4 用户自定义迭代

`for` 在四种内建可迭代形态之外，还接受**实现了迭代协议的类或枚举**。协议由内建接口
`runtime/packages/std/builtins.tli` 声明，与内建函数一样是由编译器声明表渲染、供编辑器与知识库读取的只读制品，
不是可导入或可执行的 module：

```tl
builtin trait Iterator<Item> {
    fn next() -> Item | none;
}

builtin trait Iterable<Iterator> {
    fn iter() -> Iterator;
}
```

协议块上的类型名是**实现决定的占位**，不属于调用点选择的类型实参：某类型实现 `Iterator`
时，`Item` 取自该实现 `next` 的返回类型去掉 `none` 的部分；实现 `Iterable` 时，
`Iterator` 取自该实现 `iter` 的返回类型。

用户用普通 `impl` 显式声明可迭代；类可以原地推进自身字段作为游标：

```tl
class Countdown {
    remaining: int,
}

impl Iterable for Countdown {
    fn iter() -> Countdown {
        return self;
    }
}

impl Iterator for Countdown {
    fn next() -> int | none {
        if self.remaining <= 0 {
            return none;
        }
        self.remaining -= 1;
        return self.remaining + 1;
    }
}


task main() {
    let countdown = Countdown { remaining: 3 };
    for value in countdown {
        print(value);
    }
}
```

解析顺序固定为内建可迭代形态、`Iterable` 实现、`Iterator` 实现；三者都不满足时报错，
两者都实现时 `Iterable` 优先。`for` 对可迭代表达式只求值一次，`iter()` 每次执行循环调用
一次，`next()` 每轮调用一次，包含最后一次返回 `none` 的终止调用。绑定形态仍是单绑定或
discard，双绑定继续只对映射成立。

元素类型由实现的 `next` 返回类型决定，循环体按该类型使用绑定；`iter()` 与 `next()` 抛出
的错误按普通调用传播，可被外层 `try/catch` 捕获；循环体可以 `await`；循环标签、`break`
与 `continue` 的行为与内建可迭代形态完全一致。迭代次数完全由 `next()` 决定，可以为空、
有限或无限，编译器不做静态检查，也不要求对象可重复遍历。

协议成员可以是 `task`：此时 `iter()` 与每轮 `next()` 各自创建并立即等待一个任务，
异步迭代的推进顺序与循环体交错可见；循环体本身仍可继续 `await`。

实现了 `Iterator` 的类型同时获得 8.5 节的迭代器适配器与终结方法（`map`、`filter`、
`take`、`skip`、`take_while`、`skip_while`、`chain`、`collect`、`count`、`any`、`all`、
`find`、`fold`），`next` 是 `fn` 还是 `task` 都一样，不需要实现额外成员。

其他约束：

- 协议成员必须是零参数实例方法；`static fn` 和带参数的同名方法不构成有效实现；
- `iter()` 的返回类型必须是实现 `Iterator` 协议的类或枚举；`next()` 必须返回 `none` 与
  一个具体元素类型组成的联合，不能只返回 `none`；
- 协议实现只能声明在目标类型所在的模块，和固有 `impl` 使用同一可见性边界；
- 枚举负载不是赋值目标（见 6.3 节），枚举实现的迭代器不能原地推进游标，需要可变游标时
  用类承载状态；
- `Iterable` 与 `Iterator` 是内建接口保留名，用户不能声明同名 `trait`；协议名不是类型，
  不能出现在类型标注或 `impl Trait` bound 中。

### 8.5 迭代器值与惰性适配器

内建集合提供 `iter()`，返回一个惰性迭代器值：

```tl
let values = [1, 2, 3, 4];
let doubled = values.iter().map(fn(value: int) { return value * 2; }).collect();
print(doubled);
```

- `array.iter()` 按数组顺序交付元素，`string.iter()` 按 Unicode 标量交付单个字符的
  字符串（与 `for c in text` 一致），`map.iter()` 按键快照交付键（与 `for key in map` 一致）；
- 适配器 `map`、`filter`、`take`、`skip`、`take_while`、`skip_while` 与 `chain` 都是惰性的：
  它们只在真正被消费时调用回调或推进源，并且 `take(count)` 一旦达到数量就停止读取源；
- 终结方法消费剩余元素并给出结果：`collect() -> T[]`、`count() -> int`、
  `any(predicate) -> bool`、`all(predicate) -> bool`、`find(predicate) -> T | none`、
  `fold(initial, callback) -> U`；
- `for` 可以直接驱动迭代器值，无需再调用 `iter()`，元素类型就是迭代器的元素类型。

适配器与终结方法的语义：

- `map(callback)` 对每个元素返回 `callback(element)`；`filter(predicate)` 只交付谓词成立
  的元素；`skip(count)` 先丢弃至多 `count` 个元素，`take(count)` 最多交付 `count` 个元素；
- `take_while(predicate)` 在谓词首次不成立时结束，`skip_while(predicate)` 丢弃谓词成立的
  前缀并交付其余元素（包括首个不成立的那个元素）；
- `chain(other)` 先交付接收者的全部元素，再交付 `other` 的元素，两段必须产出相同元素类型，
  实参不是迭代器值时报错；
- `count()` 返回剩余元素数量，`fold(initial, callback)` 以 `initial` 为初值按顺序折叠，
  `any`/`all`/`find` 在得出结论后立即停止读取源；`find` 没有命中时返回 `none`；
- `count <= 0` 时 `take` 的结果为空、`skip` 不丢弃任何元素；`count` 超过元素数量时按实际
  元素数量处理。

同一套适配器与终结方法对**任意 `Iterator` 实现**都可用，不需要实现额外的成员：内建集合的
`iter()`、用户协议的 `impl Iterator for T`（8.4 节）都进入同一份目录与合成阶段。协议的
`next` 声明为 `task` 时，适配器与终结方法同样逐步等待来源，因此异步迭代器与内建来源的
推进顺序一致：

```tl
class Steps { remaining: int }

impl Iterator for Steps {
    task next() -> int | none {
        if self.remaining <= 0 { return none; }
        self.remaining -= 1;
        return self.remaining + 1;
    }
}

task main() {
    let total = Steps { remaining: 4 }.skip(1).take(2).fold(0, fn(sum: int, value: int) {
        return sum + value;
    });
    print(total);
}
```

异步来源的每一步都必须等待，因此消费异步迭代器只能发生在 `task` 里；在同步 `fn` 中消费
会报“异步迭代步骤需要任务上下文”。

迭代器的静态类型写作 `Iterator<T>`，`typeof` 输出同一名称（例如 `typeof([1, 2].iter())`
是 `Iterator<int>`）。`Iterator<T>` 只由内建方法产生，用户不能声明该类型、不能把它写进
类型标注、`impl Trait` bound 或类字段，也不能构造它；它同样不能跨越 Host、模型、dylib 与
FFI 边界：边界类型转换显式拒绝该类型并报“迭代器值不能跨越外部边界”，而把它当作 Host 调用
或 `self.think` 的实参也会因类型不匹配被拒绝。跨边界前先在脚本内消费迭代器。

迭代器是**单次消费值**：同一个迭代器值被重复遍历时，第二次只会得到剩余（通常已为空）
的元素。需要重新遍历时重新调用 `iter()`，或让可迭代类型实现 `Iterable` 协议（见 8.4 节）。
元素类型无法确定时（例如尚未确定元素类型的 `[]` 或 `{}`）`iter()` 与 `map()` 会报错，
先写好集合元素类型即可。

迭代器接口不提供 `zip`、`enumerate`，也没有生成器；已有的
`array.map`/`array.filter`/`string.map`/`string.filter` 与集合上的 `fold`/`any`/`all`
仍然返回急切的集合结果，与迭代器适配器是两套入口。

裸 `{ ... }` 代码块仅建立新作用域。

## 9. 函数和闭包

### 9.1 声明、返回和调用

命名 `fn`、`task` 只允许作为模块声明或 impl 成员，任何可执行块中均禁止命名声明，包括函数体、方法体、闭包体、条件块和循环块；附加属性也不改变该限制。命名函数自身不捕获外层局部变量，实例方法的 `self` 是接收者参数。trait 方法签名和外部调用声明的现有规则不变。

命名函数使用可选的 `-> Type` 声明返回契约。Teal 没有 `void` 返回类型；省略箭头时
从函数体推导返回类型；没有具体返回结果时为 `none`。显式声明返回类型后，每条可达路径都必须返回兼容值：

```tl
fn add(left: int, right: int) -> int {
    return left + right;
}

task announce(message: string) {
    print(message);
    return;
}
```

参数类型可省略。参数名可写为 `_` 表示函数体不绑定该参数；多个 `_` 合法，但每个仍
保留实参位置、类型检查和调用帧参数槽。没有具体返回结果的函数允许 `return;` 或自然走到
末尾；显式返回类型接受 `none` 时同样允许这些形式。其他返回类型禁止裸 `return;` 和可达落空路径。
无具体结果函数的结果可以丢弃、存入命名变量或传给接受 `none` 的参数。
匿名闭包由函数体推演结果；没有具体结果时返回 `none`。实参数量必须匹配，
显式参数类型必须接受实参。

文件专用入口 `main`（可以是 `fn main` 或 `task main`）使用 4.2 节的整数返回规则，且不能作为普通函数调用或引用。

递归受支持：

```tl
fn factorial(n: int) -> int {
    if n <= 1 { return 1; }
    return n * factorial(n - 1);
}
```

### 9.2 匿名函数、函数类型和局部算法迁移

```tl
let double = fn(value: int) {
    return value * 2;
};
let transform: fn(int) -> int;
```

函数类型写为 `fn(A, B) -> R`；省略 `-> R` 等价于显式 `-> none`。`task(A, B) -> R` 使用同一可调用类型体系，但调用结果是 `Task<R>`，需要
`.await` 才得到 `R`，不能赋给返回同步 `R` 的 `fn(A, B) -> R`。函数类型声明不初始化变量，
在赋值之前只能由 `typeof` 查询，不能调用。初始化与后续赋值均检查参数和返回契约。

匿名 `fn(...) { ... }`、`task(...) { ... }` 是表达式，可赋值、传参、返回或存入容器。
块内不再接受 `fn name(...) { ... }` 或 `task name(...) { ... }`。需要保留显式返回契约时，
给闭包变量标注可调用类型：

```tl
let offset = 1;
let add_offset: fn(int) -> int = fn(value: int) { return value + offset; };
let compute: task(int) -> int = task(value: int) { return value * 2; };
```

非递归嵌套函数按此方式改为闭包绑定。递归嵌套函数应提升为模块函数或关联函数，原捕获数据
改为显式参数；命名函数递归仍受支持。闭包初始化器不能隐式引用正在定义的绑定，不提供新的
自递归语法。命名函数与闭包继续使用现有可调用类型；闭包不能新增实现自定义 trait 的能力。

### 9.3 闭包捕获

每次执行匿名函数或匿名任务表达式都会创建独立闭包。闭包可存变量、传参、返回和
放入容器。捕获的是共享词法绑定，不是创建时的副本：

```tl
fn make_counter(start: int) -> fn(int) -> int {
    let value = start;
    return fn(step: int) {
        value = value + step;
        return value;
    };
}
```

外层代码、闭包和兄弟闭包对同一捕获的写入互相可见；闭包逃逸后捕获仍存活。跨多层嵌套
时，中间闭包转发深层需要的捕获。命名函数虽然没有捕获环境，其参数和局部变量仍可被内部闭包捕获；方法内的闭包也可捕获接收者 `self`。

创建闭包时，被捕获局部绑定必须已确定初始化。先声明、赋值、再创建闭包合法；先创建再
赋值会被拒绝。后续同名 `let` 创建新绑定，不改变既有闭包捕获。

### 9.4 工作流声明与分段执行

`Task<T>` 是编译器提供的内建任务句柄类型。尖括号中的 `T` 只表示该句柄完成时的结果类型，
不是用户可声明的任务调度器参数。用户函数可以声明显式泛型参数并在调用点单态化；泛型类型、
泛型类型和 trait 关联类型都在调用和 impl 约束处静态解析；`Iterator` 仍由编译器协议提供，不能作为用户 trait bound。裸 `Task` 是
`Task<none>` 的简写，用于没有具体结果的任务。任务句柄可以出现在参数、返回值、字段和容器元素的
类型位置，但不能据此声明新的类型参数或泛型类型。

`task` 使用与 `fn` 相同的声明位置、参数、返回契约、闭包捕获、递归、方法、trait 和调用
形式。命名 task 只位于模块或 class、enum、agent 的 impl 中；块内使用匿名 task 表达式。
只做确定性计算的 task 由 `core` 配置执行；调用 Host 或 Agent 时才推导对应 Extension：

```tl
task twice(value: int) -> int { return value * 2; }
task main() { workflow().await; }
task workflow() { print(twice(21).await); }
```

运行方式为 `teal run examples/core/tasks.tl`。
`.await` 只允许出现在 task 内。需要设置进程退出码时，在 task 中使用 `exit(result.await)`。
task 调用会立即提交并返回 `Task<T>`；只有后缀 `.await` 才取得结果 `T`。对无结果任务取得结果时，
`.await` 的值为 `none`，通常直接丢弃。task 本身不表示模型调用，
脚本中的确定性计算仍由 CPU 执行。明确的 `self.think(prompt)`、`self.think_candidate(prompt)` 或 `self.chat(prompt)` 才会进入模型流程。
普通 `fn` 可以调用 `task` 或 Agent 的公开 `task` 方法，并保存、传递或返回所得的 `Task<T>`，
但不能使用 `.await`。这些限制按当前函数自身的声明类型检查，不随调用链传播：由 `task` 调用的
`fn` 仍不是 `task`。`self.think` 与 `self.chat` 只能直接出现在 Agent 的 `task` 方法中；普通 `fn`、普通 CPU
`task` 和 Agent `fn` 均不能调用。
分段推进、快照和外部请求的实现契约见 [teal-vm 实现细节](../crates/teal-vm/README.md#vm执行任务值与快照)。

`Task::group()` 创建绑定当前父 task 的 TaskGroup。`group.add(task)` 按加入顺序登记子任务，
`group.join().await` 返回 `Result<none, Error>`，等待全部成员并在首个失败处取消其余未完成成员；未 join 的 group 会以
`task_group_unjoined` 失败父 task，`group.detach()` 明确放弃结果并抑制成员未观察失败，
`group.cancel()` 请求取消全部未完成成员。Group 句柄不能跨 Host、dylib、模型 JSON 或 Agent 边界，
其成员和生命周期状态随 VM Snapshot 保存。

### 9.5 Agent、Graph 任务和模型调用

`agent` 与 `class` 共用字段列表、无字段声明、构造表达式、`impl`、trait 和普通方法语法。
差异只来自 Agent 运行时身份：Agent 用字段保存私有状态，公开 `pub task` 方法可从外部实例调用。
调用会返回 `Task<T>`；实例外不能读取私有字段，只能通过不可伪造的 Agent 句柄调用公开方法：

```tl
agent Reviewer { reviews: int }

impl Reviewer {
    pub task review(text: string) -> string {
        self.reviews += 1;
        return text;
    }
}

task main() { workflow().await; }
task workflow() {
    let reviewer = Reviewer { reviews: 0 };
    let answer: string = reviewer.review("content").await;
    print(answer);
}
```

- Agent 方法调用立即返回 `Task<T>`；同一实例的任务按提交顺序串行，不同实例可以并行。
- `pending.await` 等待任务结果；`Task::all([pending...]).await` 按输入顺序返回结果数组。
  `Task<none>[]` 同样得到 `none[]`，每个输入对应一个元素，有类型的空输入得到空数组。
- `pending.cancel()` 取消任务及其后代；Agent 句柄提供 `interrupt()` 和 `status()` 控制方法。

递归取消的错误通过目标任务报告；捕获目标的 `task_cancelled` 后，不会再因自动取消的后代重复失败。
目标或后代在取消前已经发生的独立失败仍需处理，取消不抹除失败结果。

任务句柄可以使用 `Task<T>` 作为脚本参数、返回值、字段和容器元素，但不能进入 Host、dylib 或模型
JSON 参数。失败在调用或 `.await` 处传播，可用普通 `try/catch` 处理；同级分支不会自动取消。

包含模型调用的 Agent 必须声明 `@model` 或 `@model (optional)`。`@model (optional)` 允许在调用选项中选择模型预设；`@model()` 使用有效 `model.toml` 的 `[default]` 预设，
`@model("preset")` 使用 `[profiles.preset]` 命名预设；已有的 `@model(provider, model[, auth_profile])`
继续固定源码中的模型身份。结果类型来自赋值、
返回或参数位置。每个实例持续保存自己的私有模型历史，新实例不继承父历史。模型只能使用
运行时登记的结果工具，以及能力范围内的 `spawn_agent`、`send_message`、`followup_task`、`wait_agent`、`ack_messages`、
`interrupt_agent` 和 `list_agents`。动态创建还须属于宿主工厂白名单，并校验已编译类型的初始字段；模型不能加载新源码。
`wait_agent` 非破坏读取消息并返回 cursor；`ack_messages` 显式确认，未确认消息会随 Graph 快照恢复。

`self.think(prompt)`、`self.think_candidate(prompt)` 与 `self.chat(prompt)` 使用 Agent 的 `@options` 默认调用选项；导入 `std/agent.tl` 后可以传入第二个 `RunOptions` 参数覆盖本次调用。`think_candidate` 返回 `AgentCandidate<T>`，候选保存模型完成和来源摘要；调用 `candidate.value()` 只取得形状已校验的值，业务规则仍需由应用显式验证。
标准模块允许别名；模型调用只能使用当前 Agent 的 receiver 方法，`Agent::receive` 等宿主模块函数仍按普通模块解析。

```tl
import "std/agent.tl" as ai;

@model("codex", "gpt-5.6-terra")
agent Reviewer {}

impl Reviewer {
    pub task review(text: string) -> string {
        let options = self.options();
        options.context = ["Focus on correctness and boundary conditions."];
        options.model_rounds = 4;
        options.max_output_tokens = 1024;
        options.propose_memory = false;
        return self.think(text, options).await;
    }
}
```

`RunOptions` 的 `effort` 默认为空字符串，表示继承 `@options` 或供应商默认值；`model_rounds`、`tool_calls`、`timeout_ms` 和 `max_output_tokens` 默认为零，表示继承宿主限制；
正数只能缩减限制，负数或超过宿主限制的值会失败。`context` 最多 64 个非空白字符串，UTF-8 总长度最多 64 KiB，
作为普通用户消息中的补充数据，不替换固定指令或授权。`recall_memory`、`propose_memory` 默认均为 true，
分别控制召回与未验证候选写入；宿主没有安装记忆服务时不产生记忆操作。
`model binding` 仅供 `@model (optional)` Agent 使用，采用 `profile/model`；`instructions` 是本次命名会话的固定补充指令；`tools` 是源码请求的可见工具名单。首次模型调用固定命名会话的模型、认证 profile 与指令；同一会话改绑会失败，切换模型须使用新会话。工具名单只收窄可见性，不扩大宿主授权。

每次 `self.think` 先校验这些选项，再进入该次调用的认证、模型工厂和操作账本。模型 Agent 构造时的认证预检独立保留，
并不提前执行未来 task 来计算选项。持久调用可以使用模型自主提问服务。

`self.session_options(self.session("review"))` 选择当前 Agent 的局部会话，返回普通 `RunOptions`。
`session_key` 默认为空，沿用默认会话；非空键最多 128 个 UTF-8 字节，不允许控制字符或首尾空白。
相同 Agent、相同键的模型调用串行累积确定历史；不同键或不同 Agent 不共享历史。每个 Agent 最多 64 个命名会话，持久重开不重置上限。
RuntimeServices 的 RootBudget 在命名会话和源码子 Agent 间共享；新建会话不能重置模型响应、工具或词元总量。持久 run 将用量写入账本，恢复时不能更换 limits。
`self.context(items)` 与 `self.set_context(options, context)` 只组装补充数据，不改变权限或 VM/账本。

`std/memory.tl` 的 `memory::recall(Scope.workspace, query, limit).await` 返回带来源的 `Entry[]`，仅召回该范围内活动或冲突记录。
`memory::propose(scope, content).await` 返回未验证候选身份，不接受 verified 标志；来源由运行时生成。
宿主必须显式安装 MemoryStore，默认只允许工作区候选写入；用户级写入还需宿主开放。
`propose_memory` 仅控制 `self.think` 自动完成提取，不撤销显式标准调用。持久记忆调用遵循 Journaled Host 恢复，不自动重放未知效果。

模型结果只接受具体 JSON 数据：标量、固定元素数组、字符串键映射、普通类、枚举与联合类型。`AgentCandidate<T>` 只包装这些具体结果，不允许句柄、函数或未确定的模型数据类型。
`self.think` 的结果必须经 `__teal_complete` 独占提交，普通文本或 JSON 文本仅留在模型历史中。`self.chat` 固定返回 `string` 并按 Text 完成，不能代替结构化交付。
`RunOptions` 不能降低这一完成要求；未通过完成验收前不触发完成 Hook 或记忆完成写入。
联合类型保留所有分支，JSON Schema 使用 `anyOf`，整数与浮点数等分支可以重叠。函数、Agent/Task 句柄、
未确定类型、非字符串映射键和递归名义类型在编译或程序加载时拒绝。展开的类型最多 32 层（包含根）、4096 个节点，
枚举变体计入结构开销；这些限制只作用于模型边界，不禁止普通 CPU 数据使用循环对象。
运行示例见 [模型选项与枚举结果](../examples/agent/model_options.tl)；
`task main` 直接等待 Agent 模型任务的完整入口见 [task main 模型示例](../examples/agent/task_main_model.tl)，
执行模型部分需要配置认证。
命名会话、显式 Context 与记忆候选见 [会话与记忆示例](../examples/agent/sessions_and_memory.tl)；
受控动态团队、消息 cursor 与确认协议见 [模型团队示例](../examples/agent/model_team.tl)。两者的注释列出认证和宿主服务前提。

`@options({...})` 可以为 Agent 声明 effort、model_rounds、tool_calls、timeout_ms 和 max_output_tokens 的静态默认值；map 的 key 必须是受支持的英文名称，单次 `RunOptions` 值优先于这些默认值，宿主限制仍然优先。

`@model()` 使用当前默认模型预设，`@model("preset")` 使用命名模型预设。预设在启动预检前解析
为具体 provider、model 和认证资料，并随持久运行固定；缺失或无效的预设会使运行在副作用前失败。
`@model("provider", "model")` 使用该供应商的活动认证资料，
`@model("provider", "model", "auth-name")` 固定命名认证资料。内建 `@auth` 已移除；
未定义的 `@Auth { ... }` 按普通未知属性拒绝；显式声明 `@attribute class Auth { ... }`
时它只是无认证语义的自定义属性类型。源码不能读取凭证或自行批准登录。

`@tool("name", "description")` 可标注一个顶层 `task`，编译器从恰好一个可序列化参数和可序列化返回值生成模型工具 Schema；重名、保留名和句柄类型被拒绝。每次调用在独立 VM 上下文中执行，并继承父 run 的授权、预算、审计和恢复边界；默认按可能有副作用的工具串行。`@role("description")` 可声明已编译 Agent 定义可供源码模型动态创建，实际调用仍受宿主目录约束。

`std/agent.tl` 提供 `receive().await`、`emit(kind, payload)`、可选的 `render(ui::Node)`、非秘密模型目录、工作区基线、当前程序摘要和真实工具收据。`receive()` 的输入是文本、命令、UI 事件或跳过；持久 run 的请求绑定 ID 和代次，缺少即时输入时保存检查点并等待回答。`std/approval.tl` 提供操作与资源绑定的业务批准收据；批准不替代 Host 工具权限。内建应用见 [`runtime/agents/default.tl`](../runtime/agents/default.tl)，交付核验规则见 [`agents/delivery.tl`](../runtime/packages/agents/src/delivery.tl)。普通批处理 Agent 不需要导入交互模块。

导入 `std/agent.tl` 为 `app` 后，`app::workspace_snapshot(path).await` 返回规范路径、UTF-8 文本、SHA-256 和可选打开缓冲区版本；`app::workspace_replace(snapshot, text).await` 只能在原文件及缓冲区版本仍匹配时提交修改。读取和写入分别经过宿主工具 grant、资源权限和版本复核，修改通过共享工作区事务与持久操作账本，未知效果不会因 VM 恢复自动重放。缺少工作区服务时调用明确失败。
当前内建 Codex 适配器由 Teal
直接实现浏览器 OAuth、设备码、API Key、个人访问令牌、Agent Identity 和刷新，也可显式
共享官方 Codex CLI。认证资料是 provider 下的认证快捷键，配置只保存认证方式、环境变量引用
和非秘密账户摘要；秘密来自进程环境、工作区 `.teal/.env`、用户 `TEAL_HOME/.env` 或当前
官方 Codex 凭据源。停止共享不会退出官方 CLI；账户变化时，有历史的会话会在首次发送前确认。
其他供应商通过同一 Rust 认证接口接入，不要求统一采用 OAuth。

认证 profile 可在运行前管理：

```text
teal auth login codex --profile work [--method chatgpt-browser] [--scope user|workspace]
teal auth login codex --profile work --method api-key --stdin-key
teal auth login codex --profile work --method codex-cli --yes
teal-auth status [codex] [--profile work] [--format text|json]
teal auth use codex work --model-profile coding [--scope user|workspace]
teal auth logout codex [--profile work] [--scope user|workspace]
teal auth login deepseek --profile work [--stdin-key | --env-key DEEPSEEK_API_KEY]
teal auth status deepseek [--profile work] [--verify]
```

`--stdin-key` 只适用于 `api-key`、`personal-access-token` 和 `agent-identity`，并只移除
输入末尾的换行。个人访问令牌必须使用 `at-` 形式，Agent Identity 必须使用管理端签发的
JWT。命令行参数、状态 JSON、运行数据库和错误文本都不包含秘密。

`--env-key` 接收环境变量名，仅用于 API Key，并与 `--stdin-key` 互斥；配置字段同名为
`env_key`。DeepSeek 的唯一认证方式自动选择 API Key，登录时校验官方 `/models`。
`status` 默认只检查本地状态，`--verify` 显式请求远程验证。配置版本为 1，旧
`credential_env` 和旧 CLI 选项 `--secret-stdin` 已移除；迁移时保持已有秘密变量名。

`teal run`、源码 Agent Runtime、持久恢复和工作台共用同一认证预检，只刷新并检查
所需 profile，不启动登录或切换账户。工作台本身不要求认证；Chat、Task、行内补全和扩展
模型任务在创建模型、会话或持久 run 前预检。缺少有效认证时提示执行 `teal auth login`。

## 10. 数组、映射和字符串

### 10.1 数组

数组是有序、可变、同质集合：

```tl
let values = [10, 20, 30,];
let matrix: int[][] = [[1, 2], [3, 4]];
let empty = [];
```

非空数组的元素必须可赋给同一元素类型。空数组最初是未知元素类型；对变量调用 `push`
或进行受检查赋值可固定元素类型。

数组值是引用式对象：

```tl
let first = [1, 2];
let alias = first;
alias.push(3);
print(first); // 也能看到 3
```

数组 `+` 和切片产生新的外层数组；其中的嵌套对象仍可能共享引用。

### 10.2 索引、修改和切片

索引从零开始，类型必须为 `int`：

```tl
print(values[0]);
print(values[-1]); // 最后一个元素
values[1] = 99;
values[-2] += 1;
```

数组元素读取和赋值支持从尾部计数：`-1` 是最后一个元素，`-len` 是第一个元素。
非负索引大于等于长度，或负索引小于 `-len`，都会产生 `index_out_of_bounds`。
该规则仅适用于单个数组元素；字符串索引以及数组/字符串切片仍不接受负边界，
`remove(index)` 等方法参数也保持非负索引规则。

切片左闭右开：

```tl
let middle = values[1..3];
let prefix = values[..2];
let suffix = values[1..];
let copy = values[..];
```

左右边界都可独立省略：左边界默认为 `0`，右边界默认为数组元素数或字符串 Unicode
标量数。切片使用标准左闭右开边界 `0 <= start <= end <= len`，因此
`values[len..]`、`values[len..len]` 和空集合的 `values[..]` 都产生空结果。显式边界
必须为 `int`；负边界、反向范围或超过长度会产生 `slice_out_of_bounds`。

### 10.3 映射

映射字面量可以为空，也可以包含一个或多个键值对：

```tl
let scores: {string: int} = {
    "alice": 10,
    "bob": 20,
};

let typed_empty: {string: int} = {};
let inferred_empty = {};
inferred_empty.insert("carol", 30);
```

非空映射的键和值分别推演为一致类型。空映射起初不知道键和值类型；显式类型标注会立即
固定二者，否则第一次成功的 `insert`、索引赋值或具体映射整体赋值会填充尚未确定的类型，
后续写入必须与之相容。映射使用哈希存储，不保证显示或迭代顺序。映射字面量的键和值按
源码顺序求值；键可以是运行时表达式，重复键按后写入覆盖。为跨版本稳定，推荐 `string`、
`int` 或 `bool` 键。

映射支持键索引，也保留等价的方法形式：

```tl
scores["carol"] = 30;
scores["carol"] += 5;
let value = scores["carol"];

scores.insert("dave", 40);
let other = scores.get("dave");
```

`map[key]` 与 `get(key)` 的读取语义相同：键不存在时抛出 `unknown_map_key`，不会返回
`none`。`map[key] = value` 与 `insert(key, value)` 都原地增加或覆盖；复合赋值会先读取
旧值，所以目标键必须已存在。索引键和值分别检查映射声明或推断出的类型。映射仍不能用
范围语法切片。映射可按本手册 8.3 节的单绑定键形式或双绑定键值形式直接遍历。

若空映射的值类型尚未由标注或写入确定，读取表达式会报告无法推导类型。未知类型不会
被转换成合法的 `none`。这是编译期类型规则，
不会把运行时缺失键改成 `none`；值类型已确定后，缺失键仍抛出 `unknown_map_key`。

### 10.4 字符串的字符与字节

字符串是 UTF-8。当前操作的计量单位不完全相同：

- `text[index]`、`text[start..end]`、`text[start..]` 和 `for character in text` 按 Unicode 标量值；
- `text.reverse()` 按 Unicode 标量值反转；
- `text.len()` 返回 UTF-8 字节数；
- `text.find(needle)` 返回 UTF-8 字节偏移，未找到为 `-1`。

```tl
let text = "Teal";
print(text[1]);       // e
print(text[1..3]);    // ea
```

字符串索引赋值要求右侧为字符串，并产生更新后的字符串值再写回目标。字符串值本身不像
数组那样公开可变缓冲区。

这一语义不依赖存储形式。运行时最多把 6 个 UTF-8 字节内联进 8 字节 `Value`，并以明确
little-endian 位顺序解码；超过 6 字节或以 NUL 结尾的字符串进入字符串池。因此 `"\0"`
和 `"a\0"` 能无损保留，内部含 NUL 但末字节非零的短字符串仍可内联。

## 11. 类、struct 和 newtype

`type Text = string;` 声明无运行时身份的类型别名，别名只在静态类型解析中展开。`struct` 复用 class 的字段和构造语法，但构造后字段不可重新赋值。struct 字段必须组成递归值安全布局：
不能携带 class、Agent、Task、函数或迭代器，也不能形成按值环；数组、映射、struct、newtype 和 enum
字段按同一规则递归检查，构造后按值图冻结。`newtype Name(Type)` 是单字段名义包装，构造必须显式调用
`Name(value)`；它不会自动转换为底层类型。

### 11.1 声明和构造

`class` 声明名义记录类型，字段列表可以为空：

```tl
class Point {
    x: int,
    y: int,
}


fn main() {
    let point = Point { x: 3, y: 4 };
}
```

`struct` 可以声明显式泛型参数；每次构造从字段值推导实参，显式类型位置可以写出完整应用：

```tl
struct Box<T> { value: T }
fn unwrap(value: Box<int>) -> int { return value.value; }
```

不同实际参数是不同的静态类型，字段读取会使用已绑定的具体类型。

构造时必须提供全部字段，不能重复或提供未知字段，字段值必须符合声明类型。字段名称负责
匹配布局；当前编译器按类的字段声明顺序编译并求值初始化表达式，即使构造器中字段的书写
顺序不同也是如此。类也可在函数块中声明并遵循块作用域，公共类型通常应放顶层。

普通 class 字段默认只在声明文件中可见；`pub` 放在字段名称前，使导入者可以构造、读取和
修改该字段。公开 class 不会自动公开其字段；只要存在私有字段，其他模块就不能使用完整字段
字面量构造该类型，应调用声明模块提供的公开关联函数或普通函数。字段可见性逐层检查，公开的
外层字段不会连带公开其值类型的内部字段。Agent 字段不能声明 `pub`，其状态继续通过
`pub task` 方法访问。

### 11.2 访问、修改、别名和相等

```tl
point.x = 10;
point.y += 2;
segment.end.y = 12;
matrix.cells[0][1] = 7;
```

可连续访问嵌套字段和数组。类值是引用式对象；`let b = a;` 后修改 `b.field` 也能从
`a.field` 观察到。struct 值是不可变值；字段及其嵌套集合不能通过别名修改，更新必须构造新值。
相等比较则按类类型和字段内容递归判断，不只是引用身份。

### 11.3 固有方法和关联函数

`impl` 只在编译期把类/枚举与普通函数或 task 登记起来。实例方法省略接收者，方法体自动
提供 `self`；`static fn`/`static task` 没有接收者。`Self` 表示当前名义类型：

```tl
impl Point {
    pub fn translate(dx: int) {
        self.x += dx;
    }

    pub static fn origin() -> Self {
        return Self { x: 0, y: 0 };
    }
}

let point = Point::origin();
point.translate(3);
```

Class 方法可修改引用对象字段；枚举方法接收普通枚举值，可 match 或返回新变体，
但不会隐式原地切换变体。固有 impl 只能作用于当前模块的顶层类/枚举。

### 11.4 Trait、UFCS 和 `impl Trait`

Trait 是 `fn`/`task` 实例方法和关联函数的编译期签名集合，可由类或枚举实现。实现成员
必须保持契约中的 `fn`/`task` 身份。Trait/impl 本身
不进入 Program，也不能使用普通属性：

```tl
trait Named { fn name() -> string; }
impl Named for State { fn name() -> string { return "ready"; } }

fn describe(value: impl Named) -> string { return value.name(); }
fn initial() -> impl Named { return State.ready; }
```

多个 bounds 写作 `impl A + B`。Opaque 参数只能使用 bounds 声明的实例方法，不能访问
隐藏类型的字段或固有方法。把不透明值放入推演出的数组或映射后，索引结果仍保留同一
特征表面；它也不能隐式转换成隐藏的具体类/枚举参数或字段。重名特征方法使用
`<_ as Trait>::method(value)`；具体类型
使用 `<State as Trait>::method(state)`。关联函数使用 `Type::function()` 或
`<Type as Trait>::function()`。编译器按具体类/枚举单态化，运行时没有特征对象、
vtable 或不透明 wrapper。

内建接口另外声明只约束形状的迭代协议 `Iterable` 和 `Iterator`（见 8.4 节）。
它们不是用户可声明的 trait，也不能用作类型或 bound；类型通过普通 `impl` 声明实现，
元素类型与迭代器类型由该实现的返回类型决定。
内建集合的 `iter()` 与迭代器值上的 `map`/`filter`/`take`/`skip`/`take_while`/`skip_while`/
`chain`/`collect`/`count`/`any`/`all`/`find`/`fold`（见 8.5 节）由编译器合成为
隐藏类，用户既不能声明 `Iterator<T>` 类型，也不能把它用作 bound。

## 12. 枚举和匹配

### 12.1 声明与构造

枚举至少一个变体；变体可无数据或带具名字段：

```tl
enum Action {
    archive = -1,
    reply {
        to: string,
        tone: string,
    },
}


fn main() {
    let first = Action.archive;
    let second = Action.reply {
        to: "team",
        tone: "warm",
    };
}
```

无字段变体可写显式判别式 `name = signed-int-literal`。判别式只接受可选负号加十进制
`i32` 字面量，不接受变量、运算、调用、索引或浮点表达式。同一枚举内判别式必须唯一。
首个省略值为 `0`；后续省略值为前一变体加一，带字段变体也参与连续编号，但不能显式
指定值。递增超过 `i32` 范围会在编译期报错。例如：

```tl
enum Status {
    unknown = -1,
    ready,                 // 0
    detail { code: int },  // 1
    done = 10,
    archived,              // 11
}
```

变体使用限定点语法。带字段变体必须且只能提供一次每个声明字段。枚举是名义类型，
`typeof` 返回枚举名；两个不同枚举即使使用相同判别式也不是同一值。`int(enum_value)`
返回判别式，`string(enum_value)`、打印和字符串插值继续使用声明的枚举名、变体名与字段。

运行时 `Value` 始终为 8 字节：无字段变体把 `EnumId` 和判别式直接内联；所有带字段
变体无论字段是整数、布尔、浮点、字符串、集合、类还是嵌套枚举，都使用对象池保存
“内联枚举头 + 字段值”。这一存储差异不改变 Teal 源码类型。

### 12.2 `match`

`match` 按源码顺序测试模式，但会在当前函数的实际参数专门化类型上执行穷尽检查：

```tl
match direction {
    Direction.north => { print("north"); }
    Direction.south => { print("south"); }
    Direction.east => { print("east"); }
    Direction.west => { print("west"); }
}
```

规则：

- 被匹配表达式只求值一次；
- 至少有一个臂；允许只写最终 `_`；
- `_` 出现时必须是最后一臂；
- 各臂是普通语句块；
- 具体枚举必须静态覆盖每个声明变体，或提供最终 `_`；
- `bool` 必须静态覆盖 `true` 和 `false`，或提供最终 `_`；
- `int`、`float`、`string`、数组、映射、类、联合类型、未知类型和动态表达式模式必须提供最终 `_`；
- 重复的静态枚举变体和布尔字面量报错；静态覆盖完成后的非通配臂不可达；
- 完整静态覆盖后仍可保留最终 `_`，作为未来兼容回退；
- 普通值表达式仍按 `==` 匹配，但只有静态 `Type.variant` 和布尔字面量计入有限覆盖。

穷尽检查按被匹配表达式的精确类型判定：联合类型即使包含 `none` 或 `bool`，也仍然属于
“其他 scrutinee 类型”，必须提供最终 `_`。因此 `T | none` 写成 `none` 加 `_`（或只用 `_`），
`bool | none` 不能只靠 `true`/`false` 覆盖；`none` 只是普通表达式模式，不会把联合类型
缩小成“只有 none”。

`match` 的模式都是表达式或枚举解构，**没有**“把非 `none` 部分绑定到新变量”的写法：
`match value { none => …, other => … }` 里的 `other` 是普通表达式（要求它是作用域内的值，
并按 `==` 比较），不会绑定 `value` 的 `T`。`T | none` 的 `match` 里，`none` 字面量臂
之后的臂内该变量按具体成员使用（与 5.6 的 `if` 收窄是同一机制），因此
`match value { none => …, _ => value + 1 }` 直接成立；模式本身仍是表达式比较，不绑定变量。

这里的 `_` 是模式通配符，不是丢弃绑定，也不会创建可供臂内读取的名称。

带字段枚举变体使用花括号解构：

```tl
match action {
    Action.archive => { print("archive"); }
    Action.reply { to, tone: style } => {
        print(to, style);
    }
}
```

`field` 将字段绑定到同名局部变量，`field: name` 重命名，`field: _` 显式忽略。
一个带字段模式必须把该变体声明的每个字段恰好列出一次，顺序可以不同；未知、重复、
遗漏字段及重复绑定名都会报错。无字段变体继续写作 `Type.variant`，带字段变体不能只写
该单元形式。绑定只在对应臂内有效，可以遮蔽外层名称，也可以被臂内闭包捕获。

当前不支持保护、`|` 组合、范围、嵌套字段模式、整值别名绑定，以及类、数组或
映射解构。

## 13. 错误处理

### 13.1 抛出和捕获

`throw(error)` 只接受标准 `Error` 结构且不正常返回。记录的 `kind` 用于 catch 分类，
`message`、`origin` 和 `retryable` 保留在结构化值中；catch-all 绑定交付完整 `Error`。
字符串形式不再是语言契约。

```tl
import "std/error.tl" as errors;

try {
    throw(errors::make("not_ready", "try again", errors::ErrorOrigin.user, true));
} catch "not_ready" {
    print("recover");
} catch error {
    print(error);
}
```

catch 模式有两种：

- 字符串只匹配完全相同的稳定错误种类；
- 标识符捕获任意错误，并仅在该 catch-all 自己的子句体中绑定完整 `Error`；
- `_` 捕获并丢弃任意错误，不创建源级绑定。

一个 `try` 至少一个 `catch`。catch-all 绑定必须为最后一项。若只有过滤 catch 且均
不匹配，原错误继续向外传播。字符串过滤子句不引入 catch-all 名称；若外层已有同名
变量，过滤子句读取的是该外层绑定。

### 13.2 内建运行时错误种类

| 种类 | 含义 |
| --- | --- |
| `invalid_int` | 字符串不能转换为 `int` |
| `invalid_float` | 字符串不能转换为 `float` |
| `invalid_bool` | 字符串不是 `"true"` 或 `"false"` |
| `index_out_of_bounds` | 索引越界 |
| `slice_out_of_bounds` | 切片范围无效 |
| `unknown_map_key` | 映射键不存在 |
| `division_by_zero` | 除零保护触发 |
| `modulo_by_zero` | 模零保护触发 |
| `negative_exponent` | 整数指数在运行时为负数 |
| `negative_repeat` | 字符串或数组的重复次数在运行时为负数 |
| `none_value` | 外部/畸形字节码消费 NONE |
| `null_byte_in_string` | 本地 dylib 字符串含 NUL |
| `invalid_return_type` | 动态库返回值不符合声明 |

文件系统错误包括 `fs_not_found`、`fs_permission_denied`、
`fs_already_exists`、`fs_is_a_directory`、`fs_not_a_directory`、
`fs_read_only_filesystem`、`fs_storage_full`、`fs_timed_out`，其他 I/O 错误通常是
`fs_error`。宿主主动返回的错误在 Rust API 层可能是 `host_error`。

语法和静态类型错误发生在 VM 启动前，不能由 Teal `try` 捕获。

## 14. 静态变量

`static` 声明模块级可变状态，必须有初始化器：

```tl
static answer = 40 + 2;

fn increment() {
    answer += 1;
}


task main() {
    increment();
    print(answer);
}
```

静态变量可由表达式或函数调用初始化，也可引用先前可见的静态值。类型由初始化器固定；
联合初始化器允许后续赋相应成员。静态值位于独立全局槽，不是闭包捕获。

导入模块中标记为 `pub` 的静态变量可通过命名空间读取和修改：

```tl
geometry::origin_x = 1;
```

静态可变状态使调用顺序可观察，可复用模块应缩小其使用范围。

### 14.1 编译期常量与深层只读

`const NAME = expression;` 在模块或函数块内声明编译期常量，可写显式类型；模块声明可用
`pub const` 导出，并通过普通导入及重导出访问。常量名称采用 `SCREAMING_SNAKE_CASE`。

```tl
pub const LIMIT: int = 10;
const VALUES = [1, 2, 3];

fn main() {
    const DOUBLE = LIMIT * 2;
    let alias = VALUES;
    try { alias.push(DOUBLE); }
    catch "immutable_value" {}
}
```

初始化支持标量、none、常量引用、基本运算、数组/映射/类/枚举构造及常量字段和索引读取。
不支持普通函数、方法、Host、Task 或运行时变量；依赖环与非法算术在编译期拒绝。常量求值
不受优化等级影响；整数运算保留现有 32 位环绕规则。

常量名称不能重新赋值，其整个可达聚合对象图不可修改。直接写常量在编译期拒绝；通过
普通变量、函数参数、返回值、闭包或其他容器取得的别名，在修改冻结对象之前抛出
`immutable_value`。普通 `let` 别名仍可重新绑定。新建数组与切片自身可变，但共享的常量
子对象仍只读；读取不会隐式深拷贝。普通等值字面量不受常量冻结影响。

### 14.2 通用标准库

以下新增方法和模块均不要求 Host 扩展或权限，可用于核心运行时和 WASM。

| 接口 | 行为 |
| --- | --- |
| `int`/`float`: `min`、`max`、`clamp`、`pow` | 参数与接收者同类型；幂沿用 `^`，整数负指数失败 |
| `float`: `ceil`、`trunc`、`fract` | 返回浮点结果，分别为向上取整、向零截断和小数部分 |
| `float`: `sin/cos/tan`、`asin/acos/atan`、`atan2`、`hypot` | 角度采用弧度；`y.atan2(x)`，`x.hypot(y)` |
| `float`: `exp`、`ln`、`log2`、`log10` | 指数与自然/二进制/十进制对数 |
| `float`: `to_degrees`、`to_radians` | 弧度与角度互转 |
| `float`: `is_nan`、`is_infinite`、`is_finite` | 浮点分类 |
| `std/math.tl` | `PI`、`TAU`、`E` 为常量；`lerp(start, end, t)` 允许外插 |
| 数组 `any`、`all`、`find_index` | 按索引顺序短路调用谓词，空数组返回 false、true、-1 |
| 数组 `fold(initial, callback)` | 左折叠，回调为 `fn(accumulator, element)`，结果类型与 initial 相同；空数组返回 initial |
| 映射 `contains_key`、`remove` | 返回 bool；remove 原地删除，缺失返回 false |
| 映射 `keys`、`values` | 返回新数组快照，不保证顺序 |
| 字符串 `char_len`、`chars` | 按 Unicode 标量计数或拆分；`len()` 仍为 UTF-8 字节数 |
| 字符串 `lines` | LF/CRLF 分行，孤立 CR 保留，末尾换行无额外空项，空串返回空数组 |
| 字符串 `strip_prefix`、`strip_suffix`、`replace_once` | 去掉一次匹配前后缀或替换首个匹配；未命中保持原文，空替换模式在开头插入 |

浮点数学保留 NaN/Infinity，结果沿用 Teal 的 NaN 与零规范化规则；不承诺不同平台超越函数
结果逐位相同。`min/max` 传播 NaN；`clamp` 对倒置或 NaN 边界抛出 `invalid_clamp_bounds`，
相等边界及无穷边界合法。回调方法只捕获开始时的数组长度，与现有 `map/filter` 一致；
遍历中追加元素不扩大遍历范围，缩短数组可能产生索引错误，回调异常正常传播。

`std/encoding.tl` 提供 `utf8_encode`、`utf8_decode`、`hex_encode`、`hex_decode`、
`base64_encode`、`base64_decode`。encode 接受字节数组并返回文本，decode 反向转换；UTF-8
encode 接受文本、返回字节数组。字节用 `int[]` 表示并严格限制在 0–255。`string.to_utf8()`
和 `int[].to_utf8_string()` 是对应基元。错误分别为 `invalid_byte`、`invalid_utf8`、
`invalid_hex`、`invalid_base64`。Hex 输出小写、接受大小写；Base64 使用标准字母表和规范
等号补齐，拒绝空白、非法字符、错误 padding 与非零尾部位。

`std/path.tl` 的 `Style.posix`/`Style.windows` 显式决定路径风格，和宿主平台无关。
`join(parts, style)` 拼接数组；`normalize`、`basename`、`dirname`、`extension`、
`is_absolute` 接受 `(path, style)`。全部为词法处理，不访问文件系统或解析符号链接。
normalize 空路径为 `.`，消除重复分隔符和 `.`，绝对路径的 `..` 不越根，相对路径保留
无法消除的前导 `..`。basename 无文件名返回空串，dirname 单文件返回 `.`、根父仍为根；
extension 不含点，`.env` 无扩展名。

Windows 风格接受盘符绝对/相对、根相对及 UNC，输出反斜杠并保留大小写。拼接时完整
绝对路径替换前部，根相对路径继承已有盘符；`C:foo` 与 `\foo` 不是完整绝对路径。
NUL、设备路径和不完整 UNC 抛出 `invalid_path`。

## 15. 模块、导入和命名空间

### 15.1 导入与限定名称

被导入模块只暴露直接标记为 `pub` 的声明。例如 `modules/geometry.tl` 可以定义：

```tl
class InternalCache { value: int }

pub class Point { pub x: int, pub y: int }
pub enum Axis { horizontal, vertical }
pub static origin_x = 0;

fn normalize(value: int) -> int { return value; }
pub fn add(left: int, right: int) -> int {
    return normalize(left + right);
}
```

私有 helper 可由同文件 public API 使用，但导入者不能直接写出其限定名称。

```tl
import "modules/geometry.tl";
import "modules/geometry.tl" as geo;

class Scene {
    focus: geometry::Point,
}


fn main() {
    let sum = geometry::add(2, 3);
    let point = geo::Point { x: 4, y: 5 };
    let axis = geometry::Axis.horizontal;
    geometry::origin_x = 1;
}
```

路径必须为字符串并以分号结束。文件系统宿主相对导入者解析路径；编译器通过
`SourceLoader` 获取源码，不直接依赖文件系统。

无别名时，命名空间来自加载源码的文件 stem，例如 `geometry.tl` 通常产生
`geometry`；有别名时使用别名。`::` 可限定函数、静态变量、类、枚举及构造。
尝试访问存在但未导出的声明会得到 private 诊断及声明位置；真正不存在的名称仍报告
unknown。公开 API 可以返回或接收私有名义类型值，调用者可通过类型推断继续传递该值，
但不能直接声明其私有类型名。

### 15.2 缓存与入口

加载器提供稳定 `identity`。同一 identity 的源码只需收集一次，但可在不同导入位置或
别名下挂接。建议保持依赖图清晰且无环。

导入命名空间仅供当前模块使用，不支持 `pub import`，所以 `a` 导入 `b` 不会使调用者
能够访问 `a::b::*`。需要公开依赖中的单个成员时，先普通导入，再写选择性重导出：

```tl
import "geometry.tl" as geometry;

pub geometry::Point as PublicPoint;
pub geometry::add as sum;

pub fn local_sum(left: int, right: int) -> int {
    return sum(left, right);
}
```

调用者使用 `facade::PublicPoint` 和 `facade::sum`；`facade::geometry` 仍不可见。无 `as` 时
使用目标末段作为导出名。导入与重导出的文本顺序无关，但目标必须是唯一的 public
函数、静态、类或枚举；private、未知、歧义或与当前模块任意顶层名称冲突都会报错。
多层模块必须逐层显式重导出。动态库命名空间同样局限于声明文件，模块应以 `pub fn`
包装需要提供给调用者的本机能力。

被导入文件可声明函数、类型、静态值和文件专用 `main`。入口和普通函数均不会因导入自动执行；
静态初始化属于程序初始化流程。程序只执行显式根源码（`source_id = 0`）的 `main` 入口。

## 16. 内建函数

内建函数、保留命名空间和基础类型方法的签名与文档由编译器声明表定义，并由随发行分发的
`runtime/packages/std/builtins.tli` 渲染成制品供编辑器与知识库读取。该文件是只读接口制品，
不是可导入或可执行的 Teal module；普通 package 不能声明新的 compiler Builtin。
该接口也声明只约束形状的迭代协议 `Iterable` 与 `Iterator`（语义见 8.4 节），以及集合的
`iter()`、迭代器值类型 `Iterator<T>` 上的 `map`/`filter`/`take`/`skip`/`take_while`/
`skip_while`/`chain` 与 `collect`/`count`/`any`/`all`/`find`/`fold`（语义见 8.5 节）。
本章末尾另说明随发行目录提供的可导入标准包模块。

### 16.1 输入输出

| 调用 | 返回 | 行为 |
| --- | --- | --- |
| `print(values...)` | `none` | 返回前完成输出；参数求值顺序、格式和空参数行为与 print 相同 |
| `input()` | `string` | 返回时已经读取一行，去掉末尾 CR/LF |
| `input(prompt)` | `string` | 先输出提示，再读取一行；提示失败时不继续读取 |

`print(a, b)` 在返回前输出 `a\tb\n`；制表符只出现在参数之间。参数按源码顺序
在调用点求值，实参求值失败时不会产生部分输出。底层控制台写入仍是标准 Host 操作，
VM 可以在 Host 边界挂起并由驱动器恢复，但 Teal 源码不产生 `Task` 句柄。

普通 `fn` 可使用 `print` 和 `input`，在调用处处理返回值与错误；`task` 也可以使用
这两个同步入口。同步表示当前调用返回前操作已完成，VM 仍可把 Host 请求挂起交给驱动器处理。
本机同步驱动器读取控制台时可能阻塞线程。

```tl
fn greet() {
    let name = input("Name: ");
    print("Hello", name);
}

task main() {
    greet();
    print("Done");
}
```

`input(prompt)` 在当前调用帧先写提示再读取一行；提示写入失败时不会继续读取。
无参数的输入不产生空提示写入。交互输入默认不允许自动持久恢复，输出仍按标准 Host
操作记账。

### 16.2 类型与转换

| 调用 | 返回 | 行为 |
| --- | --- | --- |
| `typeof(value)` | `string` | 编译期静态类别，不执行值 |
| `float(value)` | `float` | 接受 `int` 或 `string` |
| `int(value)` | `int` | 接受 `float`、`string` 或枚举；枚举返回判别式 |
| `string(value)` | `string` | 格式化 Teal 值 |
| `bool(value)` | `bool` | 只接受 `"true"` 或 `"false"` |

整数字符串允许一个可选正负号和前导零，必须完整落在 `i32` 范围内；浮点字符串支持普通
小数和指数形式。两者都不自动裁剪首尾空白。转换失败抛出 `invalid_float`、`invalid_int`
或 `invalid_bool`。整数解析与格式化由 Teal 自身的已检查的 decimal 实现完成，不依赖
外部 lexical conversion 库。

### 16.3 范围、参数和进程

| 调用 | 返回 | 行为 |
| --- | --- | --- |
| `range(start, end)` | `int[]` | 生成左闭右开数组 |
| `argv()` | `string[]` | 宿主传入的参数 |
| `exit()` | `none` | 状态 0 停止 |
| `exit(code)` | `none` | 给定 `int` 状态停止 |
| `throw(error)` | `none` | 抛出结构化 `Error` 记录 |

`exit` 立即 halt，不执行后续语句；CLI 将退出码交给进程。

旧内建 `the_answer()` 已移除；源码可以直接使用 `42`，或按需声明返回该值的普通用户函数。

### 标准终端 UI 模块

`import "std/ui.tl" as ui;` 由统一内置扩展包目录解析；保留的 `std/` 与 `bench/` 导入不能被本地文件覆盖。发行目录缺失时使用构建阶段产生的预编译 Module IR。模块导出 `Node`、`Component`、`TreeItem`、`Style`、`Length` 和 `UiEvent`。`ui::node(id, component)` 默认采用自动高度和填充宽度。

组件包括 row、列、grid、text、button、input、list、table、tree、progress 和 spacer。`ui::encode(root)` 输出 v1 JSON 信封，限制深度 32、大小 2 MiB。文本支持 Unicode 及转义的换行、回车、Tab、NUL；其他 ASCII 控制字符会被拒绝。宿主还须校验 ID 唯一性、节点数量和布局边界。`ui::event(map)` 将字符串信封转换为 mount/focus/键/activate/change/selection/resize/tick 事件，拒绝未知类型。标准库不导入任何宿主能力。

最小 UI 构造和事件解码示例：

```tl
import "std/ui.tl" as ui;

task main() {
    let title = ui::node("title", ui::Component.text {
        text: "Teal", style: ui::Style.heading, wrap: false,
    });
    let button = ui::node("ok", ui::Component.button {
        label: "OK", enabled: true,
    });
    let root = ui::node("root", ui::Component.row {
        children: [title, button], gap: 1,
    });
    print(ui::encode(root));

    let event = ui::event({"type": "activate", "id": "ok"});
    print(typeof(event));
}
```

### 标准包

发行目录还提供标准包 `git`、`http`、`oauth` 和 `feishu`，它们通过 catalog 加载预编译
Module IR，不改变语言语法和程序格式。包导入按 catalog 名称识别；已登记包的未知模块不能回退
到工作区文件。包内跨包导入必须在清单声明依赖。

| 包 | 导入入口 | 依赖与职责 |
| --- | --- | --- |
| Git | `git/git.tl` | `std`；以参数数组调用系统 Git |
| HTTP | `http/client.tl`、`http/json.tl` | `std`；网络请求、JSON 构造与 URL 编码 |
| OAuth | `oauth/oauth.tl` | `http`、`std`；授权码、PKCE、刷新与回环回调 |
| 飞书 | `feishu/auth.tl`、`client.tl`、`docs.tl`、`drive.tl`、`wiki.tl`、`markdown.tl` | `oauth`、`http`、`std`；云文档工作流 |

#### 运行与授权

本机运行使用 `native` 或 `agent` profile。profile 只决定实现可用性，不授予权限。通过 CLI 的
capability 与资源授权参数或嵌入宿主策略，授权实际访问的程序、文件、网络端点。

- Git 使用 `process.execute` 和 Git 可执行路径、工作目录授权，继承现有进程信任边界。Git
  本身仍可执行 hooks、访问远程和修改仓库；此封装不提供 Git 子操作级沙箱。
- HTTP 使用 `network.http` 和网络资源授权；multipart 文件另需对应读资源授权。默认返回
  3xx，不跟随重定向、不自动重试，不转发认证信息到 Location。
- OAuth 授权上下文需要 `crypto.random`；本地回调需要 `network.oauth_callback` 与
  `http://127.0.0.1:<port>/` 资源授权。环境凭证读取需要 `environment.read` 和具体环境变量授权。
- 飞书文件上传使用 `filesystem.inspect` 查询大小；Markdown 轮询需要 `clock.read`。读取 Markdown
  源文件和本地图片均受文件授权约束。

HTTP、OAuth token 交换和 Git 写入都不声明可重放恢复；未知执行结果应检查现场。回调句柄属于
当前宿主实例，不可跨进程恢复。`oauth::close` 可释放尚未进入等待的句柄；等待中的回调在成功或
超时后关闭。

#### Git

`repository(path)` 默认使用 PATH 中的 Git 和 120 秒超时。`Repository` 的路径、可执行文件和
超时通过同名 getter 与 `set_path`、`set_executable`、`set_timeout_ms` 调整，调用方不能直接使用
字段字面量构造。`run` 提供原始参数入口；常用封装包括 `status`、`log`、`diff`、`add`、`commit`、`init`、
`clone`、分支与远程管理、`fetch`、`pull`、`push`、`merge`、`rebase`、`reset`、`clean`、stash 和 worktree。

命令返回 `CommandResult`，保留退出码和诊断；需要成功时调用 `checked`。冲突保留现场，使用
`resolve_operation` 继续或中止。`pull` 只快进，`push` 不隐式强推；`reset` 明确选择模式，`clean`
要求非空路径并明确选择预览。封装禁止 Git 终端凭证提示；远程认证由现有 Git credential
helper/SSH 环境提供。

状态使用 NUL 分隔解析，支持空格、换行和 Unicode 路径；底层进程文本接口不保证非 UTF-8 文件名字节往返。输出超过捕获上限时返回 `Result.err`，不返回不完整结构。

示例：[查询 Git 状态](../examples/packages/git_status.tl)。

#### HTTP 与 OAuth

`Request` 包含方法、URL、有序且可重复的头、`Body`、超时和响应上限。请求体支持文本、base64
二进制和 multipart。响应保留状态、头、base64 内容；有效 UTF-8 内容同时提供文本。HTTP 非成功
状态仍是响应，传输失败、超时和超过限制返回 `Result<Response, Error>`；需要异常边界时显式使用
`send_or_throw`。

默认超时 30 秒，最大一小时；响应默认及最大 8 MiB。multipart 文件部分总计最多 64 MiB。文本和
base64 请求还受 16 MiB JSON 控制信封限制。二进制采用 base64，未引入语言 bytes 类型。JSON 对象构造拒绝重复字段；编码接口会验证每个嵌套值。

OAuth 通用配置支持 JSON 或表单 token 请求体。`decode`、`listen`、`wait`、`token_request`、
`exchange` 和 `refresh` 返回 `Result<_, Error>`，`decode_or_throw` 等显式适配器才抛出结构化
`Error`。`begin` 生成不透明的 `Authorization`，仅通过
`url()` 展示授权地址；state 与 S256 PKCE verifier 只由 `wait` 和 `exchange` 使用，不向调用方
公开。`exchange` 校验 state、重复参数、拒绝和缺少授权码；`refresh` 返回新的凭证。使用服务端
有效期，调用方管理刷新时机与保存方式，不把旧刷新凭证用于不明确的重试。

本地登录先 `listen`，再展示授权 URL，然后 `wait` 与 `exchange`；回环监听仅绑定 IPv4 127.0.0.1，
最多八个待接收句柄，最长五分钟。也可由调用方提供完整回调 URL。包不自动打开浏览器，不读取
第三方工具的凭证，不把 token 写入磁盘。

示例：[HTTP 请求](../examples/packages/http_get.tl)、[飞书用户授权并创建文档](../examples/packages/feishu_oauth.tl)。

#### 飞书与 Markdown

`auth::provider` 配置飞书 OAuth v3。`tenant_token` 返回 `Result<TenantToken, Error>` 获取自建应用身份；用户授权或外部 token 通过
`client` 注入。客户端地址、超时和身份分别使用 `set_base_url`、`set_timeout_ms` 与 `set_token`
更新；token 没有读取入口。`base_url()` 可用于核对本地模拟服务配置。应用权限、用户授权范围和
文档访问权都需要满足；不要将真实凭证传给不受信任端点。

`docs`、`drive`、`wiki` 和 `markdown` 的可预期失败都返回 `Result<_, Error>`；调用方应匹配 `Result.ok`/`Result.err`，或显式使用同名 `_or_throw` 适配器。`docs` 支持创建、元数据、纯文本、分页及全部块读取、增删改。块保留官方 JSON，避免对未知块做有损转换。`drive` 提供文件夹、复制、移动、删除、协作者和素材上传，单次素材上传最多 20 MiB；`wiki` 提供空间和节点查询、创建、移动、改标题与将文档移入知识库。分页原语返回服务端 continuation，`docs::all_blocks` 额外防止重复 token 和无限分页。

Markdown 使用官方导入，不实现另一套完整格式转换器：

1. `markdown::start` 接收文本、图片基目录、标题及目标文件夹，上传 `.md` 并返回 `Result<Import, Error>`；成功值包含 `Import.ticket`。
2. `markdown::wait` 有界查询，返回 `Result<Imported, Error>`，其中包含文档标识、URL、图片清单及服务端 `extra` 警告；保留 ticket 可再次查询。
3. 每张本地图片先 `attach_image` 返回新块 ID，再 `fill_image` 上传、绑定，成功后删除原标记。保存图片块 ID 可定位部分失败。
4. 如需知识库位置，用 `wiki::move_document` 移入目标节点，并检查其返回结果。

本地图片通过 Markdown 解析器识别（包含引用式图片），百分号编码路径会解码，按显式基目录解析。图片作为独立块插入，行内图片会转为独立段落，不能承诺原布局逐字保真；远程图片由飞书导入器处理。若标记丢失、重复、父块改变或上传失败，报告错误并保留文档；不自动重新导入或删除整个文档。服务端可能截断超限内容，必须检查 `warnings`。Markdown 导出和向已有文档追加 Markdown 不在本版范围。

示例：[导入 Markdown 与本地图片](../examples/packages/feishu_import.tl)。真实验证需要调用方提供有效凭证和明确的测试写入位置；本地模拟测试不能证明租户权限或官方服务端的实际转换效果。

官方契约：[OAuth v3](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/authentication-management/access-token/get-user-access-token-v3)、[创建文档](https://open.feishu.cn/document/server-docs/docs/docs/docx-v1/document/create)、[导入任务](https://open.feishu.cn/document/server-docs/docs/drive-v1/import_task/create)、[查询导入](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/drive-v1/import_task/get)。

## 17. 内建方法

方法调用 `receiver.method(args...)` 由编译器处理为带隐式首参数的内建调用。“原地”
表示修改接收者引用指向对象，并且没有可用于表达式的具体返回值。

### 17.1 字符串

| 方法 | 返回 | 说明 |
| --- | --- | --- |
| `uppercase()` / `lowercase()` | `string` | Unicode 大小写转换 |
| `starts_with(text)` / `ends_with(text)` | `bool` | 前后缀判断 |
| `replace(old, new)` | `string` | 替换全部匹配 |
| `len()` | `int` | UTF-8 字节数 |
| `contains(text)` | `bool` | 字节子串查找 |
| `trim()` / `trim_left()` / `trim_right()` | `string` | 去除 Unicode 空白 |
| `trim_sequence(chars)` | `string` | 两端去除给定字符集合 |
| `trim_sequence_left(chars)` / `trim_sequence_right(chars)` | `string` | 单侧字符集合裁剪 |
| `find(text)` | `int` | 首个 UTF-8 字节偏移，未找到为 -1 |
| `is_float()` / `is_int()` | `bool` | 是否可解析 |
| `repeat(count)` | `string` | 返回重复字符串；负数抛出 `negative_repeat` |
| `reverse()` | `string` | 按 Unicode 标量值反转 |
| `split(separator)` | `string[]` | 按字面分隔符切分 |
| `map(callback)` | `string` | `fn(string)->string` 逐字符转换并拼接 |
| `filter(callback)` | `string` | 保留使 `fn(string)->bool` 为真的字符 |

### 17.2 数组

| 方法 | 返回 | 说明 |
| --- | --- | --- |
| `len()` | `int` | 元素数 |
| `contains(value)` | `bool` | 查找相等元素 |
| `find(value)` | `int` | 首个索引，未找到为 -1 |
| `repeat(count)` | `T[]` | 返回重复后的新数组；负数抛出 `negative_repeat` |
| `push(value)` | `none` | 原地追加 |
| `remove(index)` | `none` | 原地移除，越界抛错 |
| `reverse()` | `none` | 原地反转 |
| `sort()` | `none` | 原地升序排列 int、浮点数或字符串 |
| `partition(separator)` | `T[][]` | 按相等分隔元素切分，分隔元素不保留 |
| `map(callback)` | `U[]` | 逐元素转换为新数组 |
| `filter(callback)` | `T[]` | 保留回调返回 true 的元素 |

`sort` 依据首元素选择整数、浮点或字符串排序；其他元素类型没有有用排序定义。浮点 NaN
使用当前 VM 的部分排序兜底，不适合作为稳定协议。

`map` 回调接受一个元素并返回一致类型；`filter` 回调返回 `bool`。两者都可用闭包。

### 17.3 映射、数字和字符串数组

| 接收者 | 方法 | 返回 | 说明 |
| --- | --- | --- | --- |
| 映射 | `len()` | `int` | 键值对数 |
| 映射 | `get(key)` | `V` | 缺失抛 `unknown_map_key` |
| 映射 | `insert(key, value)` | `none` | 原地插入或覆盖 |
| `int`/`float` | `abs()` | 同接收者 | 绝对值 |
| `float` | `sqrt()` | `float` | 平方根 |
| `float` | `round()` | `float` | 舍入 |
| `float` | `floor()` | `float` | 向负无穷取整 |
| `string[]` | `join()` | `string` | 无分隔连接 |
| `string[]` | `join(separator)` | `string` | 使用分隔符连接 |

`sqrt`、`round`、`floor` 不接受 `int`，需先转 `float`。`{}` 是空映射字面量；
`map[key]` / `map[key] = value` 是 `get` / `insert` 的运算符形式。

## 18. 文件系统命名空间

文件系统接口位于普通标准模块 `std/fs.tl`，使用前显式声明
`import "std/fs.tl" as fs;`，也可以选择其他别名。`fs` 不再是编译器保留命名空间。
可达调用自动推导 `filesystem`；读取、写入和删除按具体路径及操作经过统一权限规则：

| 调用 | 返回 | 行为 |
| --- | --- | --- |
| `fs::read(path)` | `Task<string>` | 读取 UTF-8 文本文件 |
| `fs::exists(path)` | `Task<bool>` | 判断路径是否存在 |
| `fs::write(path, contents)` | `Task<none>` | 覆盖写入文本 |
| `fs::append(path, contents)` | `Task<none>` | 追加文本 |
| `fs::delete(path)` | `Task<none>` | 删除文件 |
| `fs::delete_dir(path)` | `Task<none>` | 删除空目录 |

参数均为 `string`，调用后使用 `.await` 等待结果。编译器只生成宿主调用，不在编译阶段访问文件。路径含义、权限、编码
和错误来自运行宿主。

`NativeHostRuntime` 提供本机文件系统。自定义 Rust 宿主可实现虚拟文件系统或安全策略，因而
权限边界由宿主决定。不要把 Teal 脚本提供的路径视为已通过沙箱验证。

## 19. 安全宿主函数

嵌入程序通过 `HostFunctionDefinition` 描述逻辑模块、精确签名、能力标签、恢复属性和 Rust 回调，再以 `HostRegistry::register` 登记。注册表保存可复用定义；VM 加载得到的调用绑定固定到一个 HostRuntime 的生命周期与授权策略。源码只声明调用签名：

```tl
host "rein.editor" as editor {
    fn read(arg0: int) -> string;
    fn apply(arg0: string, arg1: bool);
}


fn main() {
    let source = editor::read(0);
    editor::apply(source, true);
}
```

签名为 `[返回类型] 函数名(参数类型, ...);`，省略返回类型表示 `none`。参数和返回值
允许 `bool`、`int`、`float`、`string`、数组、映射、用户声明的 `class` 与 `enum`；
`none` 允许作为安全 Host 参数、返回以及受支持容器的成员类型。函数值、union、未解析类型和 Agent/GraphTask 句柄不能跨越
该边界。类与枚举按名义身份匹配：宿主定义用限定名声明，加载期解析到本程序的布局，
实参和返回值的字段与判别值按布局递归核对，超过 64 层的值会失败关闭。数组和映射按值
递归复制，不能携带原生指针；循环集合会失败关闭，类与枚举的循环对象图同样在进入边界
时被拒绝。

`CompiledProgram::execution_request` 在运行前通过 HostRuntime 互证签名，返回实际 Extension 与去重底层操作；
统一 `ExecutionGrant` 给出主体、操作和类型化资源上限，`HostExecutionGrant` 绑定到 HostRuntime。缺失操作、函数或签名不匹配发生在
load 阶段；路径、环境和可执行文件等动态资源在回调前再次授权。Capability、资源和工具名必须分别授权。

### 19.1 标准本机模块

本机 CLI 注册 `std/process.tl`、`std/fs.tl`、`std/crypto.tl`、`std/json.tl` 和
`std/system.tl`。可达内置扩展模块调用按实际功能推导 Extension，运行命令按实际操作和类型化资源使用统一权限规则。

- `process` 以程序和参数数组直接启动，不经过 shell；支持捕获上限、异步 token、轮询、等待、
  精确终止与完整可执行路径查询。
- `fs` 提供规范化、元数据、稳定枚举、递归建目录、二进制复制和活动文件尾读。
- `crypto::sha256_file` 从同一打开句柄读取，并在读取前后检测文件身份、长度和修改时间变化。
- `json` 使用 RFC 6901 Pointer，输入上限 16 MiB、嵌套深度上限 128。
- `system` 提供平台、当前可执行文件、环境变量、UTC 与单调时钟。

稳定操作名称由 `teal-perm operations` 输出。进程操作在可恢复 Agent 运行中为 `Unsupported`；
文件读取、哈希、环境和时钟为 `Journaled`；JSON 查询为 `Pure`。

CLI 显式授予 `process.control` 时，只自动开放当前 `HostRuntime` 自己创建的不可伪造进程 token；它不授权枚举或控制任意操作系统进程。可执行文件与工作目录仍分别需要 `--allow-execute` 和读取目录授权。

标准本机包的常用签名如下。带 `task` 的调用必须在 task 中使用 `.await`；JSON、UI 和
部分 system 查询是同步纯函数：

| 包 | 调用 | 返回 | 主要能力 |
| --- | --- | --- | --- |
| `std/fs.tl` | `create_dir_all(path)`、`canonicalize(path)` | `none` / `string` | `filesystem.mutate` 或 `filesystem.inspect` |
| `std/fs.tl` | `metadata(path)`、`list(path, recursive)` | `FileEntry` / `FileEntry[]` | `filesystem.inspect` |
| `std/fs.tl` | `copy(source, target, overwrite)`、`read_tail(path, bytes)` | `string` / `TailSnapshot` | 读写范围分别授权 |
| `std/process.tl` | `command(program, args)`、`run(command, timeout_ms)` | `ProcessSpec` / `ProcessResult` | `process.control`、执行和目录能力 |
| `std/process.tl` | `spawn(spec)`、`status(handle)`、`wait(handle, timeout_ms)`、`terminate(handle)` | `ProcessHandle` / `ProcessResult` | `process.control` |
| `std/json.tl` | `has(text, pointer)`、`read_string(text, pointer)`、`read_number(text, pointer)` | `bool` / `string` | `json.read` |
| `std/json.tl` | `read_boolean(text, pointer)`、`items(text, pointer)`、`array_fields(text, pointer, fields)` | `bool` / `string[]` | `json.read` |
| `std/crypto.tl` | `sha256_file(path)` | `FileHash` | `filesystem.inspect` |
| `std/system.tl` | `platform()`、`current_executable()`、`utc_now()`、`monotonic_ms()` | `string` | `system.inspect` |
| `std/system.tl` | `environment(name)` | `EnvironmentValue` | 按名称授权环境变量 |

`ProcessHandle` 只能由 `spawn` 取得，控制操作使用其私有宿主 token；`pid()` 只返回诊断用进程标识，
不能替代句柄或扩大 `process.control` 权限。

最小文件系统例子：

```tl
import "std/fs.tl" as osfs;

task main() {
    let root = "scratch";
    osfs::create_dir_all(root).await;
    osfs::write(root + "/input.txt", "hello").await;
    let copy = root + "/copy.txt";
    osfs::copy(root + "/input.txt", copy, false).await;
    print(osfs::metadata(copy).await.size);
    print(osfs::list(root, false).await.len());
    osfs::delete(root + "/input.txt").await;
    osfs::delete(copy).await;
    osfs::delete_dir(root).await;
}
```

进程调用显式等待，JSON 查询同步返回：

```tl
import "std/json.tl" as json;
import "std/process.tl" as process;

task main() {
    let document = "{\"items\":[{\"name\":\"teal\"}]}";
    if json::has(document, "/items/0/name") {
        print(json::read_string(document, "/items/0/name"));
    }
    let command = process::command("teal", ["--version"]);
    let result = process::run(command, 10_000).await;
    print(result.stdout);
}
```

## 20. 动态库接口

### 20.1 声明

`dylib` 在顶层声明本地动态库及函数。可达调用自动推导 `dylib`；编译目标必须使用支持该 Extension 的 RuntimeProfile，执行还需精确库或库/符号权限规则：

```tl
dylib "native" {
    int add(int, int);
    float distance(float, float, float, float);
    log(string);
}
```

签名是 `[返回类型] 函数名(参数类型, ...);`。省略返回类型表示 `none`，参数没有名称。

### 20.2 ABI 类型和责任

动态库只在支持 dylib 的本机宿主中可用，并与安全 `host` 声明采用不同授权规则。
脚本作者必须确保声明与导出符号的真实
C ABI 完全一致。基础类型、字符串、一维基础输入数组和符合布局要求的类与枚举可进入相应转换
路径；联合、函数值、映射等不能作为通用 C ABI 值。

- 字符串传给 C 时必须能形成无内部 NUL 的 C 字符串；
- `int[]`、`float[]`、`bool[]`、`string[]` 顶层参数分别作为对齐的 `int32_t*`、`double*`、
  `uint8_t*`、`char**` 输入；长度必须由声明中的另一 `int` 参数显式传递，空数组传空指针；
- 数组返回、嵌套数组、类数组及类中的数组字段均在编译期拒绝；
- 类与枚举按值传递，其字段必须逐层具有可转换类型，布局由 Teal ABI 元数据决定；当前字段只接受
  `int`、`float`、`string` 和同样满足约束的非递归嵌套类与枚举，`bool`、数组与其他值类别只
  支持作为顶层参数或返回值；
- 枚举使用 C 结构 `{ int32_t tag; union { ... } payload }`：`tag` 是源码 `int(enum_value)` 可见的
  带符号判别值，载荷按最大变体对齐，C 侧必须声明相同的标签与载荷布局；返回的枚举若带有未知
  判别值则整个调用失败；
- 非字符串返回值使用声明的精确类型；C 字符串返回的空指针可能成为 `none`，因此当前
  不能把这类结果作为确定 `string` 直接消费；
- 库名称、扩展名、搜索路径和符号命名依赖平台；
- `teal-c` 编译通过不表示运行时一定找到库或符号。

`dylib` 扩展只允许生成和加载该类字节码，不授予库访问权限。本机 CLI 默认拒绝；运行时
必须通过 v3 权限规则允许 `dylib.load` 或精确的 `dylib.call`，或由嵌入宿主使用
风险更高的 `--allow-all-dylibs`。嵌入宿主使用 `DylibPolicy` 提供同样的默认拒绝、精确
授权表或显式全部允许策略。

动态库错误发生在 VM 装载阶段，通常在所选条目执行前，不能由脚本内 `try` 捕获。外部库应
先以最小声明检查，再做目标平台 ABI 集成测试。

## 21. 命令行和宿主差异

### 21.1 命令行

根 package 不提供库 target，只生成派发到 sibling 的 `teal` 入口；十四项产品工具由各自能力 package 生成，`default-run` 指向 `teal`。

```text
teal run <file.teal> [编译器选项] [观测选项] [dylib 授权] [-- <args>...]
teal run (-e | --code) <source> [编译器选项] [观测选项] [dylib 授权] [-- <args>...]
teal compile <file.teal> [编译器选项] [--otel-traces] [--otel-metrics]
teal-fmt [--check] [<file.teal>...]
teal-lsp
teal-dap
teal audit list|export|verify|clear
teal bench [PATH]
teal [-v | --version]
teal [-h | --help]
```

位置参数始终作为文件读取；内联源码必须显式使用 `--code`。

`teal-run` 与 `teal-c` 共用以下编译器选项：

- `--runtime-profile <PROFILE>`：选择内置或配置声明的目标实现配置档；
- `--build-profile <debug|release>`：选择用户可见的构建配置；
- `--optimization <none|basic|aggressive>`：选择字节码优化等级；
- `--debug` / `--no-debug`：开启或关闭编译器调试输出；
- `--debug-info` / `--no-debug-info`：开启或关闭调试器变量绑定快照；
- `--allow|--warn|--deny|--forbid <CSV>`：按检查项名或组名设置诊断等级。

运行命令使用 `--permission-preset explore|ask|develop`、`--interaction auto|never` 和 `--permission-file FILE`。`explore` 只自动允许只读检查，其余操作直接拒绝；`ask` 逐项询问；`develop` 额外自动允许工作区内常规读取与编辑。权限文件与用户配置同为 v3 类型化规则；旧 capability、资源和 dylib allow-list 参数不再接受。编译检查不执行宿主调用，因此不接受权限参数。

CLI 选项使用 `clap` 的类型化定义；`teal --help` 说明派发关系，各独立 bin 的 `--help` 是当前构建的权威
参数表。未知选项、缺少值、`--code` 与位置输入冲突、非法枚举值和超界数值会在读取源码前
失败。编译器与观测选项可放在源码参数之前或之后；只有 `--` 后面的值会传给脚本，
编译命令不接受脚本参数。重复的 optimization、条目、成对布尔开关和检查项设置按
命令行顺序后写覆盖前写，`--runtime-profile` 只能出现一次。

`teal-c options|profiles|lints` 分别显示编译器选项、实现配置档及其 Extension 集合和检查项目录。
持久 run 由 `std/runs.tl` 与工作台 `:runs` 管理；旧 Bench run CLI 已破坏性删除。未知模型或工具副作用不会自动重放，非交互恢复保持 `needs_attention`。
工作台的启动选项、键位、脚本、Agent 配置和扩展用法见 [Teal 工作台使用手册](TEAL-BENCH-MANUAL.md)。

`std/user.tl` 提供持久信息交互：`user::ask(prompt).await` 返回 `Result<Answer, Error>`，
`user::choose(prompt, choices).await` 返回 `Result<ChoiceAnswer, Error>`。业务流程先匹配 Result，
再匹配回答或跳过枚举；回答不授予权限。工作台 `:runs` 与 `std.runs.inspect/answer_text/answer_choice/skip` 读取和提交回答；恢复仍单独要求当前权限策略批准。
协议和恢复边界见 [teal-agent 用户交互契约](../crates/teal-agent/README.md#用户交互会话记忆与预算)，完整示例见
[user_interaction.tl](../examples/agent/user_interaction.tl)。
`teal-run` 支持本节 20.5 描述的全部观测选项；`teal-c` 只执行编译阶段，因此只支持追踪、
指标及其依赖选项，不收集执行性能剖析。

### 21.2 本机和 Rust 嵌入

根包不再导出 C ABI。`NativeHostRuntime` 支持输出、标准输入、文件导入、文件系统、动态库和
CLI 参数；平台无关嵌入使用 `teal-core`，高层本机嵌入使用 `teal-runtime`。

Rust 嵌入者可调用 `compile_source`、`execute_program`、
`execute_program_with_audit`、`run_with_host`、`run_native`、`check_native`。
含外部 Host 副作用的程序应使用显式审计入口；可信纯嵌入才使用普通执行入口。
`SourceLoader` 控制导入，`HostRuntime` 控制 I/O 和参数；两者都是脚本权限边界。

### 21.3 编译优化与程序序列化

Rust 宿主可通过 `CompileOptions::with_profile`、CLI 可通过
`--build-profile <debug|release>` 选择用户可见的构建配置。内部仍保留三个优化等级：

| 等级 | 行为 |
| --- | --- |
| `OptimizationLevel::None` | 关闭可选常量折叠，保留与默认模式相同的语言语义和检查 |
| `OptimizationLevel::Basic` | 高级回归等级；在上述基础上做带溢出和错误边界检查的常量折叠 |
| `OptimizationLevel::Aggressive` | Release 使用的等级；承接 Basic 常量折叠，融合仅控制分支的整数比较，在没有调试元数据时把安全的直接静态返回调用编码为尾调用，并内联受限的同步小方法 |

例如，计算密集的入口可显式选择 Release 配置：

```tl
@profile("release");

class Counter { values: int[] }
impl Counter {
    fn increment() { self.values[0] += 1; }
}
fn main() {
    let counter = Counter { values: [0] };
    for i in 0..1000 {
        if i < 500 { counter.increment(); }
    }
    print(counter.values[0]);
}
```

设置对入口文件和所有导入文件一致生效。关闭可选优化不会关闭语言执行所需的转换，
也不会放宽语法、类型、Extension 或程序验证。Release 下被内联的方法可从运行时错误栈
和性能剖析调用栈中消失；DAP 调试执行仍强制使用 `none`，保留完整调用帧。当前尚没有通用
HIR 阶段会在类型分析完成后删除已证明不可达的语句后缀；这一步对 Debug、Release 和三个内部
优化等级一致生效，并保留 `unreachable-code` lint 的源码诊断。当前尚未实现公共子表达式消除
或最终字节码 CFG 重写；尾调用只适用于静态非闭包返回调用，CLI 未指定构建配置时使用默认的 Release。

`compile_source` 返回不暴露内部指令布局的 `CompiledProgram`。宿主可调用
`CompiledProgram::serialize()` 得到带格式版本、长度和完整性校验的二进制，再用
`CompiledProgram::deserialize(bytes)` 恢复。反序列化限制负载大小，并拒绝未知版本、
非法标志、长度或校验错误和无效程序。这个过程不会执行程序、打开动态库或调用宿主 I/O。

源码 Agent 宿主把 `CompiledProgram` 或持久字节码转换为 `AgentProgram` 后再建立
`AgentRuntime`。该边界统一保存当前格式字节、摘要和认证/用户交互预检信息；构造过程不会
取得 run claim、启动模型或执行 Host 操作。持久恢复在取得执行所有权前核对保存版本和摘要。

格式版本覆盖整个中间码模式；它用于明确拒绝不兼容输入，不承诺未来所有 Teal 版本都能
读取旧文件。CRC32 只检测意外损坏，不提供来源认证或防篡改能力。当前 CLI 没有
`compile`/`run-bytecode` 文件命令，Rust 宿主自行保存和读取 `serialize()` 返回的字节。

当前格式号和兼容策略见 [teal-runtime 持久格式契约](../crates/teal-runtime/README.md#持久格式与协议契约)。
旧文件按该策略明确迁移或拒绝，不会猜测旧负载布局。DAP 会请求调试所需的源码信息；
普通执行和调试执行的语言结果保持一致。

### 21.4 Tracing、Metrics 与客体性能剖析

已有 subscriber/提供器的 Rust 嵌入方使用 `ObservationSession::from_host(optional_meter, function_spans)`。此模式继承当前线程的追踪订阅器和范围，不安装分派器或导出器，也不刷新或关闭宿主提供器；调用方负责向异步/阻塞工作线程传递上下文。`finish()` 只返回本会话计数，此模式不收集 Profiles。传入 `None, false` 时，在不需要指标、性能剖析或函数范围的情况下保留无 observer 的 VM 执行路径。

`ObservationSession::execute_program(program, host_runtime, options)` 使用已经配置的 HostRuntime，在观测
强类型宿主函数程序的同时保留 `Vm::load_with_host_runtime` 的签名检查和错误发生前的输出。执行前
应把 `ExecutionGrant` 绑定到 HostRuntime，并可用 `CompiledProgram::execution_request` 提前展示请求。宿主指标每次执行只计一次，
不受会话生命周期影响。`ObservationSession::new` 保留下面描述的独立提供器所有权模式；字节码、
能力和 ABI 均不变。

独立程序可以显式开启宿主观测能力：

```text
teal run program.tl --otel-traces
teal run program.tl --otel-metrics
teal run program.tl --profile profile.pb.gz
teal run program.tl --otel-profiles
:runs profile <run-id> merged.pb.gz
```

这些开关默认全部关闭，不是语言 Extension。Tracing 记录 compile、load、
execute、宿主、dylib 和 GC 等有因果关系的阶段；逐 Teal 函数范围只有在额外指定
`--trace-functions` 时开启，永远不会为每条指令或每次分配创建范围。Metrics 使用有限枚举
属性记录阶段耗时、执行次数、指令数、最大调用深度、托管分配、GC、池大小、宿主与 dylib
耗时，避免把源码名和函数名放入高基数指标。

Teal 客体性能剖析分析 VM 内的 Teal 程序，而不是替代原生 Rust/系统 profiler。VM 维护
逻辑 `FrameId` 调用栈，默认每 10ms 协作采样一次墙钟时间，并把数组、类、带字段枚举、
闭包、上值、映射和非内联字符串的逻辑分配归属到当前 Teal 栈。GC 槽位复用仍算新的
逻辑分配；Rust allocator、`Vec` 扩容、dylib 临时缓冲和动态库内部不在这个分配性能剖析的
覆盖范围。宿主阻塞时间归属发起调用的 Teal 栈，同时由宿主范围/metric 单独说明。

`--profile` 生成 gzip pprof；实验性的 `--otel-profiles` 使用当前 OTLP
`v1development/profiles` 数据模型。若同时启用追踪，Profile Sample 会携带对应的
trace/范围 Link。显式请求的文件或网络导出失败会报告诊断；程序原本成功时 CLI 返回 1，
而程序自身错误或非零 `exit(code)` 仍有更高退出优先级。

### 21.5 LSP 语言服务器

`teal-lsp` 是通过标准输入输出工作的 LSP 3.17 服务器。编辑器应无参数启动它，并把 Teal
文档的 language identifier 设置为 `teal`：

```text
teal-lsp
```

当前模块提供：

- UTF-16 坐标下的增量文本同步；
- 语法、编译和检查项诊断；
- 补全、hover、签名提示、定义、引用及 prepare-rename/rename；
- 文档符号、工作区符号和完整语义 tokens；
- 复用 `teal-fmt` 规则的整篇文档格式化。

编辑中的不完整文本仍会建立容错词法单元/作用域索引；需要稳定语义结果的导航会退回当前文档
最后一次成功快照。引用和重命名跟随已解析的声明身份，不会仅凭跨文档同名猜测符号关系。
选择性重导出的别名作为声明参与补全、定义、引用和重命名。
格式化请求固定使用四空格缩进和 LF 行尾，只改变 token 之间的空白；语法不完整或文档已经符合
格式时返回空编辑。

客户端可以在 initialize 请求中显式指定分析入口：

```json
{
  "initializationOptions": {
    "entryFiles": ["C:/work/app/main.tl"]
  }
}
```

`entryFiles` 指定分析根文件；未指定时，包含顶层 `main` 的文档作为根分析。其他模块仍获得语法诊断和符号服务。打开的编辑器缓冲区覆盖磁盘上的同路径文件，并参与导入解析。旧 `entryPoints` 初始化字段被拒绝。

LSP 分析把当前构建支持的全部扩展作为全局范围，并自动推导可达代码需要的 Extension。
Warn 检查项发布为 LSP Warning，deny/forbid 和语义失败发布为 Error；导入文件的
诊断发布到该文件自己的 URI。编译器的 UTF-8 字节 `start + size` 范围会在两个边界都检查
后转换为 LSP 的 UTF-16 行列坐标。

`teal-lsp` 只读取、解析和检查源码，不执行程序，也不会调用 Host、dylib、模型或观测导出器。

### 21.6 DAP 调试适配器

`teal-dap` 是通过标准输入输出工作的源码级 DAP 适配器。编辑器无参数启动适配器：

```text
teal-dap
```

launch 模式通过 DAP `runInTerminal` 在外部终端启动独立 debuggee，因此脚本的标准输入、
输出和参数都属于该终端。适配器不会在自己的 stdio 协议通道中模拟终端。VS Code 风格的
请求参数示例为：

```json
{
  "type": "teal",
  "request": "launch",
  "name": "Debug Teal",
  "program": "${workspaceFolder}/main.tl",
  "runtimeProfile": "agent",
  "permissionFile": "${workspaceFolder}/permissions.toml",
  "optimization": "none",
  "compilerDebug": false,
  "debugInfo": true,
  "warn": ["style"],
  "deny": ["unreachable-code"],
  "cwd": "${workspaceFolder}",
  "stopOnEntry": false,
  "args": ["first", "second"]
}
```

这是 DAP launch 请求参数，不是直接传给 `teal-dap` 的命令行。`program` 必填，指定文件必须声明 `fn main` 或 `task main`；旧 `entry` 字段被拒绝。四种检查项级别既可写字符串数组，也可写逗号分隔字符串。旧 `allowCapabilities`、`allowRead*`、`allowWrite*`、`allowExecutables` 和 `allowEnvironment` 已删除，DAP 使用 `permissionFile` 统一授权。debuggee 使用与普通 CLI 相同的编译选项 registry，但为保证源码单步的
正确性，最终强制 `optimization=none` 和 `debug-info=true`。

当前支持源码断点、暂停/继续、step in/next/step out、调用栈、局部作用域、数组/映射/类/
枚举展开、caught/uncaught 异常断点、watch/hover 求值和 `setVariable`。当前不支持函数
断点、条件断点、hit condition 或任意 `setExpression`。

暂停态求值器刻意保持纯粹：它允许标量字面量、可见变量、布尔/数字/字符串运算、索引、
类字段、数组/映射构造和 `int`/`float`/`bool`/`string` 转换；不允许调用用户函数或方法，
也不允许宿主 I/O、文件系统或 dylib。`setVariable` 可修改局部、全局、上值、数组元素、
已存在的映射值和类字段，但目标静态类型必须接受新值；枚举载荷字段只读。

远程 attach 先在目标机启动隐藏的 debuggee 接口：

```text
teal-dap --debuggee --listen 127.0.0.1:4711 --token <secret> --program main.tl
```

然后由编辑器发送：

```json
{
  "type": "teal",
  "request": "attach",
  "name": "Attach to Teal",
  "host": "127.0.0.1",
  "port": 4711,
  "token": "<secret>"
}
```

词法单元在任何源码、调用栈、变量或控制命令被接受前验证，但只提供认证，不加密 TCP 内容。
监听地址应限制为 loopback 或可信网络；跨机器连接应置于 SSH 等加密隧道之后。更完整的
编辑器接入配置见 [语言工具实现细节](../crates/teal-lsp/README.md#格式化lspdap-与调试)。

## 22. 诊断、限制和编写建议

### 22.1 失败阶段

宿主区分 input、parse、compile、load、运行时、宿主六个阶段。编译和运行诊断携带
源码名、完整源码和 UTF-8 字节范围。脚本应匹配稳定运行时错误种类，不要解析面向人的
诊断文本。

### 22.2 当前限制

- 每个函数帧最多 256 个寄存器槽，过大的表达式、局部集合或调用窗口会编译失败；
- `int` 固定 32 位，源码范围使用 32 位 UTF-8 字节 `start + size`；
- 泛型函数和 trait bounds 已开放并在调用点单态化；`Task<T>` 仍是编译器提供的任务句柄内建类型，
  用户不能自行实现任务调度器；
- Trait 仅支持方法签名、关联类型和静态分派；没有特征对象、默认方法、`where`、继承、
  运算符重载、异步、线程、协程或包管理语法；
- `for` 接受内建形态、用户协议（8.4 节）与迭代器值（8.5 节）；迭代器提供惰性适配器
  `map`、`filter`、`take`、`skip`、`take_while`、`skip_while`、`chain` 与终结方法
  `collect`、`count`、`any`、`all`、`find`、`fold`，但没有生成器、`zip`、`enumerate`，
  协议名与 `Iterator` 也不能用作 bound，因此不能写出对任意可迭代类型的泛型函数；
- Agent 当前只在单机进程和工作区 SQLite 内调度；没有跨机器 Actor；
- 没有 `while` 语句、`do/while`、`switch`、`defer`、`yield`、可变/默认/命名参数和多返回值；
- 可通过 `T | none` 表示包含空值的联合类型；映射 `get` 的缺失键仍抛错，不自动返回 optional；
- `match` 只解构一层枚举字段；没有保护、组合/范围/嵌套模式或其他集合/类解构；
- 字符串 `len/find` 与索引/切片使用不同计量单位。

### 22.3 推荐风格

- 文件使用 `.tl`，根入口使用私有、无参数的顶层 `fn main` 或 `task main`；
- 可复用模块给直接 API 添加 `pub`；门面模块只选择性重导出需要稳定暴露的依赖成员；
- 公共函数参数、类字段、静态槽和复杂局部写显式类型；
- 普通语句写分号，列表保留末尾逗号；
- 用 `typeof` 调试静态类型，不用它模拟运行时分派；
- 未初始化变量尽快在所有分支赋值，不把它当 nullable；
- 用新 `let` 明确改变类型，不期待普通赋值扩宽；
- 非 ASCII 字符串不要混用 `len()` 字节数和索引字符数；
- 对转换、索引、切片、映射读取和文件操作使用 `try/catch`；
- 跨模块调用写 `namespace::function(value, ...)`；
- 动态库先用 `teal-c` 编译检查，再做目标平台集成测试；
- 日常先用 `teal-c` 编译检查，再运行行为测试。

### 22.4 “可解析”不等于“可编译”

解析器能保留多段限定后缀、限定变量赋值和用户函数方法式调用等 AST，语义层不保证所有
组合。`examples/core/parser_only/qualified_postfix.tl` 记录这一边界。

普通脚本应只依赖本手册明确的可执行形式：内建方法和类字段可用点号；用户函数使用普通
或 `::` 限定调用；不要把点号后的 `namespace::method` 当成已实现方法分派。

## 23. 语法速查

以下是面向作者的近似摘要，不是机器生成文法：

```text
module          := file-directive* item*
file-directive  := build-option | edition | lint
item            := file-directive* import | attribute* dylib
                 | file-directive* host | file-directive* reexport
                 | attribute* visibility? attributable-item | trait
                 | file-directive* impl
visibility      := "pub"
attributable-item := static | const | class | enum | agent | function

edition         := "@edition(\"2026\")" ";"
build-option    := "@" ("profile" | "optimization" | "compiler_debug"
                   | "debug_info")
                   "(" expression ")" ";"
lint            := "@" ("allow" | "warn" | "deny" | "forbid")
                   "(" static-string ("," static-string)* ","? ")"

trait           := visibility? "trait" IDENT "{" trait-member* "}"
trait-member    := "static"? trait-function-signature ";"
impl            := "impl" qualified-name ("for" qualified-name)? "{" impl-member* "}"
impl-member     := attribute* visibility? "static"? function

attribute       := "@" qualified-name attribute-tail?
attribute-tail  := "(" (expression ("," expression)* ","?)? ")"
                 | "{" (field-init ("," field-init)* ","?)? "}"
                 | "." IDENT ("{" (field-init ("," field-init)* ","?)? "}")?
field-init      := IDENT ":" expression

import          := "import" static-string ("as" IDENT)? ";"
reexport        := "pub" qualified-name ("as" IDENT)? ";"
static          := "static" IDENT (":" type)? "=" expression ";"
const           := "const" IDENT (":" type)? "=" expression ";"

class           := "class" IDENT "{"
                     class-field ("," class-field)* ","?
                     "}"
agent           := "agent" IDENT "{"
                     field ("," field)* ","?
                     "}"
class-field     := visibility? IDENT ":" type
field           := IDENT ":" type

trait-function-signature := ("fn" | "task") IDENT "("
                            (parameter ("," parameter)* ","?)? ")"
                            ("->" type)?

enum            := "enum" IDENT "{"
                     variant ("," variant)* ","?
                   "}"
variant         := IDENT ("{" field ("," field)* ","? "}")?
                   ("=" signed-int-literal)?
signed-int-literal := "-"? DECIMAL_I32

dylib           := "dylib" static-string "{" dylib-signature* "}"
dylib-signature := type? IDENT "(" (type ("," type)* ","?)? ")" ";"
host            := "host" static-string "as" IDENT "{" host-signature* "}"
host-signature  := type? IDENT "(" (type ("," type)* ","?)? ")" ";"

function        := ("fn" | "task") IDENT generic-parameters? "(" (parameter ("," parameter)* ","?)? ")"
                   ("->" type)? block
generic-parameters := "<" generic-parameter ("," generic-parameter)* ","? ">"
generic-parameter := IDENT (":" qualified-name ("+" qualified-name)*)?
anonymous-function := ("fn" | "task") "(" (parameter ("," parameter)* ","?)? ")" block
parameter       := binding (":" type)?
impl-type       := "impl" qualified-name ("+" qualified-name)*

block           := "{" statement* "}"
statement       := let ";"
                 | "_" "=" expression ";"
                 | return ";"
                 | break [label] [expression] ";"
                 | continue ";"
                 | expression assignment-op expression ";"
                 | expression ";"
                 | if-statement | for | loop | try | match
                 | class | block

let             := "let" (IDENT (":" type)? ("=" expression)?
                         | "_" (":" type)? "=" expression)
return          := "return" expression?
break           := "break" LABEL?
continue        := "continue" LABEL?
assignment-op   := "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "^="
slice           := expression "[" expression? ".." expression? "]"
map-literal     := "{" (expression ":" expression
                         ("," expression ":" expression)* ","?)? "}"

if-statement    := "if" expression block
                   ("else" "if" expression block)*
                   ("else" block)?

if-expression   := "if" expression "{" expression "}"
                   ("else" "if" expression "{" expression "}")*
                   "else" "{" expression "}"

loop-label      := LABEL ":"
loop            := loop-label? "loop" block
for             := loop-label? "for" binding ("," binding)? "in"
                   (expression | expression? ".." expression)
                   block

try             := "try" block ("catch" (static-string | binding) block)+
match           := "match" expression "{"
                     pattern "=>" block
                     (pattern "=>" block)*
                   "}"
pattern         := expression | "_"
                  | qualified-name "." IDENT "{"
                      (enum-field-pattern ("," enum-field-pattern)* ","?)?
                    "}"
enum-field-pattern := IDENT (":" binding)?

type            := atom-type "[]"* | type "|" type | impl-type
atom-type       := "int" | "float" | "bool" | "string"
                 | qualified-name
                 | "{" type ":" type "}"
                 | ("fn" | "task") "(" (type ("," type)* ","?)? ")" ("->" type)?

qualified-name  := IDENT ("::" IDENT)*
variable-path   := qualified-name ("." IDENT)*
expression-string := '"' (TEXT | escape | "{{" | "}}"
                            | "{" variable-path "}")* '"'
static-string   := '"' (TEXT | escape | "{{" | "}}")* '"'
binding         := IDENT | "_"
LABEL           := "'" IDENT
```

运算符和后缀优先级见第 7 章。可执行示例位于 `examples/`，是验证手册的
配套材料。

## 24. 示例索引与综合示例

本章把前文分散的最小片段组合成较完整的程序，便于从“规则”过渡到“写程序”。每个
示例都只使用前文已经说明的语法；标为“宿主相关”的片段需要相应操作授权或本机
运行时，不能在纯 core/WASM 配置中直接运行。

### 24.1 词法、绑定和表达式

```tl
// 注释、转义、插值、丢弃绑定和复合赋值
task main() {
    let name = "Teal";
    let version: int = 1;
    let text = "${name} v${version}";
    let escaped = "line\\n\\tquoted: \\\"ok\\\"";
    let _ = escaped;
    let mutability = 2;
    mutability += 3;
    print(text, mutability);
}
```

```tl
// 算术、比较、短路逻辑和 if 表达式
task main() {
    let left = 7;
    let right = 3;
    let quotient = left / right;
    let remainder = left % right;
    let larger = if left > right { left } else { right };
    let safe = right != 0 && quotient >= 0;
    print(quotient, remainder, larger, safe);
}
```

### 24.2 初始化、分支和循环

```tl
fn classify(value: int) -> string {
    if value < 0 {
        return "negative";
    } else if value == 0 {
        return "zero";
    }
    return "positive";
}

task main() {
    let value: int;
    if argv().len() > 1 {
        value = 1;
    } else {
        value = 0;
    }

    let mut count = 0;
    'outer: loop {
        count += 1;
        if count == 2 {
            break 'outer;
        }
        continue;
    }
    print(classify(value), count);
}
```

```tl
task main() {
    let values = [2, 4, 6, 8];
    let total = 0;
    for value in values {
        total += value;
    }

    for index, value in 0..4 {
        print(index, value);
    }
    print(total);
}
```

循环变量的绑定只在循环体内有效；如果循环可能执行零次，不能把循环体中的赋值当作
循环后的初始化证明。需要保留外部值时，先声明并在每条可达分支赋值。

### 24.3 函数、匿名函数和闭包

```tl
fn apply(value: int, operation: fn(int) -> int) -> int {
    return operation(value);
}

task main() {
    let offset = 10;
    let add_offset = fn(value: int) {
        return value + offset;
    };
    let result = apply(5, add_offset);
    print(result);
}
```

```tl
fn factorial(value: int) -> int {
    if value <= 1 {
        return 1;
    }
    return value * factorial(value - 1);
}

task main() {
    print(factorial(5));
}
```

闭包捕获的是创建时解析到的绑定身份。下面的两个 `value` 是不同绑定，闭包仍读取
第一个绑定：

```tl
task main() {
    let value = 1;
    let read = fn() { return value; };
    let value = 2;
    print(read());
}
```

### 24.4 数组、映射、字符串和切片

```tl
task main() {
    let numbers = [1, 2, 3, 4];
    numbers[0] = 10;
    let middle = numbers[1..3];
    let last = numbers[-1];

    let scores: {string: int} = {"alice": 10, "bob": 8};
    scores.insert("carol", 9);
    scores["alice"] += 1;

    let word = "Teal";
    let first_character = word[0];
    let suffix = word[1..];
    print(middle, last, scores, first_character, suffix);
}
```

字符串索引按字符位置，`len`/`find` 的计量单位按本节说明执行；对非 ASCII 文本，先
确认需要字符位置还是 UTF-8 字节位置，不要混用两种单位。

### 24.5 类、Trait、枚举和模式

```tl
class Point {
    x: int,
    y: int,
}

impl Point {
    fn move_by(dx: int, dy: int) {
        self.x += dx;
        self.y += dy;
    }

    static fn origin() -> Point {
        return Point { x: 0, y: 0 };
    }
}

task main() {
    let point = Point::origin();
    point.move_by(2, 3);
    print(point.x, point.y);
}
```

```tl
enum Message {
    quit,
    text { value: string },
    move { x: int, y: int },
}

fn describe(message: Message) -> string {
    return match message {
        Message.quit => "quit",
        Message.text { value } => value,
        Message.move { x, y } => "${x},${y}",
        _ => "other",
    };
}

task main() {
    print(describe(Message.text { value: "hello" }));
}
```

模式只解构一层枚举字段。保护条件、组合模式、范围模式以及类/数组解构不属于当前
语言；需要更复杂的判断时，在分支体中使用普通条件表达式。

### 24.6 错误处理和静态值

```tl
fn read_number(text: string) -> int {
    try {
        return int(text);
    } catch "invalid-integer" {
        return 0;
    }
}

task main() {
    let value = read_number("42");
    print(value);
}
```

```tl
static greeting = "hello";
static answer: int = 6 * 7;

task main() {
    print(greeting, answer);
}
```

静态初始化在进入 `main` 前按声明顺序执行；静态初始化中的错误会阻止入口执行。将有
副作用的初始化集中在少数静态声明中，并用 `try/catch` 处理可预期的失败。

### 24.7 模块、导入、重导出和属性

`math.tl`：

```tl
pub fn twice(value: int) -> int {
    return value * 2;
}

pub static unit: int = 1;
```

`main.tl`：

```tl
import "math.tl" as math;

task main() {
    print(math::twice(math::unit));
}
```

选择性重导出只暴露指定成员：

```tl
import "math.tl" as implementation;
pub implementation::twice as double;

task main() {
    print(double(21));
}
```

自定义 Attribute 使用名义 class/enum 实例：

```tl
@attribute
class Route {
    method: string,
    path: string,
}

@Route { method: "GET", path: "/health" }
fn health() {}

task main() {
    let route = reflect::item(fn health).@Route;
    print(route.method, route.path);
}
```

### 24.8 Task、Agent、Graph 和模型调用

纯 CPU `task` 不需要 Agent runtime：

```tl
task add(left: int, right: int) -> int {
    return left + right;
}

task main() {
    let result = add(20, 22).await;
    print(result);
}
```

多个 CPU task 可以按输入顺序收集，也可以查询和取消未完成句柄：

```tl
task main() {
    let first = add(1, 2);
    let second = add(3, 4);
    print(Task::all([first, second]).await);

    let pending = add(10, 20);
    print(pending.status());
    pending.cancel();
}
```

Agent 需要宿主提供模型、认证、工具和执行策略；下面示例展示声明形状，实际模型名、
profile 和权限规则由宿主配置：

```tl
import "std/agent.tl" as ai;

@model("codex", "gpt-5.6-terra")
@options({ model_rounds: 2, max_output_tokens: 256 })
agent Analyst {}

impl Analyst {
    pub task summarize(prompt: string) -> string {
        return self.think(prompt).await;
    }
}

task main() {
    let analyst = Analyst {};
    let answer = analyst.summarize("Summarize the current task").await;
    print(answer);
}
```

模型调用方法必须在 Agent 的 `impl` 中声明；实现通常把提示转交给 `self.think`。

下面的 Agent 示例不调用模型，可以确定性地观察两个实例的状态隔离和同一实例的邮箱顺序：

```tl
agent Counter { value: int }
impl Counter {
    pub task add(delta: int) -> int {
        self.value += delta;
        return self.value;
    }
}

task main() {
    let left = Counter { value: 10 };
    let right = Counter { value: 100 };
    let first = left.add(2);
    let other = right.add(5);
    let second = left.add(3);
    print(Task::all([first, second]).await);
    print(other.await);
    print(left.status());
    left.interrupt();
}
```

命名会话和补充上下文在 Agent 方法内部组装；以下片段放进已导入 `std/agent.tl` 的 Agent task：

```tl
let options = self.session_options(self.session("review"));
self.set_context(options, self.context(["Focus on boundary conditions."]));
options.propose_memory = false;
let result: string = self.think("Review the implementation", options).await;
```

显式记忆调用需要额外导入 `std/memory.tl`，并由宿主安装 MemoryStore：

```tl
let entries = memory::recall(memory::Scope.workspace, "recovery", 8).await;
let candidate = memory::propose(memory::Scope.workspace, "Review recovery behavior").await;
```

候选 ID 不是已验证事实。完整的导入、Agent 声明和调用入口见
[会话与记忆示例](../examples/agent/sessions_and_memory.tl)。

显式知识查询需要导入 `std/knowledge.tl`，并由宿主安装版本化 KnowledgeProvider；普通
`teal run` 安装构建期内嵌的官方知识和本地层，工作台按受信任工作区配置装配同一组层：

```tl
import "std/knowledge.tl" as knowledge;

let hits: knowledge::Entry[] = knowledge::search(knowledge::Scope.official, "class fields", 5).await?;
let entry: knowledge::Entry = knowledge::read(hits[0].id, 8192).await?;
print(entry.body);

let symbols: knowledge::Entry[] = knowledge::lookup("float.sqrt", 5).await?;
```

`Scope.official` 是随版本构建的官方知识；`Scope.user` 是 `<teal_home>/knowledge` 下的用户级
文档；`Scope.project` 是工作区级文档，默认位于 `<workspace>/.teal/knowledge`，只有用户层把该
工作区记为 trusted 时才装载。`scope` 只过滤来源，不授予读取权限：工作区层仍按
`workspace.read` 逐文件授权，模型写工作区笔记时应使用工作区写入工具写到 `.teal/knowledge/`。
知识正文和搜索摘要属于辅助资料，不能替代编译器验证或交付证据。

`spawn_agent`、`send_message`、`followup_task`、`wait_agent`、`ack_messages`、
`list_agents` 和 `interrupt_agent` 是模型运行时提供给模型的工具名，不是脚本可以直接调用
的 Teal 自由函数。源码使用构造表达式、公开 task 方法和任务句柄；动态模型团队示例见
[模型团队](../examples/agent/model_team.tl)。读取消息后要显式确认 cursor；读取与确认分离，
未确认消息在恢复后仍然可见。

脚本可以通过 `std/user.tl` 请求输入，结果显式区分回答和跳过：

```tl
import "std/user.tl" as user;

task workflow() -> Result<none, Error> {
    let answer_result = user::ask("What should the report cover?").await;
    match answer_result {
        Result.ok { value: answer } => match answer {
            user::Answer.text { value } => { print(value); }
            user::Answer.skipped => { print("Skipped"); }
        }
        Result.err { error } => { return Result::err(error); }
    }
    let choice_result = user::choose("Choose a format", [
        user::Choice { id: "brief", label: "Short summary" },
        user::Choice { id: "full", label: "Full report" },
    ]).await;
    match choice_result {
        Result.ok { value: choice } => match choice {
            user::ChoiceAnswer.selected { id } => { print(id); }
            user::ChoiceAnswer.skipped => { print("Skipped"); }
        }
        Result.err { error } => { return Result::err(error); }
    }
    return Result::ok(none);
}

task main() { workflow().await; }
```

回答只提供数据，不授予权限。非交互运行的持久恢复流程见
[用户交互示例](../examples/agent/user_interaction.tl)。

Graph 委派、消息和持久 run 只有在宿主启用 Agent runtime 时可执行；没有对应扩展时，
编译器会报告缺少 Extension 或宿主操作实现，而不会静默降级成普通函数。

### 24.9 Host、文件系统和动态库（宿主相关）

安全 Host 调用必须使用宿主注册的限定名和能力授权：

```tl
import "std/fs.tl" as fs;

task main() {
    let exists = fs::exists("config.tl").await;
    if exists {
        print(fs::read("config.tl").await);
    }
}
```

`dylib` 只声明 C ABI，不授予加载库或调用符号的权限：

```tl
dylib "libexample" {
    int add(int, int);
}

task main() {
    print(add(20, 22));
}
```

执行前仍需 `dylib` RuntimeProfile、平台 ABI 兼容和统一 dylib 权限规则。把库路径、
符号名和参数类型当成外部契约处理，并在目标平台上单独做集成验证。

### 24.10 CLI、格式化、LSP 和 DAP

```powershell
# 只检查，不执行和落盘
cargo run -p teal -- compile examples/hello.tl

# 执行并向 Teal 程序传递参数
cargo run -p teal -- run examples/hello.tl -- first second

# 检查格式或就地写回
cargo run -p teal-fmt -- --check examples/hello.tl
cargo run -p teal-fmt -- examples/hello.tl

# 生成器和协议工具通过 stdio 工作
cargo run -p teal-lsp
cargo run -p teal-dap
```

LSP/DAP 不改变语言语义：编辑器诊断、补全、悬停和调试执行仍受同一套类型、Extension、
Host 和权限边界约束。DAP 的断点必须落在可生成源码范围内；无法映射的内部指令不会被
伪装成源码位置。

### 24.11 有意失败的边界样例

以下片段用于快速确认拒绝规则，不能作为合法程序运行：

```tl
// 普通赋值不能改变绑定类型
fn main() {
    let number = 1;
    number = "text";
}
```

```tl
// `while` 和多返回值不是当前语法
while true {}
fn pair() -> (int, int) { return 1, 2; }
```

```tl
import "std/fs.tl" as fs;

// Host、动态库和 Agent 的声明本身不绕过授权
task main() {
    print(fs::read("outside-workspace.txt").await);
}
```

出现失败时，先判断它属于解析、编译、加载、运行时还是宿主阶段，再根据第 22 章的诊断
规则定位；不要只根据一条人类可读错误文本推断所有失败类型。

### 24.12 与仓库示例的覆盖索引

本手册的示例用于解释规则；仓库中的可执行夹具用于验证规则。需要逐项核对当前实现时，
按 `examples/coverage.json` 的 `id` 搜索对应条目，再打开其 `examples` 路径。该索引覆盖
核心语言、标准包（`std/*` 与 `git`、`http`、`oauth`、`feishu`）、Host、dylib、Agent、
记忆、用户交互和工作台扩展，并为每项标注
`run`、`check`、`harness` 或 `external` 验证方式。索引中的 `harness` 和 `external` 只
表示需要宿主或外部服务，不能解读为普通 `cargo test` 已经执行。

推荐的核对顺序是：

1. 先用本手册确定源码形状和静态约束；
2. 再运行或检查 `examples/coverage.json` 指向的最小夹具；
3. 最后按宿主操作授权、RuntimeProfile 和认证条件复现外部行为。

这样可以区分“语言规则未覆盖”“示例未覆盖”和“宿主环境未提供”三种问题。

---

本手册章节组织参考了 Lua 参考手册的“基本概念、语言、库、独立程序”分层方式，并借鉴
Rust Reference 对语法记号、语义边界和实现限制的明确区分；Teal 语义均来自本仓库当前
实现和测试，不能由 Lua 或 Rust 的同名构造类推。
