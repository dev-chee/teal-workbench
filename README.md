# teal-workbench 逻辑工作区

这里记录未来 `teal-workbench` 仓库的边界。当前实现仍位于本仓库的 `crates/`、根命令
和工作台扩展 package；本文件与 [`workspace.toml`](workspace.toml) 是拆仓前的可校验清单。

工作台提供语言工具、编辑器、工作区服务、交互前端、发行工具和应用扩展包。它只通过
发布版本或固定 Git tag 依赖 `teal-lang`，不使用跨仓 `path` 依赖。

## 根包

此 Cargo package 保留发行版 	eal 根命令；工作台 crate 成员在完成迁移后加入本 workspace。

