# Teal 工作台分发资源

本目录保存工作台扩展、配置模板、命令补全与 Windows 安装脚本。语言 std、Builtin 制品及
`teal-fmt/lsp/dap` 等语言工具由同一 Git revision 的 teal-lang checkout 提供。

- `packages`：工作台的 agents、bench、git、http、oauth、feishu 包；每个 `.tl` 是一个 module。
  std 的权威源码和生成工具留在语言仓库。发行合并 catalog 格式为 6，package manifest 为 7。
- `config`：安装层默认声明；类别为 config、model、bench、runtime、compiler、lint、agent、observability。
  安装层只读，不保存秘密或用户批准；工作区声明经 Security 的信任规则过滤。
- `templates`：对应类别的用户模板，复制到 `TEAL_HOME` 或受信任工作区的 `.teal`。
- `completions`：PowerShell、Bash 与 Zsh 命令补全。
- `installs`：Windows 源码安装与卸载，兼容 PowerShell 5.1 和 PowerShell 7。

## 构建与安装

语言 checkout 必须干净，且 HEAD 与工作台 Cargo.lock 的语言提交完全一致；xtask 在构建前验证。
工作台自己的 `teal` 入口保留。完整发行包含 6 个工作台产品和 8 个语言工具，
`runtime/manifest.json` 记录所有资源的大小与 SHA-256。官方知识由二进制内嵌，不另存文本副本。

```powershell
cargo xtask dist --language-root D:\teal-lang-release
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\runtime\installs\install.ps1 -LanguageRoot D:\teal-lang-release
```

安装器从自身所在工作台源码树构建，并传递必填 `-LanguageRoot`。它不下载 Rust，
不自动推送或切换语言版本。当前工作台未提交修改会进入本地构建；发布前应提交并固定输入。
`-InstallDir <path>` 覆盖默认 `%LOCALAPPDATA%\Programs\Teal`，`-NoPath` 禁止修改 PATH。

安装先校验 14 个可执行文件与资源清单，再在同级 staging 目录准备文件并原子切换。
`.teal-install.json` 记录受管理条目；陌生目录、重解析点、危险路径或额外文件均拒绝覆盖。
安装目录不能覆盖任何一份源码树。

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\Teal\runtime\installs\uninstall.ps1"
```

卸载默认保留 `TEAL_HOME`；只有显式 `-Purge` 才删除用户数据。自定义安装位置须传入同一
`-InstallDir`。程序目录与用户数据目录重叠或指向危险路径时，卸载拒绝执行。

语言与标准包说明见 [语言仓库](https://github.com/dev-chee/teal-lang/tree/main/runtime/packages/std)，
当前兼容边界与验证记录见 [兼容矩阵](../docs/TEAL-LANG-COMPAT.md)。
