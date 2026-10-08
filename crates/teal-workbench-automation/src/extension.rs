//! 本地扩展清单、入口闭包和实例事务。执行账本仍由 Teal Runtime 拥有。

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use crate::{WorkbenchAutomationRuntime, PreparedScript, ScriptKind};
mod sdk;
pub use sdk::{ExtensionCall, ExtensionCallOutput};
pub use sdk::{EVIDENCE_SIGNATURES, register_evidence_preflight};

/// 工作台扩展协议版本；不改变语言标准库或字节码版本。
pub const BENCH_EXTENSION_API_VERSION: u16 = 1;
/// SDK 源码版本，属于扩展信任闭包。
pub const BENCH_SCRIPT_SDK_VERSION: &str = "1";
const MAX_STATE_BYTES: usize = 8 * 1024 * 1024;

/// 一个本地扩展包。所有入口相对清单目录解析，不能逃出包目录。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkbenchPluginManifest {
    /// 稳定扩展名称，只接受 ASCII 字母、数字、下划线和连字符。
    pub id: String,
    /// 扩展作者的版本标识。
    pub version: String,
    /// 期望的工作台扩展 API 版本。
    pub api_version: u16,
    /// 请求能力上限；不是授权。
    #[serde(default)]
    pub operations: BTreeSet<String>,
    /// 命令名到入口文件。
    #[serde(default)]
    pub commands: BTreeMap<String, String>,
    /// Hook 名到入口文件。
    #[serde(default)]
    pub hooks: BTreeMap<String, String>,
    /// 面板名到入口文件。
    #[serde(default)]
    pub panels: BTreeMap<String, String>,
    /// 模式名到事件入口文件。
    #[serde(default)]
    pub modes: BTreeMap<String, String>,
    /// 可启动的后台任务名到入口文件。
    #[serde(default)]
    pub tasks: BTreeMap<String, String>,
    /// 每项任务可见的模型工具；未声明时不提供模型工具。
    #[serde(default)]
    pub task_tools: BTreeMap<String, BTreeSet<String>>,
    /// 限制某任务的写入路径，值为保存 JSON 路径数组的实例状态键。
    #[serde(default)]
    pub task_write_state: BTreeMap<String, String>,
    /// 实例进入工作模式后是否占有工作区开发权。
    #[serde(default)]
    pub exclusive_workspace: bool,
    /// 无秘密的默认扩展配置；用户配置按键覆盖。
    #[serde(default)]
    pub config: BTreeMap<String, String>,
}

/// 已校验的包和完整编译快照；安装时不执行入口。
pub struct WorkbenchPluginPackage {
    /// 清单内容。
    pub manifest: WorkbenchPluginManifest,
    /// 规范包目录。
    pub root: PathBuf,
    /// 清单、SDK 和全部入口导入闭包的内容摘要。
    pub fingerprint: String,
    /// 精确能力集合，交给宿主批准。
    pub operations: BTreeSet<String>,
    entries: BTreeMap<String, PreparedScript>,
    inputs: BTreeMap<PathBuf, String>,
}

/// 清单、源码、状态或事务不满足扩展契约。
#[derive(Debug, thiserror::Error)]
pub enum ExtensionError {
    /// 文件读取或原子写入失败。
    #[error("Extension I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// 输入无效或当前快照已过期。
    #[error("Invalid extension: {0}")]
    Invalid(String),
    /// 持久结构不合法。
    #[error("Extension state encoding failed: {0}")]
    Json(#[from] serde_json::Error),
}

impl WorkbenchPluginPackage {
    /// 返回清单、入口和用户导入闭包的规范输入摘要。
    #[must_use]
    pub fn source_inputs(&self) -> &BTreeMap<PathBuf, String> {
        &self.inputs
    }

    /// 读取清单并编译所有入口；任何失败都不会留下部分注册。
    ///
    /// # Errors
    /// 清单版本、名称、路径、源码或能力请求无效时返回错误。
    pub fn load(root: &Path, runtime: &WorkbenchAutomationRuntime) -> Result<Self, ExtensionError> {
        let root = root.canonicalize()?;
        let bytes = fs::read(root.join("extension.toml"))?;
        if bytes.len() > 256 * 1024 {
            return invalid("manifest exceeds 256 KiB");
        }
        let manifest: WorkbenchPluginManifest = toml::from_str(
            std::str::from_utf8(&bytes).map_err(|e| ExtensionError::Invalid(e.to_string()))?,
        )
        .map_err(|e| ExtensionError::Invalid(e.to_string()))?;
        if !valid_name(&manifest.id)
            || manifest.version.trim().is_empty()
            || manifest.api_version != BENCH_EXTENSION_API_VERSION
        {
            return invalid("invalid identity or unsupported API version");
        }
        let mut entries = BTreeMap::new();
        let mut inputs = BTreeMap::from([(root.join("extension.toml"), sha256(&bytes))]);
        let mut operations = BTreeSet::new();
        let mut digest = Sha256::new();
        hash_part(&mut digest, b"teal-bench-extension/v1");
        hash_part(&mut digest, BENCH_SCRIPT_SDK_VERSION.as_bytes());
        hash_part(&mut digest, root.to_string_lossy().as_bytes());
        hash_part(&mut digest, &bytes);
        for (kind, contributions) in [
            ("command", &manifest.commands),
            ("hook", &manifest.hooks),
            ("panel", &manifest.panels),
            ("mode", &manifest.modes),
            ("task", &manifest.tasks),
        ] {
            for (name, relative) in contributions {
                if (kind != "hook" && !valid_name(name)) || (kind == "hook" && !valid_hook(name)) {
                    return invalid(format!("invalid {kind} name {name}"));
                }
                if kind == "mode" && matches!(name.as_str(), "chat" | "task") {
                    return invalid("native modes are reserved");
                }
                let target = root.join(relative).canonicalize()?;
                if !target.starts_with(&root) || !target.is_file() {
                    return invalid("entry escapes the extension directory or is not a file");
                }
                let prepared = runtime
                    .prepare(&target, ScriptKind::WorkspaceConfig)
                    .map_err(|e| ExtensionError::Invalid(e.to_string()))?;
                for (path, hash) in prepared.source_inputs() {
                    if inputs.get(path).is_some_and(|saved| saved != hash) {
                        return invalid("source changed during package compilation");
                    }
                    inputs.insert(path.clone(), hash.clone());
                }
                let hash = prepared
                    .source_inputs()
                    .get(&target)
                    .ok_or_else(|| {
                        ExtensionError::Invalid(
                            "compiled entry is missing its source fingerprint".into(),
                        )
                    })?
                    .clone();
                inputs.insert(root.join(relative), hash);
                operations.extend(prepared.capabilities().iter().cloned());
                hash_part(&mut digest, kind.as_bytes());
                hash_part(&mut digest, name.as_bytes());
                hash_part(&mut digest, prepared.trust_contents());
                entries.insert(format!("{kind}:{name}"), prepared);
            }
        }
        if !operations.is_subset(&manifest.operations) {
            return invalid(format!(
                "undeclared operations: {:?}",
                operations
                    .difference(&manifest.operations)
                    .collect::<Vec<_>>()
            ));
        }
        // 模型可调用脚本没有直接引用的已登记工具；这些能力也必须明确请求并向用户展示。
        // 静态程序需求已核对是子集，实际授权仍在工作台批准清单之后才建立。
        operations = manifest.operations.clone();
        let package = Self {
            manifest,
            root,
            fingerprint: format!("{:x}", digest.finalize()),
            operations,
            entries,
            inputs,
        };
        if !package.is_current()? {
            return invalid("source changed during package preparation");
        }
        Ok(package)
    }

    /// 返回已编译入口；只允许清单声明的类型和名称。
    #[must_use]
    pub fn entry(&self, kind: &str, name: &str) -> Option<&PreparedScript> {
        self.entries.get(&format!("{kind}:{name}"))
    }
    /// 比对清单和所有文件输入，不重复编译；导入别名被替换也会失效。
    ///
    /// # Errors
    /// 任一输入无法读取时返回错误。
    pub fn is_current(&self) -> Result<bool, ExtensionError> {
        for (path, expected) in &self.inputs {
            if sha256(&fs::read(path)?) != *expected {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// 捕获后不再从当前焦点重新读取的扩展事件。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionEvent {
    /// 稳定投递身份，重复投递不重复提交。
    pub id: String,
    /// 输入、模式、面板、Hook 或任务结果类别。
    pub kind: String,
    /// 本次入口名称。
    pub name: String,
    /// 无凭证的事件字段。
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
}

/// 阶段任务到 RuntimeStore 的引用，不复制运行或副作用账本。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionTaskRef {
    /// 发起请求的贡献入口，完成事件返回该入口。
    #[serde(default)]
    pub callback: Option<ExtensionCallback>,
    /// 由实例、事件和序号构成的稳定启动键。
    pub key: String,
    /// 清单中的任务名。
    pub task: String,
    /// 原始输入，恢复时不得替换。
    pub input: String,
    /// 此阶段必须仍然有效的业务审批身份。
    #[serde(default)]
    pub approval_ids: Vec<String>,
    /// 请求时冻结的业务上下文；后续面板输入不改变已准备运行的参数。
    #[serde(default)]
    pub state: BTreeMap<String, String>,
    /// RuntimeStore 运行身份；准备后、执行前写入。
    pub run_id: Option<String>,
    /// 提交到扩展的终态结果是否已确认。
    pub delivered: bool,
    /// 已提交但尚未应用的控制意图，恢复时仍需显式继续。
    #[serde(default)]
    pub control: Option<ExtensionTaskControl>,
}

/// 扩展只能控制自己拥有的后台任务。
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionTaskControl {
    /// 请求取消；未知操作不会因取消而变成可重放。
    Cancel,
    /// 从原运行身份显式继续或投递原终态。
    Resume,
}

/// 一个不携带执行权限的扩展事件回调地址。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionCallback {
    /// command、hook、panel 或 mode。
    pub kind: String,
    /// 清单贡献名称。
    pub name: String,
}

/// 绑定内容的普通文件证据。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReceipt {
    /// 工作区相对路径。
    pub path: String,
    /// 文件内容 SHA-256；明确不存在的文件使用 absent，以便验证删除及重新创建。
    pub sha256: String,
}

/// 宿主签发的业务审批；信息回答不能创建此记录。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalReceipt {
    /// 用户决定应返回的贡献入口。
    #[serde(default)]
    pub callback: Option<ExtensionCallback>,
    /// 请求身份。
    pub id: String,
    /// 供用户评审的提示。
    pub prompt: String,
    /// 批准绑定的文件版本。
    pub artifacts: Vec<ArtifactReceipt>,
    /// 具体操作，不是权限通配符。
    pub operation: String,
    /// 目标环境，普通制品审批为空。
    pub environment: String,
    /// None 表示等待，false 表示拒绝。
    pub approved: Option<bool>,
}

/// 结构化命令的确定执行证据，不从显示文本解析成功。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandReceipt {
    /// 稳定动作身份。
    pub id: String,
    /// 实际程序和参数的摘要。
    pub command_digest: String,
    /// 真实退出码；信号或超时不假定为成功。
    pub exit_code: Option<i32>,
    /// 完整捕获输出的摘要。
    pub output_digest: String,
    /// 本次检查的输入文件版本。
    pub artifacts: Vec<ArtifactReceipt>,
}

/// 一个扩展在当前工作区的业务状态。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionInstance {
    /// 业务状态格式版本。
    pub version: u16,
    /// 扩展身份。
    pub extension: String,
    /// 实例身份。
    pub id: String,
    /// 固定的清单及源码指纹。
    pub fingerprint: String,
    /// 单调事务修订。
    pub revision: u64,
    /// 扩展私有业务数据；不含宿主收据。
    pub values: BTreeMap<String, String>,
    /// 已提交事件身份及其摘要，防止同身份不同载荷。
    pub events: BTreeMap<String, String>,
    /// 已接受的后台任务请求。
    pub tasks: BTreeMap<String, ExtensionTaskRef>,
    /// 宿主拥有的业务审批记录。
    pub approvals: BTreeMap<String, ApprovalReceipt>,
}

impl ExtensionInstance {
    /// 建立尚无事件的新实例。
    #[must_use]
    pub fn new(extension: &str, id: &str, fingerprint: &str) -> Self {
        Self {
            version: 1,
            extension: extension.into(),
            id: id.into(),
            fingerprint: fingerprint.into(),
            revision: 0,
            values: BTreeMap::new(),
            events: BTreeMap::new(),
            tasks: BTreeMap::new(),
            approvals: BTreeMap::new(),
        }
    }

    /// 检查事件是否已经提交；同 ID 不同载荷被拒绝。
    ///
    /// # Errors
    /// 身份为空、载荷过大或重复身份内容不同时返回错误。
    pub fn contains_event(&self, event: &ExtensionEvent) -> Result<bool, ExtensionError> {
        if event.id.is_empty() || event.id.len() > 256 {
            return invalid("invalid event identity");
        }
        let bytes = serde_json::to_vec(event)?;
        if bytes.len() > 1024 * 1024 {
            return invalid("event exceeds 1 MiB");
        }
        match self.events.get(&event.id) {
            Some(saved) if saved != &sha256(&bytes) => {
                invalid("event identity has different content")
            }
            Some(_) => Ok(true),
            None => Ok(false),
        }
    }

    /// 原子保存业务数据及事件确认。外部副作用不属于本事务。
    ///
    /// # Errors
    /// 并发修订、来源变化、重复事件冲突或文件写入失败时返回错误。
    pub fn commit(
        &mut self,
        path: &Path,
        event: &ExtensionEvent,
        values: BTreeMap<String, String>,
        tasks: Vec<ExtensionTaskRef>,
        approvals: Vec<ApprovalReceipt>,
    ) -> Result<(), ExtensionError> {
        if self.contains_event(event)? {
            return Ok(());
        }
        if path.exists() {
            let saved = Self::load(path)?;
            if saved.id != self.id
                || saved.fingerprint != self.fingerprint
                || saved.revision != self.revision
            {
                return invalid("extension transaction is stale");
            }
        }
        let mut next = self.clone();
        next.values = values;
        for task in tasks {
            if next.tasks.insert(task.key.clone(), task).is_some() {
                return invalid("duplicate task identity");
            }
        }
        for receipt in approvals {
            if next.approvals.insert(receipt.id.clone(), receipt).is_some() {
                return invalid("duplicate approval identity");
            }
        }
        next.events
            .insert(event.id.clone(), sha256(&serde_json::to_vec(event)?));
        next.revision += 1;
        next.save(path)?;
        *self = next;
        Ok(())
    }

    /// 读取已保存状态，拒绝未知版本或超大输入。
    ///
    /// # Errors
    /// 状态缺失、损坏或版本不同返回错误，不自动重置。
    pub fn load(path: &Path) -> Result<Self, ExtensionError> {
        let bytes = fs::read(path)?;
        if bytes.len() > MAX_STATE_BYTES {
            return invalid("state exceeds 8 MiB");
        }
        let value: Self = serde_json::from_slice(&bytes)?;
        if value.version != 1 || !valid_name(&value.extension) || value.id.is_empty() {
            return invalid("invalid state identity or version");
        }
        Ok(value)
    }

    /// 原子保存宿主更新后的运行引用或审批；调用者必须串行持有实例。
    ///
    /// # Errors
    /// 状态超大或持久化失败时返回错误。
    pub fn save(&self, path: &Path) -> Result<(), ExtensionError> {
        let bytes = serde_json::to_vec_pretty(self)?;
        if bytes.len() > MAX_STATE_BYTES {
            return invalid("state exceeds 8 MiB");
        }
        let parent = path
            .parent()
            .ok_or_else(|| ExtensionError::Invalid("missing state parent".into()))?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file_mut().sync_all()?;
        temporary.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
}

/// 对拥有所有权的内容求 SHA-256，供制品和事务比对。
#[must_use]
pub fn sha256(bytes: &[u8]) -> String {
    teal_utils::sha256_hex(bytes)
}

fn hash_part(digest: &mut Sha256, part: &[u8]) {
    digest.update((part.len() as u64).to_le_bytes());
    digest.update(part);
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn valid_hook(name: &str) -> bool {
    matches!(
        name,
        "app.started"
            | "app.stopping"
            | "workspace.opened"
            | "buffer.opened"
            | "buffer.changed"
            | "buffer.saved"
            | "buffer.closed"
            | "panel.focused"
            | "diagnostics.changed"
            | "lsp.started"
            | "agent.completed"
            | "session.restored"
    )
}
fn invalid<T>(message: impl Into<String>) -> Result<T, ExtensionError> {
    Err(ExtensionError::Invalid(message.into()))
}
