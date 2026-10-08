//! 命令目录、命令行解析和跨层动作描述。
//!
//! 注册表只定义名称、别名和帮助，不执行文件或界面操作；根应用解析结果后再分发。
//! 外部命令尾部与普通参数的转义规则不同，不能先按通用空白拆分再还原。

use std::{collections::BTreeMap, path::PathBuf};

use thiserror::Error;

use crate::{Direction, KeymapPreset, PanelId};

/// 已解析的命令行输入。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandInvocation {
    /// 已注册的命令名；直接启动子进程时为 `!`。
    pub name: String,

    /// 采用 shell 风格引号的参数。
    pub args: Vec<String>,

    /// 名称是否使用了末尾 Vim 风格 `!` 修饰符。
    pub bang: bool,

    /// 移除可选前导冒号后的原始输入。
    pub raw: String,
}

/// 用于补全和帮助的静态命令元数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    /// 规范名称。
    pub name: String,

    /// 接受的别名。
    pub aliases: Vec<String>,

    /// 单行帮助。
    pub description: String,

    /// 该命令适用的工作流窗口；`Window` 表示全部窗口。
    pub scope: CommandScope,
}

/// 命令适用的工作流窗口。
///
/// 作用域只影响 `:` 命令行的补全与可执行判定，不改变命令本身的语义；`Window`
/// 表示所有窗口都可用，其余取值与 `WindowKind::as_str` 的稳定名称一致。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandScope {
    /// 全部窗口都可使用。
    Window,

    /// 仅在指定类型的工作流窗口中补齐与执行。
    Kind(&'static str),
}

impl CommandScope {
    /// 返回该命令是否适用于给定窗口类型名。
    ///
    /// `window` 为 `None` 时视为没有可归属窗口，只有全局命令可用。
    #[must_use]
    pub fn allows(self, window: Option<&str>) -> bool {
        match self {
            Self::Window => true,
            Self::Kind(kind) => window == Some(kind),
        }
    }
}

/// 返回命令的默认作用域。
///
/// 只把确实属于单一工作流、且不该在其他窗口出现的命令收窄到具体窗口；创建窗口或
/// 能从任一窗口启动的命令保持全局可用，避免命令在需要的窗口里消失。
fn default_scope(name: &str) -> CommandScope {
    match name {
        // 暂停会话的推进只能在 Debug 窗口里进行；`:debug` 本身仍可从任一窗口启动会话。
        "debug-out" | "debug-next" | "debug-step" | "debug-stop" | "debug-frame" | "debug-eval"
        | "break" | "break-clear" => CommandScope::Kind("debug"),
        // `teal.test` 会打开 Tests 窗口，因此从任一窗口都可用；结果操作仍属该窗口。
        "test-select" | "test-jump" | "test-rerun" => CommandScope::Kind("tests"),
        _ => CommandScope::Window,
    }
}

/// 无效的命令行引号或缺失名称。
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum CommandParseError {
    /// 命令行不包含命令。
    #[error("command is empty")]
    Empty,

    /// 引号在输入结束前未闭合。
    #[error("unterminated {0} quote")]
    UnterminatedQuote(char),

    /// 末尾转义符后没有字符。
    #[error("command line ends with an incomplete escape")]
    TrailingEscape,
}

/// 确定性的命令目录与解析器。
#[derive(Clone, Debug, Default)]
pub struct CommandRegistry {
    commands: BTreeMap<String, CommandSpec>,
    aliases: BTreeMap<String, String>,
}

impl CommandRegistry {
    /// 创建标准 Teal 工作台命令目录。
    #[must_use]
    pub fn standard() -> Self {
        let mut registry = Self::default();
        for (name, aliases, description) in [
            (
                "extension",
                &[][..],
                "Load, reload, unload or inspect a local Teal extension",
            ),
            ("mode", &[][..], "Select a registered working mode"),
            ("open", &["edit", "e"][..], "Open a file or directory"),
            ("write", &["w"][..], "Save the focused document"),
            ("quit", &["q"][..], "Close the focused panel or application"),
            (
                "window",
                &["win"][..],
                "List, create, close or switch workbench windows",
            ),
            (
                "script",
                &[][..],
                "List, scaffold, open or reconfigure workspace scripts",
            ),
            (
                "scripts",
                &[][..],
                "Open the workspace script manifest view",
            ),
            (
                "delivery",
                &[][..],
                "Open the delivery record view for the active task flow",
            ),
            (
                "packages",
                &["pkgs"][..],
                "List the first-party package catalog or verify the runtime lock",
            ),
            (
                "diagnostics",
                &["diag"][..],
                "Open the aggregated diagnostics view",
            ),
            (
                "test",
                &["tests"][..],
                "Compile the focused script and run its registered `@test` functions",
            ),
            (
                "test-jump",
                &[][..],
                "Open the source location of a test in the Tests window",
            ),
            (
                "test-select",
                &[][..],
                "Select a test in the Tests window and refresh its output panel",
            ),
            (
                "test-rerun",
                &[][..],
                "Re-run one test from the Tests window",
            ),
            (
                "break",
                &["breakpoint"][..],
                "Replace the debugger breakpoint set with the given source lines",
            ),
            (
                "debug-frame",
                &[][..],
                "Select the debugger frame shown in the Debug window",
            ),
            (
                "debug-eval",
                &[][..],
                "Evaluate a side-effect-free expression in the paused debugger frame",
            ),
            (
                "evidence",
                &[][..],
                "Re-check the accepted delivery receipt against the workspace",
            ),
            (
                "diff",
                &[][..],
                "Compare the focused buffer with its file on disk",
            ),
            ("split", &["sp"][..], "Split the focused panel horizontally"),
            ("vsplit", &["vs"][..], "Split the focused panel vertically"),
            (
                "focus",
                &[][..],
                "Focus the panel in the specified direction",
            ),
            ("buffer", &["b"][..], "Select an open buffer"),
            ("explorer", &["ex"][..], "Open the workspace file browser"),
            (
                "file-new",
                &[][..],
                "Create a file in the focused file browser",
            ),
            (
                "dir-new",
                &[][..],
                "Create a directory in the focused file browser",
            ),
            ("rename", &[][..], "Rename the selected file browser entry"),
            (
                "copy",
                &[][..],
                "Copy selected file browser entries to a directory",
            ),
            (
                "move",
                &[][..],
                "Move selected file browser entries to a directory",
            ),
            (
                "trash",
                &[][..],
                "Move selected file browser entries to the trash",
            ),
            (
                "delete",
                &[][..],
                "Use ! to permanently delete selected items",
            ),
            ("search", &[][..], "Search the focused buffer"),
            ("grep", &[][..], "Search workspace files"),
            ("browse", &[][..], "Open URL in text browser"),
            ("browse-head", &[][..], "Get HTTP response headers"),
            ("browse-find", &[][..], "Find text on current web page"),
            ("browse-refresh", &[][..], "Refresh the current web page"),
            (
                "preview",
                &["md"][..],
                "Render a Markdown file or the focused buffer",
            ),
            (
                "agent",
                &[][..],
                "Run Chat or use task <goal> for a verified Teal delivery",
            ),
            (
                "agent-insert",
                &[][..],
                "Open a numbered Agent code block in an editable buffer",
            ),
            (
                "agent-write",
                &[][..],
                "Write a numbered Agent code block into a workspace file",
            ),
            (
                "agent-run",
                &[][..],
                "Run a numbered Teal code block and return its result to the Agent buffer",
            ),
            (
                "agent-graph",
                &[][..],
                "Load a persisted Agent Graph run into the Agent buffer",
            ),
            (
                "agent-branch",
                &[][..],
                "Create, list, save or switch workspace-bound conversation branches",
            ),
            (
                "agent-merge",
                &[][..],
                "Open a three-way merge result in an editable buffer",
            ),
            (
                "agent-merge-verify",
                &[][..],
                "Revalidate and review the current three-way merge resolution",
            ),
            (
                "agent-imports",
                &[][..],
                "Show direct Teal module imports in the Agent buffer",
            ),
            (
                "agent-callers",
                &[][..],
                "Show callers of a Teal symbol in the Agent buffer",
            ),
            (
                "agent-callees",
                &[][..],
                "Show callees of a Teal symbol in the Agent buffer",
            ),
            (
                "agent-symbols",
                &[][..],
                "Show declared symbols in a Teal source file",
            ),
            (
                "agent-occurrences",
                &[][..],
                "Show declaration and reference spans for a Teal symbol",
            ),
            (
                "agent-visible",
                &[][..],
                "Show Teal symbols visible at a byte position",
            ),
            (
                "agent-type",
                &[][..],
                "Show Teal symbol and type information at a byte position",
            ),
            (
                "agent-references",
                &[][..],
                "Show all Teal symbol references in a source file",
            ),
            (
                "agent-tokens",
                &[][..],
                "Show the Teal token stream for a source file",
            ),
            (
                "diagnose",
                &[][..],
                "Run a focused Rust check or test and show structured diagnostics",
            ),
            (
                "agent-context",
                &[][..],
                "Open the source location for a numbered diagnostic context",
            ),
            (
                "agent-view",
                &[][..],
                "Show Teal AST, type, data-flow, or effect analysis",
            ),
            (
                "task-commit",
                &[][..],
                "Commit the verified artifacts of the current completed Task",
            ),
            (
                "task-evidence",
                &[][..],
                "Export a redacted evidence record for the current Task",
            ),
            (
                "agent-answer",
                &[][..],
                "Answer a persistent workspace run question; execution requires explicit resume",
            ),
            (
                "agents",
                &[][..],
                "Select a configured Teal Agent application",
            ),
            (
                "roles",
                &[][..],
                "List roles exported by the active Teal Agent application",
            ),
            (
                "agent-config",
                &[][..],
                "Open user or workspace Agent configuration",
            ),
            (
                "memory",
                &[][..],
                "View, approve, or delete Agent long-term memory",
            ),
            (
                "knowledge",
                &[][..],
                "Inspect Teal knowledge, refresh project sources, or read a topic",
            ),
            (
                "goal",
                &[][..],
                "Start, continue, inspect or stop an explicit persistent goal",
            ),
            (
                "permissions",
                &[][..],
                "View, enable, or revoke workspace development permissions",
            ),
            (
                "runs",
                &[][..],
                "Inspect and manage persistent Agent runs in the current workspace",
            ),
            (
                "agent-setup",
                &[][..],
                "Explicitly select provider, profile and model and generate Teal configuration",
            ),
            (
                "runtime",
                &[][..],
                "Check the project-locked Teal runtime environment",
            ),
            ("debug", &[][..], "Start an embedded Teal debugging session"),
            (
                "debug-continue",
                &[][..],
                "Continue the current Teal debugging session",
            ),
            (
                "debug-next",
                &[][..],
                "Step over the current Teal debug location",
            ),
            (
                "debug-step",
                &[][..],
                "Step into the next Teal debug location",
            ),
            ("debug-out", &[][..], "Step out of the current Teal frame"),
            (
                "debug-stop",
                &[][..],
                "Terminate current Teal debugging session",
            ),
            ("complete", &[][..], "Request and accept inline completion"),
            ("lsp", &[][..], "Start a language server preset"),
            ("panel", &[][..], "Open a registered Teal panel"),
            ("lsp-complete", &[][..], "Request completion"),
            ("lsp-hover", &[][..], "Show symbol documentation"),
            ("lsp-definition", &[][..], "Jump to definition"),
            ("lsp-references", &[][..], "Find references"),
            ("lsp-rename", &[][..], "Rename symbol"),
            ("lsp-format", &[][..], "Format document"),
            ("lsp-code-action", &[][..], "Select code action"),
            ("lsp-semantic-tokens", &[][..], "Refresh semantic tokens"),
            ("source", &[][..], "Execute Teal script"),
            ("reload", &[][..], "Reload configuration or focused content"),
            ("messages", &[][..], "Show structured message history"),
            ("session", &[][..], "Save, restore or view session"),
            ("keymap", &[][..], "Switch the default keymap preset"),
        ] {
            registry.register(CommandSpec {
                name: name.to_owned(),
                aliases: aliases.iter().map(ToString::to_string).collect(),
                description: description.to_owned(),
                scope: default_scope(name),
            });
        }
        registry
    }

    /// 注册或替换命令及其别名。
    pub fn register(&mut self, spec: CommandSpec) {
        for alias in &spec.aliases {
            self.aliases.insert(alias.clone(), spec.name.clone());
        }
        self.commands.insert(spec.name.clone(), spec);
    }

    /// 在拥有命令的脚本重新加载时注销命令及其别名。
    pub fn unregister(&mut self, name: &str) {
        self.commands.remove(name);
        self.aliases.retain(|_, target| target != name);
    }

    /// 解析规范命令或别名。
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<&CommandSpec> {
        self.commands.get(name).or_else(|| {
            self.aliases
                .get(name)
                .and_then(|canonical| self.commands.get(canonical))
        })
    }

    /// 按稳定顺序返回前缀匹配的名称和别名。
    #[must_use]
    pub fn complete(&self, prefix: &str) -> Vec<&str> {
        self.complete_scoped(prefix, None)
    }

    /// 按稳定顺序返回前缀匹配且适用于给定窗口类型的名称和别名。
    ///
    /// `window` 为 `Some(kind)` 时使用 `WindowKind::as_str` 的稳定名称过滤。
    #[must_use]
    pub fn complete_scoped(&self, prefix: &str, window: Option<&str>) -> Vec<&str> {
        self.commands
            .keys()
            .chain(self.aliases.keys())
            .filter(|name| name.starts_with(prefix))
            .filter(|name| {
                let spec = self.resolve(name);
                spec.is_none_or(|spec| spec.scope.allows(window))
            })
            .map(String::as_str)
            .collect()
    }

    /// 返回已注册命令是否适用于给定窗口类型。
    #[must_use]
    pub fn allows(&self, name: &str, window: Option<&str>) -> bool {
        self.resolve(name)
            .is_none_or(|spec| spec.scope.allows(window))
    }

    /// 解析一条命令行，但不在目录中解析其名称。
    pub fn parse(input: &str) -> Result<CommandInvocation, CommandParseError> {
        let raw = input
            .trim()
            .strip_prefix(':')
            .unwrap_or(input.trim())
            .trim();
        if raw.is_empty() {
            return Err(CommandParseError::Empty);
        }
        if let Some(shell) = raw.strip_prefix('!') {
            return Ok(CommandInvocation {
                name: "!".to_owned(),
                args: vec![shell.trim_start().to_owned()],
                bang: false,
                raw: raw.to_owned(),
            });
        }
        let mut words = Vec::new();
        let mut current = String::new();
        let mut quote = None;
        let mut escaped = false;
        for character in raw.chars() {
            if escaped {
                current.push(character);
                escaped = false;
                continue;
            }
            if character == '\\' && quote != Some('\'') {
                escaped = true;
                continue;
            }
            if let Some(active) = quote {
                if character == active {
                    quote = None;
                } else {
                    current.push(character);
                }
            } else if matches!(character, '\'' | '"') {
                quote = Some(character);
            } else if character.is_whitespace() {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            } else {
                current.push(character);
            }
        }
        if escaped {
            return Err(CommandParseError::TrailingEscape);
        }
        if let Some(quote) = quote {
            return Err(CommandParseError::UnterminatedQuote(quote));
        }
        if !current.is_empty() {
            words.push(current);
        }
        let mut name = words.first().cloned().ok_or(CommandParseError::Empty)?;
        let bang = name.ends_with('!');
        if bang {
            name.pop();
        }
        Ok(CommandInvocation {
            name,
            args: words.into_iter().skip(1).collect(),
            bang,
            raw: raw.to_owned(),
        })
    }
}

/// 由命令、按键、脚本和工具产生的类型化应用修改。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppCommand {
    /// 在聚焦面板或新面板中打开路径。
    Open(PathBuf),

    /// 保存聚焦文档。
    Save,

    /// 将聚焦文档保存到显式路径。
    SaveAs(PathBuf),

    /// 关闭面板，可选择强制关闭脏面板。
    Close {
        /// 要关闭的面板。
        panel: PanelId,

        /// 忽略脏缓冲区保护。
        force: bool,
    },

    /// 退出应用，可选择强制关闭脏缓冲区。
    Quit {
        /// 忽略脏缓冲区保护。
        force: bool,
    },

    /// 拆分聚焦面板。
    Split {
        /// 使用并排窗格而非上下堆叠窗格。
        vertical: bool,
    },

    /// 按几何方向移动焦点。
    Focus(Direction),

    /// 新建窗口并聚焦。
    WindowNew(crate::WindowKind),

    /// 新建浮动窗口并聚焦。
    WindowFloat(crate::WindowKind),

    /// 在停靠与浮动之间切换活动窗口。
    WindowToggleFloat,

    /// 调整活动浮动窗口的尺寸。
    WindowResize {
        /// 调整方向；水平改宽度，垂直改高度。
        direction: Direction,

        /// 放大为 `true`，缩小为 `false`。
        grow: bool,
    },

    /// 关闭活动窗口。
    WindowClose,

    /// 按顺序切换窗口。
    WindowCycle {
        /// 是否切换到上一个窗口。
        backwards: bool,
    },

    /// 按从 1 开始的序号选择窗口。
    WindowSelect(usize),

    /// 在指定路径打开文件浏览器。
    Explorer(PathBuf),

    /// 搜索聚焦文档。
    Search {
        /// 正则表达式或字面量搜索输入。
        pattern: String,

        /// 朝缓冲区开头搜索。
        backwards: bool,
    },

    /// 搜索工作区文件。
    Grep(String),

    /// 在文本浏览器中打开 URL。
    Browse(String),

    /// 使聚焦的文本浏览器后退。
    BrowseBack,

    /// 使聚焦的文本浏览器前进。
    BrowseForward,

    /// 在外部打开聚焦文本浏览器的 URL。
    BrowseExternal,

    /// 在操作系统浏览器中打开显式 URL。
    BrowseExternalUrl(String),

    /// 向智能体发送提示。
    Agent(String),

    /// 执行一个 Teal 源文件。
    Source(PathBuf),

    /// 启动用户输入的子进程。
    Run(String),

    /// 更改随附的默认键位映射。
    SetKeymap(KeymapPreset),

    /// 显示消息历史。
    Messages,

    /// 持久化当前会话。
    SaveSession,

    /// 重新加载配置与内容。
    Reload,

    /// 执行一条工作台命令行输入（不含前导 `:`）。
    Command(String),
}

#[cfg(test)]
mod tests {
    use super::CommandRegistry;

    #[test]
    fn parses_quotes_bang_and_literal_shell_tail() {
        let parsed =
            CommandRegistry::parse(":open \"a b.rs\"").expect("quoted command should parse");
        assert_eq!(parsed.name, "open");
        assert_eq!(parsed.args, ["a b.rs"]);

        let parsed = CommandRegistry::parse("q!").expect("bang command should parse");
        assert_eq!(parsed.name, "q");
        assert!(parsed.bang);

        let parsed = CommandRegistry::parse(":! cargo test -- --nocapture")
            .expect("shell command should parse");
        assert_eq!(parsed.name, "!");
        assert_eq!(parsed.args, ["cargo test -- --nocapture"]);
    }

    #[test]
    fn completion_and_execution_respect_the_window_scope() {
        use super::CommandScope;

        let registry = CommandRegistry::standard();

        // `:break` 只属于 Debug 窗口；Agent 窗口既不补全也不允许执行。
        assert!(
            !registry
                .complete_scoped("break", Some("agent"))
                .contains(&"break")
        );
        assert!(!registry.allows("break", Some("agent")));
        assert!(registry.allows("break", Some("debug")));
        assert!(
            registry
                .complete_scoped("break", Some("debug"))
                .contains(&"break")
        );

        // 启动会话的 `:debug` 与运行测试的 `:test` 从任一窗口都可用。
        assert!(registry.allows("debug", Some("agent")));
        assert!(registry.allows("test", Some("agent")));
        assert!(registry.allows("roles", Some("agent")));

        // 结果操作仍限定在 Tests 窗口。
        assert!(!registry.allows("test-jump", Some("editor")));
        assert!(registry.allows("test-jump", Some("tests")));

        // 没有活动窗口时只保留全局命令。
        assert!(registry.allows("open", None));
        assert!(!registry.allows("break", None));
        assert!(matches!(
            registry.resolve("break").map(|spec| spec.scope),
            Some(CommandScope::Kind("debug"))
        ));
    }
}
