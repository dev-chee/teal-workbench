//! 工作台配置类别：`bench.toml`。
//!
//! 该文件保存工作台声明式配置——命名 Agent、MCP 服务器、命令、Hook、面板与本地插件。
//! 层优先级、合并、来源解析与存盘全部由 `teal-config` 负责；本模块只声明字段与领域校验，
//! 并由工作台在启动时把声明安装到自动化运行时。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use teal_config::{ALLOW_ALL, ConfigCategory, ConfigDocument, ConfigFile, ConfigLayer};

/// 一个由 Teal 源码定义全部行为的 Agent 应用入口。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAppConfig {
    /// 相对于声明配置层的根 Teal 源码，必须包含 `task main`。
    pub script: PathBuf,
}

/// 工作台 MCP 服务器声明。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    /// `http` 或 `stdio`。
    pub transport: String,
    /// URL 或程序路径。
    pub target: String,
    /// stdio 参数。
    #[serde(default)]
    pub arguments: Vec<String>,
}

/// 一个声明式 Teal 自动化脚本引用。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptConfig {
    /// 相对于声明配置层的脚本路径。
    pub script: PathBuf,
}

/// 本地工作台插件引用；配置不能包含秘密值，路径相对所属配置层。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkbenchPluginConfig {
    /// 包含 extension.toml 的目录。
    pub path: PathBuf,
    /// 无秘密的扩展业务配置覆盖。
    #[serde(default)]
    pub config: BTreeMap<String, String>,
}

/// 工作台的声明式总体配置。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchDocument {
    /// 本地工作台插件；加载配置不等于批准插件操作。
    #[serde(default)]
    pub plugins: BTreeMap<String, WorkbenchPluginConfig>,
    /// `prompt`、`always` 或 `never`。
    pub session_restore: Option<String>,
    /// 命名源码 Agent 应用；Bench 只选择入口并提供交互端口。
    #[serde(default)]
    pub agent_apps: BTreeMap<String, AgentAppConfig>,
    /// MCP 服务器。
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
    /// 命名命令脚本。
    #[serde(default)]
    pub commands: BTreeMap<String, ScriptConfig>,
    /// 按事件名分组的 Hook 脚本。
    #[serde(default)]
    pub hooks: BTreeMap<String, Vec<ScriptConfig>>,
    /// 命名面板脚本。
    #[serde(default)]
    pub panels: BTreeMap<String, ScriptConfig>,
}

impl ConfigDocument for BenchDocument {
    fn validate(&self) -> Result<(), String> {
        if let Some(policy) = self.session_restore.as_deref()
            && !matches!(policy, "prompt" | "always" | "never")
        {
            return Err("session_restore must be prompt, always, or never".into());
        }
        for (name, app) in &self.agent_apps {
            if name.trim().is_empty() || app.script.as_os_str().is_empty() {
                return Err("Agent application declaration is invalid".into());
            }
        }
        for server in self.mcp_servers.values() {
            if server.target.trim().is_empty()
                || !matches!(server.transport.as_str(), "http" | "stdio")
                || (server.transport == "http" && !server.arguments.is_empty())
            {
                return Err("Bench MCP declaration is invalid".into());
            }
        }
        Ok(())
    }
}

impl ConfigCategory for BenchDocument {
    const FILE: ConfigFile = ConfigFile::Bench;

    type Document = Self;

    fn allowed_paths(layer: ConfigLayer) -> &'static [&'static str] {
        match layer {
            // 安装层只提供界面默认值，不能携带脚本、插件、MCP 或 Agent 声明。
            ConfigLayer::Install => &["session_restore"],
            ConfigLayer::User => ALLOW_ALL,
            // 受信任工作区只登记本工作区的行为声明。
            ConfigLayer::Workspace => &[
                "session_restore",
                "agent_apps",
                "mcp_servers",
                "commands",
                "hooks",
                "panels",
                "plugins",
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_agent_application_round_trips_without_behavior_fields() {
        let configured: BenchDocument = serde_json::from_value(serde_json::json!({
            "agent_apps": {
                "reviewer": { "script": "agents/reviewer.tl" }
            }
        }))
        .unwrap();
        configured.validate().unwrap();
        assert_eq!(
            configured.agent_apps["reviewer"].script,
            PathBuf::from("agents/reviewer.tl")
        );
        assert_eq!(
            serde_json::from_value::<BenchDocument>(serde_json::to_value(&configured).unwrap())
                .unwrap(),
            configured
        );
    }
}
