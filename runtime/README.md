# Teal 分发资源

此目录集中保存随 Teal CLI、LSP 和 DAP 一同分发的运行时资源。发行工具会原样复制整个目录并生成 SHA-256 清单；这里不是 Cargo 包，也不包含 Rust API。

内置扩展包、分发与维护工具的整体位置见 [teal-workspace 实现细节](../crates/teal-workspace/README.md#工作区分发与维护工具)。

## 目录

- `packages`：`std`、`bench`、`git`、`http`、`oauth` 与 `feishu` 内置扩展包的唯一权威源码目录；每个 `.tl` 是一个 module，`std/builtins.tli` 是由编译器声明表渲染的 Builtin 接口制品（说明为英文），构建阶段分别生成预编译 Module IR 与 Builtin Interface IR。发行目录存在时必须与内嵌制品完全一致。
- `config`：安装层默认值，按配置类别拆分为 `config.toml`、`model.toml`（provider/model 与认证 profile）、`bench.toml`（工作台声明）与 `runtime.toml`（语言运行时）。可执行文件旁的 `runtime/config` 就是最低优先级层；覆盖链为用户 `TEAL_HOME` 与工作区 `<workspace>/.teal`，合并与存盘规则由 `teal-config` 负责。安装层只读、不得包含秘密、权限、信任或本地知识层根，因此 `config.toml` 只声明知识开关与配额的默认值。
- `templates`：给用户复制的完整配置模板，四个类别各一份（`config.toml`、`model.toml`、`bench.toml`、`runtime.toml`）。复制到 `TEAL_HOME/<类别>.toml`，或复制到受信任工作区的 `.teal/<类别>.toml`；工作区层只能保留各自的行为声明（`knowledge.workspace_root`、`default`/`providers`、`bench` 的 Agent/MCP/命令/Hook/面板/插件），安装层不允许这些键，所以模板不能直接放在 `config/` 里。
- `completions`：`teal` 的 PowerShell、Bash 和 Zsh 命令补全脚本。PowerShell 执行 `runtime/completions/teal.ps1` 注册补全；Bash/Zsh 分别 source `teal.bash` 或 `_teal`。
- `installs`：Windows 源码安装与卸载脚本。`install.ps1` 固定从当前脚本所属的 Teal 源码根构建，校验本次生成的分发清单后默认安装到 `%LOCALAPPDATA%\Programs\Teal`；`uninstall.ps1` 只操作带有效所有权标记且没有额外条目的安装目录。两者均兼容 Windows PowerShell 5.1 与 PowerShell 7。
- `assets`：未来由 Teal 工具消费的默认资产；没有真实资产前不创建占位文件。

## Windows 源码安装

在源码树中执行：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\runtime\installs\install.ps1
```

安装器不接受其它源码路径，也不下载或安装 Rust 工具链。它以脚本位置向上两级得到源码根，在该目录执行 `cargo run --locked --package teal-xtask -- dist`，所以当前工作树中的未提交修改同样进入安装结果。默认安装将 `%LOCALAPPDATA%\Programs\Teal` 加入 `PATH`。`-InstallDir <path>` 覆盖默认安装目录，`-NoPath` 禁止修改当前用户和当前进程的 `PATH`。

安装器先验证十二个可执行文件与 `runtime/manifest.json` 的全部路径、大小和 SHA-256，再在目标同级目录中准备 staging，并通过目录切换完成更新。安装根的 `.teal-install.json` 记录受管理条目；已有陌生目录、重解析点、危险路径或额外文件会使安装和卸载安全失败。

默认卸载保留 `TEAL_HOME`：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\Teal\runtime\installs\uninstall.ps1"
```

自定义安装位置时传入相同的 `-InstallDir`。只有显式 `-Purge` 才会按 Teal 的发现规则删除 `TEAL_HOME`：优先使用环境变量，否则依次使用 `%USERPROFILE%\.teal` 与 `%HOME%\.teal`。程序目录与用户数据目录重叠、指向文件系统根、用户主目录或重解析点时，卸载器拒绝删除。

官方知识没有随发行分发的文本副本：`teal-knowledge` 在构建期生成内嵌制品，身份与当前 Teal 二进制
绑定，`cargo xtask dist` 不再物化 `runtime/knowledge`，发行清单里也没有知识文件。

标准包的接口、授权和示例见 [标准包](../docs/TEAL-LANG-MANUAL.md#标准包)。

内置扩展包固定采用以下结构（其他包沿用 bench 的清单和 src 结构）：

```text
packages/
├─ catalog.toml
├─ catalog-core.toml
├─ catalog-workbench.toml
├─ std/
│  ├─ package.toml
│  ├─ builtins.tli
│  └─ src/**/*.tl
└─ bench/
   ├─ package.toml
   └─ src/**/*.tl
```

`catalog.toml` 只列完整发行 package；`catalog-core.toml` 与 `catalog-workbench.toml` 显式声明未来两个仓库的 package 分层，必须互斥并覆盖完整 catalog。每个 `package.toml` 精确列出版本、依赖、语言 edition、全部 module、每个 module 的责任域和可选接口。一个 `.tl` 就是一个 module，多个 module 由 package 聚合；只有 `std` 可以把根目录的 `builtins.tli` 声明为 Builtin 接口。清单外源码、缺失文件、符号链接、越界路径和非规范名称均拒绝。当前 catalog 格式为 4、package manifest 格式为 5；`[modules]` 表将 module 路径直接映射到责任域，domain 或 edition 变化会改变 package 与 import closure 摘要。

`std/builtins.tli` 是唯一生成制品：内容由 `teal-compiler` 的 Rust 声明表经 `cargo xtask builtins generate` 渲染，编辑器与知识库读它，编译器自身不解析该文件。`cargo xtask builtins verify` 逐字节比对制品与声明表，发行目录仍必须与二进制内嵌快照完全一致。

语言示例、无效输入和仅解析样例不属于分发资源，继续由仓库根目录的 `examples` 维护。
