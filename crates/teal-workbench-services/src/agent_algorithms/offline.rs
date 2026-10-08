//! 工作台离线受限模式的能力判定。

use serde::{Deserialize, Serialize};

/// 工作台的连接模式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectivityMode {
    /// 允许经过正常认证和授权的网络能力。
    #[default]
    Online,
    /// 禁止远程模型、网络请求和不可约束的外部副作用。
    Offline,
}

/// 需要由连接模式判定的一类能力。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectivityCapability {
    /// 只读本地文件、历史或录制品。
    LocalRead,
    /// 由工作台明确选择的本地编译、格式化或测试进程。
    LocalVerification,
    /// 经过配置校验且只能连接字面量环回地址的模型。
    LocalModel,
    /// 远程或无法证明为本地的模型请求。
    RemoteModel,
    /// HTTP、浏览器或远程遥测请求。
    Network,
    /// 任意扩展子进程或其他无法约束网络行为的外部副作用。
    ExternalSideEffect,
}

/// 离线模式拒绝能力时的稳定错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("OfflineBlocked: {capability:?} is unavailable in offline mode")]
pub struct OfflineBlocked {
    /// 被拒绝的能力。
    pub capability: ConnectivityCapability,
}

impl ConnectivityMode {
    /// 检查能力能否在当前连接模式下执行。
    ///
    /// # Errors
    /// 离线模式下请求远程模型、网络或不可约束的外部副作用时返回 `OfflineBlocked`。
    pub const fn require(self, capability: ConnectivityCapability) -> Result<(), OfflineBlocked> {
        if matches!(
            (self, capability),
            (
                Self::Offline,
                ConnectivityCapability::RemoteModel
                    | ConnectivityCapability::Network
                    | ConnectivityCapability::ExternalSideEffect
            )
        ) {
            Err(OfflineBlocked { capability })
        } else {
            Ok(())
        }
    }
}

/// 返回离线本地模型可以看到的受限工作台工具。
///
/// 这里只允许工作区读取、经既有审批或 ChangeSet 路径执行的修改，以及不执行目标程序的
/// Teal 格式与编译检查。网络、桌面、MCP、任意进程和程序执行均不在白名单中。
#[must_use]
pub fn allows_offline_model_tool(name: &str) -> bool {
    matches!(
        name,
        "workspace.read_file"
            | "workspace.list"
            | "workspace.stat"
            | "workspace.read_range"
            | "workspace.search"
            | "workspace.write_file"
            | "workspace.replace_file"
            | "workspace.apply_patch"
            | "workspace.apply_changeset"
            | "workspace.create_dir"
            | "workspace.move"
            | "workspace.delete"
            | "workspace.diagnostics"
            | "knowledge.search"
            | "knowledge.read"
            | "knowledge.lookup"
            | "teal.fmt_check"
            | "teal.check"
    )
}
