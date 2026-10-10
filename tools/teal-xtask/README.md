# teal-xtask

工作台仓库维护入口，通过 `cargo xtask <command>` 调用，不作为产品运行时 API 发布。

- `workspace verify`：使用当前 `cargo metadata --locked --no-deps` 检查 workspace，并核对六个工作台扩展包。
- `rustdoc audit|files|build|verify`：只覆盖当前工作台的 13 个产品 library target；验证前与 Cargo 实际目标比较。
- `dist`：构建九个工作台二进制并组装工作台资源、摘要清单和平台归档。
- `dist --language-root PATH`：额外核对干净语言 checkout 与锁文件提交一致，构建五个语言 sibling binaries，合并语言标准包和工作台扩展 catalog。根 `teal` 派发器始终由工作台提供。
- `test-gate run --spec PATH [--force]`：执行显式 argv 规范，保存日志、报告与前后内容指纹；输入变化时结果为 `STALE`。

`builtins`、`knowledge`、ABI/VM/Host coverage 和语言 rustdoc 归 `teal-lang` 的维护工具所有。本仓库不复制语言实现，也不生成标准包制品。验证和发行边界见 [跨仓兼容矩阵](../../docs/TEAL-LANG-COMPAT.md) 与 [AGENTS.md](../../AGENTS.md)。
