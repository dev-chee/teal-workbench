//! 限制在工作区内的智能体工具与交互式批准通道。
//!
//! 工具定义只描述调度和参数；权限检查仍基于具体参数及解析后的路径。
//! 对已打开文档的修改送回应用队列，工具任务等待完成通知但不直接改写界面状态。

use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{SearchService, run_command_blocking, run_program_blocking};
#[cfg(test)]
use serde_json::json;
use sha2::{Digest, Sha256};
#[cfg(test)]
use teal_agent::{ActionContext, ActionSpec, ApprovalScope, Decision, PermissionProvider};
use teal_agent::{ApprovalResponse, PermissionPrompt, ToolCall, ToolRegistry};
use teal_vm::{
    ExternalCallRecovery, HostCallPreparation, HostError, HostFunctionDefinition, HostResource,
    HostResourceRequest, HostToolCheckpoint, HostToolEffect, HostToolExport, HostToolParameter,
    HostValue, ResourceAccess, RuntimeType,
};
use tokio::sync::{mpsc, oneshot};

mod catalog;
use catalog::{WorkspaceOperation, register_workspace_hosts};

mod source;
pub use source::SourceQuerySnapshot;

mod approval;
mod changeset;
mod headless;
mod media;
mod mutation;
mod plan;
mod requests;
mod toolchain;
mod transactions;

pub use approval::ChannelApproval;
use changeset::ChangeSetInput;
pub use headless::HeadlessWorkspaceTools;
pub use mutation::MutationKind;
use plan::register_plan_host;
pub use requests::{ChangeSetRequest, MutationRequest};
use teal_host::browser::register_browser_hosts;
use teal_host::computer::register_computer_hosts;
use teal_host::web::{WebBackend, register_web_hosts};
#[cfg(test)]
use toolchain::teal_tool_stem;
use toolchain::{
    TaskToolchain, ToolchainPreflightError, preflight_task_toolchain, teal_executable,
    teal_format_executable, teal_tool_arguments, teal_tool_executable,
};

/// 返回与当前工作台同目录的 Teal 主程序。
pub fn distributed_teal_executable() -> Option<PathBuf> {
    teal_executable()
}

/// 返回 Task 启动所需的同目录 Teal 工具链；失败时不应启动模型。
pub fn preflight_task_tools() -> Result<TaskToolchain, ToolchainPreflightError> {
    preflight_task_toolchain()
}

/// 送往终端界面确认浮层的待处理敏感调用。
pub struct ApprovalPrompt {
    /// 发起工具的上下文，仅限当前进程，永不序列化。
    pub observation: crate::tracing_context::Context,

    /// 具体的模型调用。
    pub call: ToolCall,

    /// 共享策略产生的规范资源、允许范围和精确操作绑定。
    pub permission: PermissionPrompt,

    /// 由用户输入完成的类型化范围响应。
    pub response: oneshot::Sender<ApprovalResponse>,
}

/// 根植于单个工作区的文件系统、搜索、进程、网页与桌面工具。
#[derive(Clone)]
pub struct WorkspaceTools {
    diagnostics: teal_lsp_client::DiagnosticStore,
    root: PathBuf,
    checkpoints: PathBuf,
    mutations: mpsc::Sender<MutationRequest>,
    changesets: mpsc::Sender<ChangeSetRequest>,
    open_buffers: Arc<Mutex<BTreeMap<PathBuf, OpenBufferVersion>>>,
    /// 阶段任务的精确写入集合；读取仍受工作区边界控制。
    write_scope: Option<std::collections::BTreeSet<PathBuf>>,
    verification_gate: Arc<Mutex<VerificationGate>>,
    browser: teal_host::browser::BrowserManager,
    computer: Arc<dyn teal_host::computer::ComputerBackend>,
}

#[derive(Default)]
struct VerificationGate {
    active: Option<VerificationGateIdentity>,
    scripts: BTreeMap<PathBuf, VerificationStage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct VerificationGateIdentity {
    task_id: String,
    generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum VerificationStage {
    Formatted {
        source_digest: String,
    },
    Checked {
        source_digest: String,
        tier: String,
        program_digest: String,
    },
    /// 编译检查之后又成功运行了登记测试；测试证据沿用同一份程序摘要。
    Tested {
        source_digest: String,
        tier: String,
        program_digest: String,
    },
}

#[derive(Clone)]
struct OpenBufferVersion {
    id: teal_workbench_core::BufferId,
    revision: u64,
    digest: String,
}

impl OpenBufferVersion {
    fn identity(&self) -> String {
        format!("{}:{}", self.revision, self.digest)
    }
}

/// 为脚本 Host 调用与 Agent 工具共同持有工作区能力和绑定生命周期。
pub struct WorkspaceToolRuntime {
    tools: Arc<WorkspaceTools>,
    host: Mutex<teal::NativeHostRuntime>,
    grant: Arc<teal::HostExecutionGrant>,
}

/// 工作区工具目录的构建或绑定错误。
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceToolError {
    /// 本机宿主的定义目录或服务无法构建。
    #[error(transparent)]
    HostBuild(#[from] teal::NativeHostBuildError),
    /// Host 定义重复或工具元数据无效。
    #[error(transparent)]
    Registry(#[from] teal::HostRegistryError),
    /// Host 工具无法映射到模型目录。
    #[error(transparent)]
    Bridge(#[from] teal::HostToolBridgeError),
    /// 工具 HostRuntime 的互斥状态不可用。
    #[error("Workspace tool runtime is unavailable")]
    Poisoned,
}

impl WorkspaceTools {
    /// 返回已打开缓冲区的身份，供只读运行状态观察；不导出文档正文。
    pub(crate) fn runtime_buffer_version(&self, path: &Path) -> Option<String> {
        self.open_buffer_version(path).ok().flatten()
    }

    /// 创建限制在工作区内的工具执行器。
    pub fn new(
        root: &Path,
        checkpoints: PathBuf,
        mutations: mpsc::Sender<MutationRequest>,
        changesets: mpsc::Sender<ChangeSetRequest>,
        diagnostics: teal_lsp_client::DiagnosticStore,
    ) -> Self {
        Self {
            diagnostics,
            root: root.canonicalize().unwrap_or_else(|_| normalize(root)),
            checkpoints,
            mutations,
            changesets,
            open_buffers: Arc::default(),
            write_scope: None,
            verification_gate: Arc::new(Mutex::new(VerificationGate::default())),
            browser: teal_host::browser::BrowserManager::default()
                .with_audit_context(teal_security::process_context()),
            computer: teal_host::computer::platform_backend_with_audit(
                teal_security::process_context(),
            ),
        }
    }

    fn resolve(&self, input: &str) -> Option<PathBuf> {
        let candidate = Path::new(input);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };

        let normalized = normalize(&joined);
        normalized.starts_with(&self.root).then_some(normalized)
    }

    fn begin_verification(&self, task_id: &str, generation: u64) {
        if let Ok(mut gate) = self.verification_gate.lock() {
            gate.active = Some(VerificationGateIdentity {
                task_id: task_id.to_owned(),
                generation,
            });
            gate.scripts.clear();
        }
    }

    fn end_verification(&self, task_id: &str, generation: u64) {
        if let Ok(mut gate) = self.verification_gate.lock()
            && gate.active.as_ref().is_some_and(|identity| {
                identity.task_id == task_id && identity.generation == generation
            })
        {
            gate.active = None;
            gate.scripts.clear();
        }
    }

    fn verify_before(
        &self,
        name: &str,
        path: &Path,
        tier: &str,
        source_digest: &str,
        program_digest: Option<&str>,
    ) -> Result<(), HostError> {
        let gate = self
            .verification_gate
            .lock()
            .map_err(|_| host_error("Task verification gate is unavailable"))?;
        if gate.active.is_none() {
            return Ok(());
        }
        let stage = gate.scripts.get(path);
        match name {
            "teal.fmt_check" => Ok(()),
            "teal.check" => match stage {
                Some(VerificationStage::Formatted {
                    source_digest: formatted,
                }) if formatted == source_digest => Ok(()),
                Some(VerificationStage::Formatted { .. }) => Err(host_error(
                    "Task verification gate requires teal.fmt_check after the source changed",
                )),
                _ => Err(host_error(
                    "Task verification gate requires a successful teal.fmt_check before teal.check",
                )),
            },
            "teal.test" => match stage {
                Some(VerificationStage::Checked {
                    source_digest: checked_source,
                    tier: checked_profile,
                    program_digest: checked_program,
                }) if checked_source == source_digest
                    && checked_profile == tier
                    && program_digest == Some(checked_program.as_str()) =>
                {
                    Ok(())
                }
                Some(VerificationStage::Checked { .. }) => Err(host_error(
                    "Task verification gate requires teal.fmt_check and teal.check after the source or runtime profile changed",
                )),
                _ => Err(host_error(
                    "Task verification gate requires a successful teal.check before teal.test",
                )),
            },
            "teal.run" => match stage {
                Some(VerificationStage::Checked {
                    source_digest: checked_source,
                    tier: checked_profile,
                    program_digest: checked_program,
                }) if checked_source == source_digest
                    && checked_profile == tier
                    && program_digest == Some(checked_program.as_str()) =>
                {
                    Ok(())
                }
                Some(VerificationStage::Tested {
                    source_digest: checked_source,
                    tier: checked_profile,
                    program_digest: checked_program,
                }) if checked_source == source_digest
                    && checked_profile == tier
                    && program_digest == Some(checked_program.as_str()) =>
                {
                    Ok(())
                }
                Some(VerificationStage::Checked { .. }) => Err(host_error(
                    "Task verification gate requires teal.fmt_check and teal.check after the source or runtime profile changed",
                )),
                Some(VerificationStage::Tested { .. }) => Err(host_error(
                    "Task verification gate requires teal.fmt_check and teal.check after the source or runtime profile changed",
                )),
                _ => Err(host_error(
                    "Task verification gate requires a successful teal.check before teal.run",
                )),
            },
            _ => Ok(()),
        }
    }

    #[allow(clippy::collapsible_match)]
    fn verify_after(
        &self,
        name: &str,
        path: &Path,
        tier: &str,
        source_digest: &str,
        exit_code: i32,
        program_digest: &str,
    ) {
        let Ok(mut gate) = self.verification_gate.lock() else {
            return;
        };
        if gate.active.is_none() {
            return;
        }
        match name {
            "teal.fmt_check" => {
                if exit_code == 0 {
                    gate.scripts.insert(
                        path.to_owned(),
                        VerificationStage::Formatted {
                            source_digest: source_digest.to_owned(),
                        },
                    );
                } else {
                    gate.scripts.remove(path);
                }
            }
            "teal.check" => {
                if exit_code == 0 && !program_digest.is_empty() {
                    gate.scripts.insert(
                        path.to_owned(),
                        VerificationStage::Checked {
                            source_digest: source_digest.to_owned(),
                            tier: tier.to_owned(),
                            program_digest: program_digest.to_owned(),
                        },
                    );
                }
            }
            "teal.test" => {
                if exit_code == 0 && !program_digest.is_empty() {
                    gate.scripts.insert(
                        path.to_owned(),
                        VerificationStage::Tested {
                            source_digest: source_digest.to_owned(),
                            tier: tier.to_owned(),
                            program_digest: program_digest.to_owned(),
                        },
                    );
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::collapsible_if)]
    fn verify_start_failed(&self, name: &str, path: &Path) {
        if let Ok(mut gate) = self.verification_gate.lock()
            && gate.active.is_some()
        {
            if name == "teal.fmt_check" {
                gate.scripts.remove(path);
            }
        }
    }

    fn verify_invalidated(&self, path: &Path) {
        if let Ok(mut gate) = self.verification_gate.lock()
            && gate.active.is_some()
        {
            gate.scripts.remove(path);
        }
    }

    fn prepare(
        &self,
        operation: WorkspaceOperation,
        arguments: &[HostValue],
    ) -> Result<HostCallPreparation, HostError> {
        let name = operation.name();
        let mut arguments = arguments.to_vec();
        let mut resources = Vec::new();
        if operation == WorkspaceOperation::WorkspaceApplyChangeset {
            let raw = string_argument(&arguments, 0, "changes")?;
            let mut input = parse_changeset_input(&raw)?;
            for file in &mut input.files {
                let path = self.resolve_prepared(&file.path, false)?;
                if self
                    .write_scope
                    .as_ref()
                    .is_some_and(|scope| !scope.contains(&path))
                {
                    return Err(host_error("write path is outside the approved task scope"));
                }
                file.path = path.to_string_lossy().into_owned();
                resources.push(HostResourceRequest {
                    access: ResourceAccess::Write,
                    resource: HostResource::Path(path.clone()),
                });
                if let Some(version) = self.open_buffer_version(&path)? {
                    resources.push(HostResourceRequest {
                        access: ResourceAccess::Write,
                        resource: HostResource::Application {
                            namespace: "workspace-buffer".into(),
                            identity: format!("{}@{version}", path.to_string_lossy()),
                        },
                    });
                }
            }
            arguments[0] = HostValue::String(serde_json::to_string(&input).map_err(host_error)?);
            return Ok(HostCallPreparation::new(arguments, resources));
        }
        if operation == WorkspaceOperation::WorkspaceMove {
            for index in 0..=1 {
                let path = self.resolve_prepared(
                    &string_argument(
                        &arguments,
                        index,
                        if index == 0 { "source" } else { "destination" },
                    )?,
                    index == 0,
                )?;
                arguments[index] = HostValue::String(path.to_string_lossy().into_owned());
                resources.push(HostResourceRequest {
                    access: ResourceAccess::Write,
                    resource: HostResource::Path(path),
                });
            }
            return Ok(HostCallPreparation::new(arguments, resources));
        }
        let path_access = match operation {
            WorkspaceOperation::WorkspaceReadFile
            | WorkspaceOperation::WorkspaceList
            | WorkspaceOperation::WorkspaceStat
            | WorkspaceOperation::WorkspaceReadRange
            | WorkspaceOperation::WorkspaceDiagnostics
            | WorkspaceOperation::WorkspaceDiagnosticsJson
            | WorkspaceOperation::TealCheck
            | WorkspaceOperation::TealFmtCheck
            | WorkspaceOperation::TealTest
            | WorkspaceOperation::TealRun => Some(ResourceAccess::Read),
            WorkspaceOperation::WorkspaceWriteFile
            | WorkspaceOperation::WorkspaceReplaceFile
            | WorkspaceOperation::WorkspaceApplyPatch
            | WorkspaceOperation::WorkspaceCreateDir
            | WorkspaceOperation::WorkspaceDelete => Some(ResourceAccess::Write),
            WorkspaceOperation::WorkspaceApplyChangeset
            | WorkspaceOperation::WorkspaceMove
            | WorkspaceOperation::WorkspaceSearch
            | WorkspaceOperation::WorkspaceRun => None,
        };
        if let Some(access) = path_access {
            let input = string_argument(&arguments, 0, "path")?;
            let path = if input.is_empty()
                && matches!(
                    operation,
                    WorkspaceOperation::WorkspaceDiagnostics
                        | WorkspaceOperation::WorkspaceDiagnosticsJson
                ) {
                self.root.clone()
            } else {
                self.resolve_prepared(
                    &input,
                    access == ResourceAccess::Read
                        && operation != WorkspaceOperation::WorkspaceStat,
                )?
            };
            arguments[0] = HostValue::String(path.to_string_lossy().into_owned());
            resources.push(HostResourceRequest {
                access,
                resource: HostResource::Path(path.clone()),
            });
            if matches!(
                operation,
                WorkspaceOperation::WorkspaceWriteFile
                    | WorkspaceOperation::WorkspaceReplaceFile
                    | WorkspaceOperation::WorkspaceApplyPatch
            ) && let Some(version) = self.open_buffer_version(&path)?
            {
                resources.push(HostResourceRequest {
                    access: ResourceAccess::Write,
                    resource: HostResource::Application {
                        namespace: "workspace-buffer".into(),
                        identity: format!("{}@{version}", path.to_string_lossy()),
                    },
                });
            }
        } else if operation == WorkspaceOperation::WorkspaceSearch {
            resources.push(HostResourceRequest {
                access: ResourceAccess::Read,
                resource: HostResource::Path(self.root.clone()),
            });
        }
        if operation == WorkspaceOperation::WorkspaceRun {
            resources.push(HostResourceRequest {
                access: ResourceAccess::Execute,
                resource: HostResource::Path(self.root.clone()),
            });
        } else if matches!(
            operation,
            WorkspaceOperation::TealCheck
                | WorkspaceOperation::TealFmtCheck
                | WorkspaceOperation::TealTest
                | WorkspaceOperation::TealRun
        ) && let Some(executable) =
            teal_tool_executable(name).and_then(|path| path.canonicalize().ok())
        {
            resources.push(HostResourceRequest {
                access: ResourceAccess::Execute,
                resource: HostResource::Path(executable),
            });
        }
        if operation == WorkspaceOperation::TealRun {
            let permission_file =
                self.resolve_prepared(&string_argument(&arguments, 2, "permission_file")?, true)?;
            arguments[2] = HostValue::String(permission_file.to_string_lossy().into_owned());
            resources.push(HostResourceRequest {
                access: ResourceAccess::Read,
                resource: HostResource::Path(permission_file),
            });
        }
        Ok(HostCallPreparation::new(arguments, resources))
    }

    fn resolve_prepared(&self, input: &str, follow_final: bool) -> Result<PathBuf, HostError> {
        let candidate = Path::new(input);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };
        let path = if follow_final {
            joined.canonicalize().map_err(host_error)?
        } else {
            let absolute = std::path::absolute(&joined).map_err(host_error)?;
            let mut missing = vec![
                absolute
                    .file_name()
                    .ok_or_else(|| host_error("path has no final component"))?
                    .to_owned(),
            ];
            let mut cursor = absolute
                .parent()
                .ok_or_else(|| host_error("path has no existing ancestor"))?
                .to_path_buf();
            while !cursor.exists() {
                missing.push(
                    cursor
                        .file_name()
                        .ok_or_else(|| host_error("path has no existing ancestor"))?
                        .to_owned(),
                );
                cursor = cursor
                    .parent()
                    .ok_or_else(|| host_error("path has no existing ancestor"))?
                    .to_path_buf();
            }
            let mut resolved = cursor.canonicalize().map_err(host_error)?;
            for component in missing.into_iter().rev() {
                resolved.push(component);
            }
            resolved
        };
        if !path.starts_with(&self.root) {
            return Err(host_error("path is outside the workspace"));
        }
        Ok(path)
    }

    fn checkpoint(&self, path: &Path) -> Result<Option<String>, HostError> {
        let Ok(contents) = std::fs::read(path) else {
            return Ok(None);
        };

        std::fs::create_dir_all(&self.checkpoints).map_err(host_error)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis());
        let hash = Sha256::digest(path.to_string_lossy().as_bytes());
        let name = format!(
            "{now}-{:02x}{:02x}{:02x}{:02x}.bak",
            hash[0], hash[1], hash[2], hash[3]
        );
        let checkpoint = self.checkpoints.join(name);
        std::fs::write(&checkpoint, contents).map_err(host_error)?;

        Ok(Some(
            serde_json::json!({"backup":checkpoint.to_string_lossy(),"path":path.to_string_lossy()}).to_string(),
        ))
    }

    fn diagnostic_records(&self, path: &str) -> Result<serde_json::Value, HostError> {
        let uri = if path.is_empty() {
            None
        } else {
            Some(
                url::Url::from_file_path(
                    self.resolve(path)
                        .ok_or_else(|| host_error("Diagnostic path is invalid"))?,
                )
                .map_err(|()| host_error("Invalid file URI"))?
                .to_string(),
            )
        };
        let records = self
            .diagnostics
            .snapshot(uri.as_deref())
            .into_iter()
            .filter(|record| {
                url::Url::parse(&record.uri)
                    .ok()
                    .and_then(|url| url.to_file_path().ok())
                    .is_some_and(|path| normalize(&path).starts_with(&self.root))
            })
            .collect::<Vec<_>>();
        serde_json::to_value(records).map_err(host_error)
    }

    fn invoke(
        &self,
        operation: WorkspaceOperation,
        arguments: &[HostValue],
        observation: Option<crate::tracing_context::Context>,
    ) -> Result<HostValue, HostError> {
        let name = operation.name();
        match operation {
            WorkspaceOperation::WorkspaceReadFile => {
                let path = self
                    .resolve(&string_argument(arguments, 0, "path")?)
                    .ok_or_else(|| host_error("path is outside the workspace"))?;
                let bytes = std::fs::read(path).map_err(host_error)?;
                if bytes.len() > 1024 * 1024 {
                    return Err(host_error("File exceeds the 1 MiB tool read limit"));
                }
                Ok(HostValue::String(
                    String::from_utf8(bytes)
                        .map_err(|_| host_error("File is not UTF-8 encoded"))?,
                ))
            }
            WorkspaceOperation::WorkspaceList => {
                let root = self
                    .resolve(&string_argument(arguments, 0, "path")?)
                    .ok_or_else(|| host_error("path is outside the workspace"))?;
                let recursive = bool_argument(arguments, 1, "recursive")?;
                let limit = usize::try_from(int_argument(arguments, 2, "limit")?)
                    .ok()
                    .filter(|limit| (1..=5_000).contains(limit))
                    .ok_or_else(|| host_error("limit must be between 1 and 5000"))?;
                let mut pending = vec![root];
                let mut entries = Vec::new();
                while let Some(directory) = pending.pop() {
                    let mut children = std::fs::read_dir(&directory)
                        .map_err(host_error)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(host_error)?;
                    children.sort_by_key(std::fs::DirEntry::file_name);
                    for entry in children {
                        let path = entry.path();
                        let metadata = std::fs::symlink_metadata(&path).map_err(host_error)?;
                        let relative = path
                            .strip_prefix(&self.root)
                            .map_err(host_error)?
                            .to_string_lossy()
                            .replace('\\', "/");
                        entries.push(HostValue::Map(vec![
                            (
                                HostValue::String("path".into()),
                                HostValue::String(relative),
                            ),
                            (
                                HostValue::String("kind".into()),
                                HostValue::String(path_kind(&metadata).into()),
                            ),
                            (
                                HostValue::String("size".into()),
                                HostValue::String(metadata.len().to_string()),
                            ),
                        ]));
                        if entries.len() >= limit {
                            return Ok(HostValue::Array(entries));
                        }
                        if recursive && metadata.is_dir() && !metadata.file_type().is_symlink() {
                            pending.push(path);
                        }
                    }
                }
                Ok(HostValue::Array(entries))
            }
            WorkspaceOperation::WorkspaceStat => {
                let path = self
                    .resolve(&string_argument(arguments, 0, "path")?)
                    .ok_or_else(|| host_error("path is outside the workspace"))?;
                let metadata = std::fs::symlink_metadata(&path).map_err(host_error)?;
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |value| value.as_millis());
                Ok(HostValue::Map(vec![
                    (
                        HostValue::String("path".into()),
                        HostValue::String(
                            path.strip_prefix(&self.root)
                                .unwrap_or(&path)
                                .to_string_lossy()
                                .replace('\\', "/"),
                        ),
                    ),
                    (
                        HostValue::String("kind".into()),
                        HostValue::String(path_kind(&metadata).into()),
                    ),
                    (
                        HostValue::String("size".into()),
                        HostValue::String(metadata.len().to_string()),
                    ),
                    (
                        HostValue::String("modified_ms".into()),
                        HostValue::String(modified.to_string()),
                    ),
                ]))
            }
            WorkspaceOperation::WorkspaceReadRange => {
                let path = self
                    .resolve(&string_argument(arguments, 0, "path")?)
                    .ok_or_else(|| host_error("path is outside the workspace"))?;
                let start = usize::try_from(int_argument(arguments, 1, "start_line")?)
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or_else(|| host_error("start_line must be positive"))?;
                let count = usize::try_from(int_argument(arguments, 2, "line_count")?)
                    .ok()
                    .filter(|value| (1..=10_000).contains(value))
                    .ok_or_else(|| host_error("line_count must be between 1 and 10000"))?;
                let text = std::fs::read_to_string(path).map_err(host_error)?;
                let value = text
                    .lines()
                    .skip(start - 1)
                    .take(count)
                    .collect::<Vec<_>>()
                    .join("\n");
                if value.len() > 256 * 1024 {
                    return Err(host_error("Requested range exceeds the 256 KiB limit"));
                }
                Ok(HostValue::String(value))
            }
            WorkspaceOperation::WorkspaceSearch => {
                let query = string_argument(arguments, 0, "query")?;
                let matches = SearchService::workspace(
                    &self.root,
                    &query,
                    false,
                    200,
                    &Arc::new(AtomicBool::new(false)),
                )
                .map_err(host_error)?;
                Ok(HostValue::String(
                    matches
                        .iter()
                        .map(|found| {
                            format!(
                                "{}:{}:{}: {}",
                                found
                                    .path
                                    .as_ref()
                                    .map_or("<buffer>".into(), |path| path.to_string_lossy()),
                                found.line,
                                found.column + 1,
                                found.line_text
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ))
            }
            WorkspaceOperation::WorkspaceWriteFile => {
                let path = self
                    .resolve(&string_argument(arguments, 0, "path")?)
                    .ok_or_else(|| host_error("path is outside the workspace"))?;
                if std::fs::symlink_metadata(&path)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
                {
                    return Err(host_error("workspace writes cannot follow symbolic links"));
                }
                let content = string_argument(arguments, 1, "content")?;
                if self
                    .write_scope
                    .as_ref()
                    .is_some_and(|scope| !scope.contains(&path))
                {
                    return Err(host_error("write path is outside the approved task scope"));
                }
                let checkpoint = self.checkpoint(&path)?;
                let expected_content_sha256 = self.mutation_content_digest(&path)?;
                let (response, result) = oneshot::channel();
                self.mutations
                    .blocking_send(MutationRequest {
                        observation: observation
                            .clone()
                            .unwrap_or_else(crate::tracing_context::Context::current),
                        expected_buffer_version: self.mutation_buffer_version(&path)?,
                        path,
                        kind: MutationKind::Write(content),
                        response,
                        expected_content_sha256: Some(expected_content_sha256),
                    })
                    .map_err(|_| host_error("Editor mutation channel is closed"))?;
                result
                    .blocking_recv()
                    .map_err(|_| host_error("Editor mutation response channel is closed"))?
                    .map_err(host_error)?;
                Ok(tool_result("File written", checkpoint))
            }
            WorkspaceOperation::WorkspaceReplaceFile => {
                let path = self
                    .resolve(&string_argument(arguments, 0, "path")?)
                    .ok_or_else(|| host_error("path is outside the workspace"))?;
                if std::fs::symlink_metadata(&path)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
                {
                    return Err(host_error(
                        "workspace replacements cannot follow symbolic links",
                    ));
                }
                if self
                    .write_scope
                    .as_ref()
                    .is_some_and(|scope| !scope.contains(&path))
                {
                    return Err(host_error("write path is outside the approved task scope"));
                }
                let old = string_argument(arguments, 1, "old")?;
                let new = string_argument(arguments, 2, "new")?;
                let checkpoint = self.checkpoint(&path)?;
                let expected_content_sha256 = self.mutation_content_digest(&path)?;
                let (response, result) = oneshot::channel();
                self.mutations
                    .blocking_send(MutationRequest {
                        observation: observation
                            .unwrap_or_else(crate::tracing_context::Context::current),
                        expected_buffer_version: self.mutation_buffer_version(&path)?,
                        path,
                        kind: MutationKind::Replace { old, new },
                        response,
                        expected_content_sha256: Some(expected_content_sha256),
                    })
                    .map_err(|_| host_error("Editor mutation channel is closed"))?;
                result
                    .blocking_recv()
                    .map_err(|_| host_error("Editor mutation response channel is closed"))?
                    .map_err(host_error)?;
                Ok(tool_result("File patch applied", checkpoint))
            }
            WorkspaceOperation::WorkspaceApplyPatch => {
                let path = self
                    .resolve(&string_argument(arguments, 0, "path")?)
                    .ok_or_else(|| host_error("path is outside the workspace"))?;
                let requested = string_argument(arguments, 1, "expected_sha256")?;
                // 模型无法自行计算规范化文本摘要；空值表示由宿主绑定当前内容。
                let expected_sha256 = if requested.is_empty() {
                    self.mutation_content_digest(&path)?
                } else {
                    if requested.len() != 64
                        || !requested.bytes().all(|byte| byte.is_ascii_hexdigit())
                    {
                        return Err(host_error("expected_sha256 must be a SHA-256 digest"));
                    }
                    requested.to_ascii_lowercase()
                };
                let patch = string_argument(arguments, 2, "patch")?;
                if patch.len() > 2 * 1024 * 1024 {
                    return Err(host_error("patch exceeds the 2 MiB limit"));
                }
                let checkpoint = self.checkpoint(&path)?;
                self.mutate(
                    path,
                    MutationKind::ApplyPatch {
                        expected_sha256,
                        patch,
                    },
                    observation,
                )?;
                Ok(tool_result("Unified patch applied", checkpoint))
            }
            WorkspaceOperation::WorkspaceApplyChangeset => {
                let input = parse_changeset_input(&string_argument(arguments, 0, "changes")?)?;
                let mut files = Vec::with_capacity(input.files.len());
                for file in input.files {
                    let absolute = self
                        .resolve(&file.path)
                        .ok_or_else(|| host_error("path is outside the workspace"))?;
                    if std::fs::symlink_metadata(&absolute)
                        .is_ok_and(|metadata| metadata.file_type().is_symlink())
                    {
                        return Err(host_error("workspace writes cannot follow symbolic links"));
                    }
                    if self
                        .write_scope
                        .as_ref()
                        .is_some_and(|scope| !scope.contains(&absolute))
                    {
                        return Err(host_error("write path is outside the approved task scope"));
                    }
                    // 未保存的缓冲区内容不会进入模型上下文，必须在这里失败关闭。
                    self.mutation_buffer_version(&absolute)?;
                    let relative = absolute
                        .strip_prefix(&self.root)
                        .map_err(|_| host_error("ChangeSet path is outside the workspace"))?
                        .to_owned();
                    let original = self.document_text(&absolute)?;
                    let original_sha256 = bind_changeset_digest(
                        file.expected_sha256.as_deref(),
                        &original,
                        "expected_sha256 does not match the current file text",
                    )?;
                    // 先用修改前摘要占位建立 hunk 身份，再用真实应用结果替换修改后摘要。
                    let mut candidate = teal_workbench_core::ChangeSetFile::new(
                        relative,
                        original_sha256.clone(),
                        original_sha256,
                        file.patch,
                    )
                    .map_err(host_error)?;
                    let applied = candidate.apply_all(&original).map_err(host_error)?;
                    candidate.new_sha256 = bind_changeset_digest(
                        file.new_sha256.as_deref(),
                        &applied,
                        "new_sha256 does not match the patch result",
                    )?;
                    files.push(candidate);
                }
                let change_set =
                    teal_workbench_core::ChangeSet::new(None, 0, files).map_err(host_error)?;
                let (response, result) = oneshot::channel();
                self.changesets
                    .blocking_send(ChangeSetRequest {
                        observation: observation
                            .unwrap_or_else(crate::tracing_context::Context::current),
                        change_set,
                        response,
                    })
                    .map_err(|_| host_error("ChangeSet review channel is closed"))?;
                let value = result
                    .blocking_recv()
                    .map_err(|_| host_error("ChangeSet review response channel is closed"))?
                    .map_err(host_error)?;
                Ok(HostValue::String(value))
            }
            WorkspaceOperation::WorkspaceCreateDir => {
                let path = self
                    .resolve(&string_argument(arguments, 0, "path")?)
                    .ok_or_else(|| host_error("path is outside the workspace"))?;
                if path == self.root {
                    return Err(host_error("workspace root cannot be created or replaced"));
                }
                self.mutate(path, MutationKind::CreateDirectory, observation)?;
                Ok(HostValue::String("Directory created".into()))
            }
            WorkspaceOperation::WorkspaceMove => {
                let source = self
                    .resolve(&string_argument(arguments, 0, "source")?)
                    .ok_or_else(|| host_error("source is outside the workspace"))?;
                if source == self.root {
                    return Err(host_error("workspace root cannot be moved"));
                }
                let destination = self
                    .resolve(&string_argument(arguments, 1, "destination")?)
                    .ok_or_else(|| host_error("destination is outside the workspace"))?;
                self.mutate(source, MutationKind::Move { destination }, observation)?;
                Ok(HostValue::String("Workspace path moved".into()))
            }
            WorkspaceOperation::WorkspaceDelete => {
                let path = self
                    .resolve(&string_argument(arguments, 0, "path")?)
                    .ok_or_else(|| host_error("path is outside the workspace"))?;
                if path == self.root {
                    return Err(host_error("workspace root cannot be deleted"));
                }
                self.mutate(path, MutationKind::Delete, observation)?;
                Ok(HostValue::String(
                    "Workspace path moved to recycle bin".into(),
                ))
            }
            WorkspaceOperation::WorkspaceRun => {
                let command = string_argument(arguments, 0, "command")?;
                let output = run_command_blocking(&command, &self.root).map_err(host_error)?;
                Ok(HostValue::String(format!(
                    "exit={}\n{}",
                    output.status, output.output
                )))
            }
            WorkspaceOperation::TealCheck
            | WorkspaceOperation::TealFmtCheck
            | WorkspaceOperation::TealTest
            | WorkspaceOperation::TealRun => {
                let path = self
                    .resolve(&string_argument(arguments, 0, "path")?)
                    .ok_or_else(|| host_error("path is outside the workspace"))?;
                if path.extension().and_then(|extension| extension.to_str()) != Some("tl") {
                    return Err(host_error("Teal tool only accepts .tl source files"));
                }
                let executable = teal_tool_executable(name).ok_or_else(|| {
                    host_error("Cannot find the required Teal tool next to Teal Bench")
                })?;
                let tier = if operation == WorkspaceOperation::TealFmtCheck {
                    "agentic".to_owned()
                } else {
                    string_argument(arguments, 1, "tier")?
                };
                let source_before = std::fs::read(&path).map_err(host_error)?;
                let source_digest = crate::digest(&source_before);
                let program_before = (operation != WorkspaceOperation::TealFmtCheck)
                    .then(|| crate::compile_digest_in_workspace(&self.root, &path, &tier))
                    .transpose()
                    .map_err(host_error)?;
                self.verify_before(
                    name,
                    &path,
                    &tier,
                    &source_digest,
                    program_before.as_deref(),
                )?;
                let mut process_arguments = teal_tool_arguments(name, &path);
                if operation != WorkspaceOperation::TealFmtCheck {
                    process_arguments.push("--tier".into());
                    process_arguments.push(tier.clone());
                }
                if operation == WorkspaceOperation::TealTest {
                    let filter = string_argument(arguments, 2, "filter")?;
                    if !filter.trim().is_empty() {
                        process_arguments.push("--filter".into());
                        process_arguments.push(filter);
                    }
                }
                if operation == WorkspaceOperation::TealRun {
                    process_arguments.push("--permission-file".into());
                    process_arguments.push(string_argument(arguments, 2, "permission_file")?);
                }
                let output = match run_program_blocking(&executable, &process_arguments, &self.root)
                {
                    Ok(output) => output,
                    Err(error) => {
                        self.verify_start_failed(name, &path);
                        return Err(host_error(format!(
                            "Teal tool process start failed: {error}"
                        )));
                    }
                };
                let source_after = std::fs::read(&path).map_err(host_error)?;
                if crate::digest(&source_after) != source_digest {
                    self.verify_invalidated(&path);
                    return Err(host_error("Teal source changed while the tool was running"));
                }
                let program_digest = if let Some(before) = program_before {
                    let after = crate::compile_digest_in_workspace(&self.root, &path, &tier)
                        .map_err(host_error)?;
                    if before != after {
                        self.verify_invalidated(&path);
                        return Err(host_error(
                            "Teal program changed while the tool was running",
                        ));
                    }
                    before
                } else {
                    String::new()
                };
                self.verify_after(
                    name,
                    &path,
                    &tier,
                    &source_digest,
                    output.status,
                    &program_digest,
                );
                Ok(toolchain_result(
                    &path,
                    &tier,
                    source_digest,
                    program_digest,
                    output.status,
                    output.output,
                ))
            }

            WorkspaceOperation::WorkspaceDiagnostics => {
                let path = string_argument(arguments, 0, "path")?;
                json_to_host(self.diagnostic_records(&path)?)
            }
            WorkspaceOperation::WorkspaceDiagnosticsJson => {
                let path = string_argument(arguments, 0, "path")?;
                Ok(HostValue::String(
                    serde_json::to_string(&self.diagnostic_records(&path)?).map_err(host_error)?,
                ))
            }
        }
    }

    fn mutate(
        &self,
        path: PathBuf,
        kind: MutationKind,
        observation: Option<crate::tracing_context::Context>,
    ) -> Result<(), HostError> {
        let expected_content_sha256 = match &kind {
            MutationKind::Write(_)
            | MutationKind::WriteNew { .. }
            | MutationKind::Replace { .. }
            | MutationKind::ApplyPatch { .. } => Some(self.mutation_content_digest(&path)?),
            MutationKind::CreateDirectory | MutationKind::Move { .. } | MutationKind::Delete => {
                None
            }
        };
        let (response, result) = oneshot::channel();
        self.mutations
            .blocking_send(MutationRequest {
                observation: observation.unwrap_or_else(crate::tracing_context::Context::current),
                expected_buffer_version: self.mutation_buffer_version(&path)?,
                path,
                kind,
                response,
                expected_content_sha256,
            })
            .map_err(|_| host_error("Editor mutation channel is closed"))?;
        result
            .blocking_recv()
            .map_err(|_| host_error("Editor mutation response channel is closed"))?
            .map_err(host_error)
    }

    fn open_buffer_version(&self, path: &Path) -> Result<Option<String>, HostError> {
        self.open_buffers
            .lock()
            .map_err(|_| host_error("Open buffer version registry is unavailable"))
            .map(|versions| versions.get(path).map(OpenBufferVersion::identity))
    }

    /// 打开文档必须仍与磁盘基线一致；否则 Agent 没有看到未保存内容，不能安全修改。
    fn mutation_buffer_version(&self, path: &Path) -> Result<Option<String>, HostError> {
        let version = self
            .open_buffers
            .lock()
            .map_err(|_| host_error("Open buffer version registry is unavailable"))?
            .get(path)
            .cloned();
        let Some(version) = version else {
            return Ok(None);
        };
        let disk_text = if path.exists() {
            teal_workbench_core::Document::open(path)
                .map_err(host_error)?
                .to_string()
        } else {
            String::new()
        };
        if crate::digest(disk_text.as_bytes()) != version.digest {
            return Err(host_error(
                "Open buffer has unsaved changes and cannot be modified by an Agent tool",
            ));
        }
        Ok(Some(version.identity()))
    }

    fn mutation_content_digest(&self, path: &Path) -> Result<String, HostError> {
        let text = self.document_text(path)?;
        Ok(crate::digest(text.as_bytes()))
    }

    /// 读取事务提交时使用的规范文本；不存在的路径按空文件处理，供新建文件绑定摘要。
    fn document_text(&self, path: &Path) -> Result<String, HostError> {
        if path.exists() {
            Ok(teal_workbench_core::Document::open(path)
                .map_err(host_error)?
                .to_string())
        } else {
            Ok(String::new())
        }
    }

    fn synchronize_buffers(
        &self,
        buffers: &teal_workbench_core::BufferStore,
    ) -> Result<(), WorkspaceToolError> {
        let mut versions = self
            .open_buffers
            .lock()
            .map_err(|_| WorkspaceToolError::Poisoned)?;
        let mut current = BTreeMap::new();
        for buffer in buffers.iter() {
            let Some(path) = buffer.path() else {
                continue;
            };
            let path = path.canonicalize().unwrap_or_else(|_| normalize(&path));
            let version = versions
                .get(&path)
                .filter(|version| {
                    version.id == buffer.id() && version.revision == buffer.revision()
                })
                .cloned()
                .unwrap_or_else(|| OpenBufferVersion {
                    id: buffer.id(),
                    revision: buffer.revision(),
                    digest: crate::digest(buffer.to_string().as_bytes()),
                });
            current.insert(path, version);
        }
        *versions = current;
        Ok(())
    }
}

const TOOL_NAMES: [&str; 48] = [
    "workspace.read_file",
    "workspace.read_image",
    "workspace.list",
    "workspace.stat",
    "workspace.read_range",
    "workspace.search",
    "workspace.write_file",
    "workspace.replace_file",
    "workspace.apply_patch",
    "workspace.apply_changeset",
    "workspace.create_dir",
    "workspace.move",
    "workspace.delete",
    "workspace.run",
    "workspace.diagnostics",
    "teal.check",
    "teal.fmt_check",
    "teal.test",
    "teal.run",
    "web.fetch",
    "process.exec",
    "process.start",
    "process.status",
    "process.wait",
    "process.terminate",
    "terminal.start",
    "terminal.read",
    "terminal.write",
    "terminal.wait",
    "terminal.resize",
    "terminal.close",
    "browser.start",
    "browser.open",
    "browser.allow_network",
    "browser.snapshot",
    "browser.screenshot",
    "browser.click",
    "browser.fill",
    "browser.close",
    "computer.list_windows",
    "computer.snapshot",
    "computer.invoke",
    "computer.set_value",
    "computer.raw_click",
    "computer.raw_keys",
    "update_plan",
    "record_proposals",
    "adopt_proposal",
];

const TOOL_CAPABILITIES: [&str; 13] = [
    "editor.read",
    "editor.write",
    "process.spawn",
    "network.http",
    "process.execute",
    "process.control",
    "browser.inspect",
    "browser.navigate",
    "browser.interact",
    "computer.inspect",
    "computer.interact",
    "computer.raw-input",
    "agent.plan",
];

impl WorkspaceToolRuntime {
    /// 返回与模型工具目录绑定的当前工作区能力上限。
    ///
    /// 调用方仍需安装权限提供器；取得目录与上限不会批准单次操作。
    #[must_use]
    pub fn agent_grant(&self) -> Arc<teal::HostExecutionGrant> {
        Arc::clone(&self.grant)
    }

    /// 返回当前工具运行时共享的打开缓冲区版本，不读取正文。
    pub(crate) fn runtime_buffer_version(&self, path: &Path) -> Option<String> {
        self.tools.runtime_buffer_version(path)
    }

    /// 创建由 Agent HostRuntime 持有、同时可复制到脚本注册表的工作区能力。
    pub fn new(
        root: &Path,
        checkpoints: PathBuf,
        mutations: mpsc::Sender<MutationRequest>,
        changesets: mpsc::Sender<ChangeSetRequest>,
        diagnostics: teal_lsp_client::DiagnosticStore,
    ) -> Result<Self, WorkspaceToolError> {
        let tools = Arc::new(WorkspaceTools::new(
            root,
            checkpoints,
            mutations,
            changesets,
            diagnostics,
        ));
        Self::from_tools(tools)
    }

    fn from_tools(tools: Arc<WorkspaceTools>) -> Result<Self, WorkspaceToolError> {
        let mut registry = teal::HostRegistry::standard();
        teal::register_process_tools(&mut registry)?;
        register_workspace_hosts(&mut registry, Arc::clone(&tools))?;
        register_web_hosts(
            &mut registry,
            "std.bench.web",
            WebBackend::new(teal_security::process_context().into()),
        )?;
        register_browser_hosts(&mut registry, "std.bench.browser", tools.browser.clone())?;
        register_computer_hosts(
            &mut registry,
            "std.bench.computer",
            Arc::clone(&tools.computer),
        )?;
        // 计划记录只属于 Agent 工具目录：脚本 Host 目录不暴露更新计划的入口。
        register_plan_host(&mut registry)?;
        let grant = grant_workspace_resources(
            teal::HostExecutionGrant::deny_all("teal-bench/workspace-tools/v4")
                .allow_permissions(TOOL_CAPABILITIES)
                .allow_tools(TOOL_NAMES),
            &tools,
        );
        let grant = Arc::new(grant);
        let host = teal::NativeHostRuntime::builder()
            .audit_context(teal_security::process_context())
            .console(false)
            .file_system(false)
            .standard_registry(registry)
            .authorization(grant.clone())
            .build()?;
        Ok(Self {
            tools,
            host: Mutex::new(host),
            grant,
        })
    }

    /// 把同一工作区能力实现登记到 Teal 自动化 Host 目录。
    pub fn register_script_hosts(
        &self,
        registry: &mut teal::HostRegistry,
    ) -> Result<(), teal::HostRegistryError> {
        register_workspace_hosts(registry, Arc::clone(&self.tools))?;
        register_web_hosts(
            registry,
            "std.bench.web",
            WebBackend::new(teal_security::process_context().into()),
        )?;
        register_browser_hosts(registry, "std.bench.browser", self.tools.browser.clone())?;
        register_computer_hosts(
            registry,
            "std.bench.computer",
            Arc::clone(&self.tools.computer),
        )
    }

    /// 刷新工具准备阶段可见的打开文档身份与修订。
    pub fn synchronize_buffers(
        &self,
        buffers: &teal_workbench_core::BufferStore,
    ) -> Result<(), WorkspaceToolError> {
        self.tools.synchronize_buffers(buffers)
    }

    /// 为一个 Agent Task 开启格式、编译和执行顺序门。
    pub fn begin_task_verification(&self, task_id: &str, generation: u64) {
        self.tools.begin_verification(task_id, generation);
    }

    /// 结束一个 Agent Task 的顺序门，避免影响后续脚本或交互调用。
    pub fn end_task_verification(&self, task_id: &str, generation: u64) {
        self.tools.end_verification(task_id, generation);
    }

    /// 为已获信任的 Teal 脚本组合其精确 capability 与工作区资源边界。
    pub fn script_grant(
        &self,
        capabilities: &std::collections::BTreeSet<String>,
    ) -> Arc<teal::HostExecutionGrant> {
        Arc::new(grant_workspace_resources(
            teal::HostExecutionGrant::deny_all("teal-bench/script-tools/v1")
                .allow_permissions(capabilities.iter().cloned()),
            &self.tools,
        ))
    }

    /// 为一个已批准阶段收缩文件写入集合，仍复用原始工作区事务实现。
    pub fn register_scoped_script_hosts(
        &self,
        registry: &mut teal::HostRegistry,
        paths: &[String],
    ) -> Result<(), teal::HostRegistryError> {
        let mut tools = (*self.tools).clone();
        tools.write_scope = Some(
            paths
                .iter()
                .filter_map(|path| tools.resolve(path))
                .collect(),
        );
        let tools = Arc::new(tools);
        register_workspace_hosts(registry, Arc::clone(&tools))?;
        register_web_hosts(
            registry,
            "std.bench.web",
            WebBackend::new(teal_security::process_context().into()),
        )?;
        register_browser_hosts(registry, "std.bench.browser", tools.browser.clone())?;
        register_computer_hosts(registry, "std.bench.computer", Arc::clone(&tools.computer))
    }

    /// 通过通用 Host 工具桥建立模型目录，不复制 DataShape、回调或检查点处理。
    pub fn tool_registry(&self) -> Result<ToolRegistry, WorkspaceToolError> {
        let mut host = self.host.lock().map_err(|_| WorkspaceToolError::Poisoned)?;
        let mut registry = ToolRegistry::new();
        teal::register_host_tools(
            &mut *host,
            &self.grant,
            teal::HostToolBridgeOptions {
                persistent: false,
                types: None,
            },
            &mut registry,
        )?;
        Ok(registry)
    }

    /// 为持久源码 Agent 建立具有 Host 恢复契约的同一工具目录。
    ///
    /// 不具备恢复契约的定义在注册阶段失败；调用仍须通过当前授权与审批。
    pub fn persistent_tool_registry(&self) -> Result<ToolRegistry, WorkspaceToolError> {
        let mut host = self.host.lock().map_err(|_| WorkspaceToolError::Poisoned)?;
        let mut registry = ToolRegistry::new();
        teal::register_host_tools(
            &mut *host,
            &self.grant,
            teal::HostToolBridgeOptions {
                persistent: true,
                types: None,
            },
            &mut registry,
        )?;
        Ok(registry)
    }
}

fn grant_workspace_resources(
    grant: teal::HostExecutionGrant,
    tools: &WorkspaceTools,
) -> teal::HostExecutionGrant {
    let mut grant = grant
        .allow_resource(teal::ResourceGrant {
            access: teal::ResourceAccess::Read,
            resource: teal::HostResource::Path(tools.root.clone()),
            scope: teal::ResourceScope::Descendants,
        })
        .allow_resource(teal::ResourceGrant {
            access: teal::ResourceAccess::Write,
            resource: teal::HostResource::Path(tools.root.clone()),
            scope: teal::ResourceScope::Descendants,
        })
        .allow_resource(teal::ResourceGrant {
            access: teal::ResourceAccess::Execute,
            resource: teal::HostResource::Path(tools.root.clone()),
            scope: teal::ResourceScope::Descendants,
        });
    for namespace in ["browser", "computer", "process", "workspace-buffer"] {
        for access in [teal::ResourceAccess::Read, teal::ResourceAccess::Write] {
            grant = grant.allow_resource(teal::ResourceGrant {
                access,
                resource: teal::HostResource::Application {
                    namespace: namespace.into(),
                    identity: String::new(),
                },
                scope: teal::ResourceScope::Descendants,
            });
        }
    }
    grant = grant.allow_resource(teal::ResourceGrant {
        access: teal::ResourceAccess::Network,
        resource: teal::HostResource::Network(String::new()),
        scope: teal::ResourceScope::Descendants,
    });
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path).filter_map(|path| path.canonicalize().ok()) {
            grant = grant.allow_resource(teal::ResourceGrant {
                access: teal::ResourceAccess::Execute,
                resource: teal::HostResource::Path(directory),
                scope: teal::ResourceScope::Descendants,
            });
        }
    }
    for executable in [
        teal_tool_executable("teal.check"),
        teal_tool_executable("teal.run"),
        teal_tool_executable("teal.test"),
        teal_format_executable(),
    ]
    .into_iter()
    .flatten()
    .filter_map(|path| path.canonicalize().ok())
    {
        grant = grant.allow_resource(teal::ResourceGrant {
            access: teal::ResourceAccess::Execute,
            resource: teal::HostResource::Path(executable),
            scope: teal::ResourceScope::Exact,
        });
    }
    grant
}

fn string_argument(arguments: &[HostValue], index: usize, name: &str) -> Result<String, HostError> {
    match arguments.get(index) {
        Some(HostValue::String(value)) => Ok(value.clone()),
        _ => Err(host_error(format!("Parameter `{name}` must be string"))),
    }
}

fn parse_changeset_input(value: &str) -> Result<ChangeSetInput, HostError> {
    if value.len() > 8 * 1024 * 1024 {
        return Err(host_error("ChangeSet exceeds the 8 MiB limit"));
    }
    let input: ChangeSetInput = serde_json::from_str(value).map_err(host_error)?;
    if input.files.is_empty() || input.files.len() > 64 {
        return Err(host_error("ChangeSet must contain between 1 and 64 files"));
    }
    for file in &input.files {
        for (name, digest) in [
            ("expected_sha256", &file.expected_sha256),
            ("new_sha256", &file.new_sha256),
        ] {
            if digest.as_ref().is_some_and(|digest| {
                digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            }) {
                return Err(host_error(format!("{name} must be a SHA-256 digest")));
            }
        }
        if file.patch.len() > 2 * 1024 * 1024 {
            return Err(host_error("ChangeSet file patch exceeds the 2 MiB limit"));
        }
    }
    Ok(input)
}

/// 核对外部声称的内容摘要，缺省时按当前文本绑定。
fn bind_changeset_digest(
    claimed: Option<&str>,
    text: &str,
    mismatch: &str,
) -> Result<String, HostError> {
    let actual = crate::digest(text.as_bytes());
    match claimed {
        Some(claimed) if !claimed.eq_ignore_ascii_case(&actual) => Err(host_error(mismatch)),
        Some(claimed) => Ok(claimed.to_ascii_lowercase()),
        None => Ok(actual),
    }
}

fn bool_argument(arguments: &[HostValue], index: usize, name: &str) -> Result<bool, HostError> {
    match arguments.get(index) {
        Some(HostValue::Bool(value)) => Ok(*value),
        _ => Err(host_error(format!("{name} must be a boolean"))),
    }
}

fn int_argument(arguments: &[HostValue], index: usize, name: &str) -> Result<i32, HostError> {
    match arguments.get(index) {
        Some(HostValue::Int(value)) => Ok(*value),
        _ => Err(host_error(format!("{name} must be an integer"))),
    }
}

fn path_kind(metadata: &std::fs::Metadata) -> &'static str {
    if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else if metadata.file_type().is_symlink() {
        "symlink"
    } else {
        "other"
    }
}

fn tool_result(value: &str, checkpoint: Option<String>) -> HostValue {
    HostValue::Map(vec![
        (
            HostValue::String("value".into()),
            HostValue::String(value.into()),
        ),
        (
            HostValue::String("checkpoint".into()),
            checkpoint.map_or(HostValue::None, HostValue::String),
        ),
    ])
}

fn toolchain_result(
    path: &Path,
    tier: &str,
    source_digest: String,
    program_digest: String,
    exit_code: i32,
    output: String,
) -> HostValue {
    let mut fields = vec![
        (
            HostValue::String("exit_code".into()),
            HostValue::Int(exit_code),
        ),
        (
            HostValue::String("output".into()),
            HostValue::String(output),
        ),
        (
            HostValue::String("path".into()),
            HostValue::String(path.to_string_lossy().into_owned()),
        ),
        (
            HostValue::String("tier".into()),
            HostValue::String(tier.into()),
        ),
        (
            HostValue::String("source_digest".into()),
            HostValue::String(source_digest),
        ),
        (
            HostValue::String("program_digest".into()),
            HostValue::String(program_digest),
        ),
    ];
    if exit_code != 0 {
        fields.extend([
            (
                HostValue::String("failure_kind".into()),
                HostValue::String("nonzero_exit".into()),
            ),
            (
                HostValue::String("diagnostic_code".into()),
                HostValue::String("teal_tool_nonzero_exit".into()),
            ),
            (HostValue::String("retryable".into()), HostValue::Bool(true)),
            (
                HostValue::String("suggested_action".into()),
                HostValue::String("Inspect the tool output and repair the Task".into()),
            ),
        ]);
    }
    HostValue::Map(fields)
}

fn json_to_host(value: serde_json::Value) -> Result<HostValue, HostError> {
    match value {
        serde_json::Value::Null => Ok(HostValue::None),
        serde_json::Value::Bool(value) => Ok(HostValue::Bool(value)),
        serde_json::Value::Number(value) => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(HostValue::Int)
            .or_else(|| value.as_f64().map(HostValue::Float))
            .ok_or_else(|| host_error("Diagnostic JSON number is invalid")),
        serde_json::Value::String(value) => Ok(HostValue::String(value)),
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(json_to_host)
            .collect::<Result<Vec<_>, _>>()
            .map(HostValue::Array),
        serde_json::Value::Object(values) => values
            .into_iter()
            .map(|(key, value)| Ok((HostValue::String(key), json_to_host(value)?)))
            .collect::<Result<Vec<_>, HostError>>()
            .map(HostValue::Map),
    }
}

fn host_error(error: impl ToString) -> HostError {
    HostError::new(std::io::ErrorKind::Other, error.to_string())
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized.canonicalize().unwrap_or(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_model_session(model: &str) -> teal_model::ModelSessionContext {
        teal_model::ModelSessionContext {
            target: teal_model::ModelRef {
                provider: teal_model::ProviderId::new("test-provider").unwrap(),
                model: teal_model::ModelId::new(model).unwrap(),
            },
            auth_profile: teal_model::AuthProfileName::new("default").unwrap(),
            service: teal_model::ModelServiceContract {
                endpoint: "https://models.example.test/".into(),
                protocol: teal_model::ProtocolId::new("responses").unwrap(),
                transport: "http".into(),
            },
            account_identity: "test-account".into(),
            account: teal_model::AuthAccount {
                account_id: Some("test-account".into()),
                email: None,
                plan_type: Some("test".into()),
            },
        }
    }

    struct FakeComputer {
        failure: Option<&'static str>,
    }

    impl FakeComputer {
        fn result<T>(&self, value: T) -> Result<T, teal_host::computer::ComputerError> {
            self.failure.map_or(Ok(value), |message| {
                Err(teal_host::computer::ComputerError::Automation(
                    message.into(),
                ))
            })
        }
    }

    impl teal_host::computer::ComputerBackend for FakeComputer {
        fn list_windows(
            &self,
        ) -> Result<Vec<teal_host::computer::ComputerWindow>, teal_host::computer::ComputerError>
        {
            self.result(vec![teal_host::computer::ComputerWindow {
                process_id: 7,
                window_handle: "42".into(),
                title: "fixture".into(),
                generation: 1,
            }])
        }

        fn snapshot(
            &self,
            window: &teal_host::computer::ComputerWindow,
        ) -> Result<teal_host::computer::ComputerSnapshot, teal_host::computer::ComputerError>
        {
            self.result(teal_host::computer::ComputerSnapshot {
                window: teal_host::computer::ComputerWindow {
                    generation: window.generation + 1,
                    ..window.clone()
                },
                elements: Vec::new(),
            })
        }

        fn invoke(
            &self,
            _: &teal_host::computer::ComputerWindow,
            _: &str,
        ) -> Result<(), teal_host::computer::ComputerError> {
            self.result(())
        }
        fn set_value(
            &self,
            _: &teal_host::computer::ComputerWindow,
            _: &str,
            _: &str,
        ) -> Result<(), teal_host::computer::ComputerError> {
            self.result(())
        }
        fn raw_click(
            &self,
            _: &teal_host::computer::ComputerWindow,
            _: &str,
        ) -> Result<(), teal_host::computer::ComputerError> {
            self.result(())
        }
        fn raw_keys(
            &self,
            _: &teal_host::computer::ComputerWindow,
            _: &str,
            _: &str,
        ) -> Result<(), teal_host::computer::ComputerError> {
            self.result(())
        }
    }

    #[test]
    fn computer_hosts_export_the_six_capability_partitioned_tools() {
        let mut registry = teal::HostRegistry::standard();
        register_computer_hosts(
            &mut registry,
            "std.bench.computer",
            Arc::new(FakeComputer { failure: None }),
        )
        .unwrap();
        for function in [
            "list_windows",
            "snapshot",
            "invoke",
            "set_value",
            "raw_click",
            "raw_keys",
        ] {
            assert!(
                registry.contains("std.bench.computer", function),
                "{function}"
            );
        }
    }

    #[test]
    fn workspace_image_export_uses_the_canonical_workspace_read_boundary() {
        let directory = tempfile::tempdir().unwrap();
        let image = directory.path().join("image.png");
        std::fs::write(&image, b"image fixture").unwrap();
        let tools = Arc::new(WorkspaceTools::new(
            directory.path(),
            PathBuf::new(),
            mpsc::channel(1).0,
            mpsc::channel(1).0,
            teal_lsp_client::DiagnosticStore::default(),
        ));
        let prepared = tools
            .prepare(
                WorkspaceOperation::WorkspaceReadFile,
                &[HostValue::String("image.png".into())],
            )
            .unwrap();
        let canonical = image.canonicalize().unwrap();
        assert!(matches!(
            &prepared.resources[..],
            [HostResourceRequest {
                access: ResourceAccess::Read,
                resource: HostResource::Path(path),
            }] if path == &canonical
        ));
        let mut registry = teal::HostRegistry::standard();
        super::media::register_image_host(&mut registry, tools).unwrap();
        assert!(registry.contains("std.bench.workspace", "read_image"));
    }

    #[tokio::test]
    async fn channel_approval_prompts_for_external_tools_and_allows_pure_tools() {
        let (sender, mut receiver) = mpsc::channel(8);
        let approval = Arc::new(ChannelApproval::new(sender));
        for (tool, access, resource) in [
            (
                "web.fetch",
                teal_agent::ActionResourceAccess::Read,
                "network://https://example.com/api",
            ),
            (
                "computer.list_windows",
                teal_agent::ActionResourceAccess::Read,
                "application://computer/windows",
            ),
            (
                "computer.snapshot",
                teal_agent::ActionResourceAccess::Read,
                "application://computer/windows",
            ),
            (
                "computer.invoke",
                teal_agent::ActionResourceAccess::Interact,
                "application://computer/windows",
            ),
            (
                "computer.set_value",
                teal_agent::ActionResourceAccess::Interact,
                "application://computer/windows",
            ),
            (
                "computer.raw_click",
                teal_agent::ActionResourceAccess::Interact,
                "application://computer/windows",
            ),
            (
                "computer.raw_keys",
                teal_agent::ActionResourceAccess::Interact,
                "application://computer/windows",
            ),
            (
                "terminal.write",
                teal_agent::ActionResourceAccess::Interact,
                "application://terminal/session-token",
            ),
        ] {
            let approval = Arc::clone(&approval);
            let task = tokio::spawn(async move {
                approval
                    .authorize(
                        ActionContext {
                            run_id: "run".into(),
                            task_id: "task".into(),
                            generation: 1,
                            policy_version: "test".into(),
                        },
                        ActionSpec {
                            id: tool.into(),
                            tool: tool.into(),
                            arguments: json!({}),
                            resources: vec![teal_agent::ResourceVersion {
                                access,
                                resource: resource.into(),
                                version: "v1".into(),
                            }],
                        },
                    )
                    .await
            });
            let prompt = receiver.recv().await.unwrap();
            assert_eq!(prompt.call.name, tool);
            if tool == "terminal.write" {
                assert_eq!(prompt.permission.scopes, vec![ApprovalScope::Once]);
            }
            prompt
                .response
                .send(ApprovalResponse::Allow(ApprovalScope::Once))
                .unwrap();
            assert!(matches!(task.await.unwrap().unwrap(), Decision::Allow));
        }
        // 没有外部资源的纯工具直接放行，不进入交互通道。
        assert!(matches!(
            approval
                .authorize(
                    ActionContext {
                        run_id: "run".into(),
                        task_id: "task".into(),
                        generation: 1,
                        policy_version: "test".into()
                    },
                    ActionSpec {
                        id: "read".into(),
                        tool: "workspace.read_file".into(),
                        arguments: json!({}),
                        resources: Vec::new()
                    }
                )
                .await
                .unwrap(),
            Decision::Allow
        ));
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn channel_approval_applies_task_scope_to_the_same_normalized_workspace_operation() {
        let directory = tempfile::tempdir().expect("workspace fixture");
        let workspace = directory.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace directory");
        let path = workspace.join("file.tl");
        std::fs::write(&path, "before").expect("workspace file");
        let canonical = path
            .canonicalize()
            .expect("workspace file should canonicalize");
        let paths =
            teal_config::ConfigPaths::with_home(directory.path().join("home"), Some(&workspace));
        let (sender, mut receiver) = mpsc::channel(2);
        let approval = Arc::new(ChannelApproval::for_workspace(sender, paths));
        let context = ActionContext {
            run_id: "run".into(),
            task_id: "task".into(),
            generation: 1,
            policy_version: "caller-version".into(),
        };
        let action = ActionSpec {
            id: "write-1".into(),
            tool: "workspace.write_file".into(),
            arguments: json!({"contents": "after"}),
            resources: vec![teal_agent::ResourceVersion {
                access: teal_agent::ActionResourceAccess::Write,
                resource: format!("file://{}", canonical.to_string_lossy().replace('\\', "/")),
                version: "before".into(),
            }],
        };
        let pending = {
            let approval = Arc::clone(&approval);
            let context = context.clone();
            let action = action.clone();
            tokio::spawn(async move { approval.authorize(context, action).await })
        };
        let prompt = receiver.recv().await.expect("workspace write prompt");
        assert_eq!(prompt.call.name, "workspace.write_file");
        assert!(prompt.permission.scopes.contains(&ApprovalScope::Task));
        assert_eq!(prompt.permission.resources, action.resources);
        prompt
            .response
            .send(ApprovalResponse::Allow(ApprovalScope::Task))
            .expect("task response should deliver");
        assert_eq!(
            pending
                .await
                .expect("approval task should join")
                .expect("decision"),
            Decision::Allow
        );

        let mut retry = action;
        retry.id = "write-2".into();
        assert_eq!(
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                approval.authorize(context, retry),
            )
            .await
            .expect("task-scoped operation must not prompt")
            .expect("task-scoped decision"),
            Decision::Allow
        );
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn successful_toolchain_result_carries_the_delivery_validation_fields() {
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("main.tl");
        std::fs::write(&script, "fn main() {}\n").unwrap();
        let result = toolchain_result(
            &script,
            "agentic",
            "source-digest".into(),
            "program-digest".into(),
            0,
            "tool output".into(),
        );
        let HostValue::Map(fields) = result else {
            panic!("toolchain result must be a map");
        };
        let field = |name: &str| {
            fields
                .iter()
                .find_map(|(key, value)| {
                    matches!(key, HostValue::String(key) if key == name).then_some(value)
                })
                .unwrap_or_else(|| panic!("toolchain field `{name}` is missing"))
        };
        assert_eq!(field("exit_code"), &HostValue::Int(0));
        assert_eq!(
            field("path"),
            &HostValue::String(script.to_string_lossy().into_owned())
        );
        assert_eq!(field("tier"), &HostValue::String("agentic".into()));
        assert_eq!(
            field("source_digest"),
            &HostValue::String("source-digest".into())
        );
        assert_eq!(
            field("program_digest"),
            &HostValue::String("program-digest".into())
        );
        assert_eq!(field("output"), &HostValue::String("tool output".into()));
    }

    #[test]
    fn teal_toolchain_dispatches_every_operation_to_its_leaf_binary() {
        let path = Path::new("fixture.tl");
        for (name, stem, arguments) in [
            (
                "teal.check",
                "teal-c",
                vec![path.to_string_lossy().into_owned()],
            ),
            (
                "teal.fmt_check",
                "teal-fmt",
                vec!["--check".to_owned(), path.to_string_lossy().into_owned()],
            ),
            (
                "teal.test",
                "teal-test",
                vec![path.to_string_lossy().into_owned()],
            ),
            (
                "teal.run",
                "teal-run",
                vec![path.to_string_lossy().into_owned()],
            ),
        ] {
            assert_eq!(teal_tool_stem(name), stem, "{name}");
            assert_eq!(teal_tool_arguments(name, path), arguments, "{name}");
        }

        let mut run = teal_tool_arguments("teal.run", path);
        run.extend([
            "--tier".to_owned(),
            "agentic".to_owned(),
            "--permission-file".to_owned(),
            "permissions.toml".to_owned(),
        ]);
        assert_eq!(
            run,
            [
                "fixture.tl",
                "--tier",
                "agentic",
                "--permission-file",
                "permissions.toml",
            ]
        );
    }

    #[test]
    fn teal_run_preparation_binds_the_permission_file_as_a_read_resource() {
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("main.tl");
        let permission_file = root.path().join("permissions.toml");
        std::fs::write(&script, "fn main() {}\n").unwrap();
        std::fs::write(&permission_file, "version = 3\n").unwrap();
        let (sender, _receiver) = mpsc::channel(1);
        let tools = WorkspaceTools::new(
            root.path(),
            root.path().join("checkpoints"),
            sender,
            mpsc::channel(1).0,
            teal_lsp_client::DiagnosticStore::default(),
        );

        let prepared = tools
            .prepare(
                WorkspaceOperation::TealRun,
                &[
                    HostValue::String("main.tl".into()),
                    HostValue::String("agentic".into()),
                    HostValue::String("permissions.toml".into()),
                ],
            )
            .unwrap();
        assert_eq!(
            prepared.arguments[2],
            HostValue::String(
                permission_file
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .into()
            )
        );
        for expected in [script, permission_file] {
            let expected = expected.canonicalize().unwrap();
            assert!(prepared.resources.iter().any(|request| {
                matches!(request, HostResourceRequest {
                    access: ResourceAccess::Read,
                    resource: HostResource::Path(path),
                } if path == &expected)
            }));
        }
    }

    #[tokio::test]
    async fn diagnostics_use_canonical_uri_paths_and_exclude_other_workspaces() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = root.path().join("\u{4e16}\u{754c}.rs");
        let other = outside.path().join("outside.rs");
        std::fs::write(&file, "a😀b").unwrap();
        std::fs::write(&other, "outside").unwrap();
        let diagnostics = teal_lsp_client::DiagnosticStore::default();
        for path in [&file, &other] {
            let uri = url::Url::from_file_path(path.canonicalize().unwrap()).unwrap();
            assert!(diagnostics.update(&json!({"uri":uri,"version":4,"diagnostics":[{"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":3}},"severity":1,"message":"\u{5b9e}\u{9645}\u{8bca}\u{65ad}"}]}),Some("a😀b"),Some(4)));
        }
        let (sender, _receiver) = mpsc::channel(1);
        let tools = WorkspaceTools::new(
            root.path(),
            root.path().join("checkpoints"),
            sender,
            mpsc::channel(1).0,
            diagnostics,
        );
        for path in ["", "\u{4e16}\u{754c}.rs"] {
            let value = tools
                .invoke(
                    WorkspaceOperation::WorkspaceDiagnostics,
                    &[HostValue::String(path.to_owned())],
                    None,
                )
                .unwrap();
            let HostValue::Array(records) = value else {
                panic!("diagnostics should return an array");
            };
            assert_eq!(records.len(), 1);
            let HostValue::Map(record) = &records[0] else {
                panic!("diagnostic should be a map");
            };
            let field = |name: &str| {
                record
                    .iter()
                    .find_map(|(key, value)| {
                        matches!(key, HostValue::String(key) if key == name).then_some(value)
                    })
                    .unwrap_or_else(|| panic!("diagnostic field `{name}` is missing"))
            };
            assert_eq!(
                field("message"),
                &HostValue::String("\u{5b9e}\u{9645}\u{8bca}\u{65ad}".into())
            );
        }
    }

    #[test]
    fn workspace_host_rejects_paths_outside_its_root_before_queuing_a_mutation() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let (sender, mut receiver) = mpsc::channel(1);
        let tools = WorkspaceTools::new(
            root.path(),
            root.path().join("checkpoints"),
            sender,
            mpsc::channel(1).0,
            teal_lsp_client::DiagnosticStore::default(),
        );
        for path in [
            "../outside.txt".to_owned(),
            outside
                .path()
                .join("outside.txt")
                .to_string_lossy()
                .into_owned(),
        ] {
            assert!(
                tools
                    .invoke(
                        WorkspaceOperation::WorkspaceReadFile,
                        &[HostValue::String(path.clone())],
                        None,
                    )
                    .is_err()
            );
            assert!(
                tools
                    .invoke(
                        WorkspaceOperation::WorkspaceWriteFile,
                        &[HostValue::String(path), HostValue::String("blocked".into())],
                        None,
                    )
                    .is_err()
            );
        }
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn workspace_discovery_range_limits_and_root_mutations_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("alpha.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::create_dir(root.path().join("nested")).unwrap();
        std::fs::write(root.path().join("nested/item.txt"), "item").unwrap();
        let (sender, mut receiver) = mpsc::channel(1);
        let tools = WorkspaceTools::new(
            root.path(),
            root.path().join("checkpoints"),
            sender,
            mpsc::channel(1).0,
            teal_lsp_client::DiagnosticStore::default(),
        );

        let listed = tools
            .invoke(
                WorkspaceOperation::WorkspaceList,
                &[
                    HostValue::String(String::new()),
                    HostValue::Bool(false),
                    HostValue::Int(1),
                ],
                None,
            )
            .unwrap();
        assert!(matches!(listed, HostValue::Array(entries) if entries.len() == 1));
        assert!(
            tools
                .invoke(
                    WorkspaceOperation::WorkspaceList,
                    &[
                        HostValue::String(String::new()),
                        HostValue::Bool(true),
                        HostValue::Int(0),
                    ],
                    None,
                )
                .is_err()
        );
        assert!(
            tools
                .invoke(
                    WorkspaceOperation::WorkspaceStat,
                    &[HostValue::String("../outside.txt".into())],
                    None,
                )
                .is_err()
        );

        let range = tools
            .invoke(
                WorkspaceOperation::WorkspaceReadRange,
                &[
                    HostValue::String("alpha.txt".into()),
                    HostValue::Int(2),
                    HostValue::Int(2),
                ],
                None,
            )
            .unwrap();
        assert_eq!(range, HostValue::String("two\nthree".into()));
        std::fs::write(root.path().join("large.txt"), "x".repeat(256 * 1024 + 1)).unwrap();
        assert!(
            tools
                .invoke(
                    WorkspaceOperation::WorkspaceReadRange,
                    &[
                        HostValue::String("large.txt".into()),
                        HostValue::Int(1),
                        HostValue::Int(1),
                    ],
                    None,
                )
                .is_err()
        );

        for (operation, arguments) in [
            (
                WorkspaceOperation::WorkspaceCreateDir,
                vec![HostValue::String(String::new())],
            ),
            (
                WorkspaceOperation::WorkspaceDelete,
                vec![HostValue::String(String::new())],
            ),
            (
                WorkspaceOperation::WorkspaceMove,
                vec![
                    HostValue::String(String::new()),
                    HostValue::String("nested".into()),
                ],
            ),
        ] {
            assert!(
                tools.invoke(operation, &arguments, None).is_err(),
                "{}",
                operation.name()
            );
        }
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn agent_catalog_uses_all_shared_workspace_host_definitions() {
        let root = tempfile::tempdir().unwrap();
        let (sender, _receiver) = mpsc::channel(1);
        let runtime = WorkspaceToolRuntime::new(
            root.path(),
            root.path().join("checkpoints"),
            sender,
            mpsc::channel(1).0,
            teal_lsp_client::DiagnosticStore::default(),
        )
        .unwrap();
        let definitions = runtime.tool_registry().unwrap().definitions();
        let names = definitions
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>();
        let mut expected = TOOL_NAMES.to_vec();
        expected.sort_unstable();
        assert_eq!(names, expected);
        let write = definitions
            .iter()
            .find(|definition| definition.name == "workspace.write_file")
            .unwrap();
        assert_eq!(
            write.input.json_schema()["required"],
            json!(["path", "content"])
        );
        assert_eq!(write.output.json_schema()["type"], "string");
        let diagnostics = definitions
            .iter()
            .find(|definition| definition.name == "workspace.diagnostics")
            .unwrap();
        assert_eq!(diagnostics.input.json_schema()["required"], json!(["path"]));
        assert_eq!(diagnostics.output.json_schema()["type"], "array");
        // 测试工具与检查、执行共用同一程序摘要，并接受可选的名称过滤器。
        let tests = definitions
            .iter()
            .find(|definition| definition.name == "teal.test")
            .expect("the test tool must be registered");
        // `filter` 是必填参数，模型用空串表示“运行全部”，与宿主签名一致。
        assert_eq!(
            tests.input.json_schema()["required"],
            json!(["path", "tier", "filter"])
        );
        assert_eq!(tests.input.json_schema()["type"], "object");
    }

    #[test]
    fn source_query_snapshot_prefers_open_buffers_for_entry_and_imports() {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main.tl");
        let dependency = root.path().join("dependency.tl");
        std::fs::write(
            &main,
            "import \"dependency.tl\" as dependency; fn main() { dependency::missing(); }",
        )
        .unwrap();
        std::fs::write(
            &dependency,
            "pub fn answer() -> string { return \"disk\"; }",
        )
        .unwrap();

        let entry_overlay =
            "import \"dependency.tl\" as dependency; fn main() { print(dependency::answer()); }";
        let dependency_overlay = "pub fn answer() -> int { return 42; }";
        let snapshot = SourceQuerySnapshot::new(
            root.path(),
            [
                (main.clone(), entry_overlay.to_owned()),
                (dependency.clone(), dependency_overlay.to_owned()),
            ],
        )
        .unwrap();
        let inspected = snapshot
            .inspect(&main, teal::CompileOptions::default())
            .expect("open entry and imported buffers should form one source snapshot");
        let sources = inspected.analysis.sources();
        assert!(sources.iter().any(|source| {
            source.source_name().ends_with("main.tl") && source.source_contents() == entry_overlay
        }));
        assert!(sources.iter().any(|source| {
            source.source_name().ends_with("dependency.tl")
                && source.source_contents() == dependency_overlay
        }));
    }

    #[test]
    fn source_query_snapshot_reads_each_disk_import_once() {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main.tl");
        let dependency = root.path().join("dependency.tl");
        std::fs::write(
            &main,
            "import \"dependency.tl\" as dependency; fn main() { print(dependency::answer()); }",
        )
        .unwrap();
        let original = "pub fn answer() -> int { return 42; }";
        std::fs::write(&dependency, original).unwrap();

        let snapshot = SourceQuerySnapshot::new(root.path(), []).unwrap();
        snapshot
            .inspect(&main, teal::CompileOptions::default())
            .expect("initial disk snapshot should compile");
        std::fs::write(&dependency, "pub fn broken() -> int { return missing; }").unwrap();
        let inspected = snapshot
            .inspect(&main, teal::CompileOptions::default())
            .expect("a captured query must not mix a later disk revision");
        assert!(inspected.analysis.sources().iter().any(|source| {
            source.source_name().ends_with("dependency.tl") && source.source_contents() == original
        }));
    }

    struct ScriptedWorkspaceModel {
        responses: Mutex<std::collections::VecDeque<Vec<teal_model::ModelEvent>>>,
    }

    impl teal_model::ModelProvider for ScriptedWorkspaceModel {
        fn name(&self) -> &str {
            "workspace-test"
        }

        fn stream(
            &self,
            _: teal_model::ModelRequest,
            events: mpsc::Sender<teal_model::ModelEvent>,
            _: teal_model::Cancellation,
        ) -> teal_model::ProviderFuture<Result<(), teal_model::ModelError>> {
            let response = self.responses.lock().unwrap().pop_front();
            Box::pin(async move {
                for event in response
                    .ok_or_else(|| teal_model::ModelError::Provider("response exhausted".into()))?
                {
                    events
                        .send(event)
                        .await
                        .map_err(|_| teal_model::ModelError::Cancelled)?;
                }
                Ok(())
            })
        }
    }

    #[derive(Default)]
    struct RecordingAllow {
        actions: Mutex<Vec<ActionSpec>>,
    }

    impl PermissionProvider for RecordingAllow {
        fn version(&self) -> &str {
            "workspace-test/v1"
        }

        fn authorize(
            &self,
            _: ActionContext,
            action: ActionSpec,
        ) -> teal_model::ProviderFuture<Result<Decision, teal_agent::PolicyError>> {
            self.actions.lock().unwrap().push(action);
            Box::pin(async { Ok(Decision::Allow) })
        }
    }

    fn model_tool_events(
        id: &str,
        name: &str,
        arguments: serde_json::Value,
    ) -> Vec<teal_model::ModelEvent> {
        vec![
            teal_model::ModelEvent::ToolStart {
                id: id.into(),
                name: name.into(),
            },
            teal_model::ModelEvent::ToolArguments {
                id: id.into(),
                delta: arguments.to_string(),
            },
            teal_model::ModelEvent::ToolEnd { id: id.into() },
            teal_model::ModelEvent::End {
                reason: teal_model::StopReason::ToolUse,
            },
        ]
    }

    #[tokio::test]
    async fn plan_tool_is_registered_for_agents_only() {
        let root = tempfile::tempdir().unwrap();
        let runtime = WorkspaceToolRuntime::new(
            root.path(),
            root.path().join("checkpoints"),
            mpsc::channel(1).0,
            mpsc::channel(1).0,
            teal_lsp_client::DiagnosticStore::default(),
        )
        .unwrap();
        let registry = runtime.tool_registry().unwrap();
        let plan = registry
            .definitions()
            .into_iter()
            .find(|definition| definition.name == "update_plan")
            .expect("agents must see the plan tool");
        assert!(plan.description.contains("never delivery evidence"));
        let teal_agent::DataShape::Record { fields, .. } = &plan.input else {
            panic!("the plan tool takes a named parameter record");
        };
        assert_eq!(
            fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            vec!["steps"]
        );

        let mut scripts = teal::HostRegistry::standard();
        runtime.register_script_hosts(&mut scripts).unwrap();
        assert!(
            !scripts.contains("std.bench.plan", "update"),
            "script hosts must not expose the plan tool"
        );

        // 探索工具同样只导出给 Agent：记录候选与采纳都只改 Flow 记录。
        for (name, function, expected_field) in [
            ("record_proposals", "record_proposals", "goal"),
            ("adopt_proposal", "adopt_proposal", "id"),
        ] {
            let definition = registry
                .definitions()
                .into_iter()
                .find(|definition| definition.name == name)
                .unwrap_or_else(|| panic!("agents must see `{name}`"));
            let teal_agent::DataShape::Record { fields, .. } = &definition.input else {
                panic!("`{name}` takes a named parameter record");
            };
            assert!(
                fields.iter().any(|field| field.name == expected_field),
                "`{name}` must expose the `{expected_field}` parameter"
            );
            assert!(
                !scripts.contains("std.bench.plan", function),
                "script hosts must not expose `{name}`"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn agent_registry_authorizes_invokes_and_returns_workspace_checkpoint() {
        struct TestAudit;
        impl teal_security::AuditSink for TestAudit {
            fn append(
                &self,
                _: &teal_security::Event,
            ) -> Result<teal_security::Receipt, teal_security::AuditError> {
                Ok(teal_security::Receipt {
                    sequence: 1,
                    hash: "1".repeat(64),
                })
            }
        }
        let _ = teal_security::install_process_sink(Arc::new(TestAudit));
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("file.txt");
        std::fs::write(&path, "before").unwrap();
        let (mutations, mut mutation_rx) = mpsc::channel(1);
        let runtime = WorkspaceToolRuntime::new(
            root.path(),
            root.path().join("checkpoints"),
            mutations,
            mpsc::channel(1).0,
            teal_lsp_client::DiagnosticStore::default(),
        )
        .unwrap();
        let tools = Arc::new(runtime.tool_registry().unwrap());
        let model = Arc::new(ScriptedWorkspaceModel {
            responses: Mutex::new(std::collections::VecDeque::from([
                model_tool_events(
                    "write-1",
                    "workspace.write_file",
                    json!({"path":"file.txt","content":"after"}),
                ),
                model_tool_events("complete-1", "__teal_complete", json!({"value":"done"})),
            ])),
        });
        let permissions = Arc::new(RecordingAllow::default());
        let mutation = tokio::spawn(async move {
            let request =
                tokio::time::timeout(std::time::Duration::from_secs(5), mutation_rx.recv())
                    .await
                    .expect("authorized workspace mutation must be dispatched")
                    .expect("workspace mutation request");
            assert_eq!(request.path, normalize(&path));
            assert!(matches!(request.kind, MutationKind::Write(ref text) if text == "after"));
            request.response.send(Ok(())).unwrap();
        });
        let agent = teal_agent::Agent::builder(
            teal_agent::AgentConfig {
                model: test_model_session("test-model"),
                compaction_model: None,
                instructions: "test".into(),
                limits: teal_agent::RuntimeLimits::default(),
            },
            model,
        )
        .grant(runtime.agent_grant())
        .tools(tools)
        .build()
        .with_permissions(permissions.clone());
        let result = agent
            .run(
                teal_agent::AgentInvocation {
                    context: ActionContext {
                        run_id: "run".into(),
                        task_id: "task".into(),
                        generation: 1,
                        policy_version: permissions.version().into(),
                    },
                    input: "write".into(),
                    history: Vec::new(),
                    result_schema: teal_model::DataShape::String,
                    memory: None,
                    window_checkpoint: None,
                    window_origin_id: None,
                },
                teal_model::Cancellation::new(),
            )
            .await
            .expect("workspace tool loop should complete");
        mutation.await.unwrap();

        assert_eq!(result.value, json!("done"));
        assert_eq!(result.tool_calls, 2, "workspace call plus typed completion");
        assert_eq!(result.checkpoints.len(), 1);
        assert!(result.checkpoints[0].contains("backup"));
        let actions = permissions.actions.lock().unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].tool, "workspace.write_file");
        assert!(actions[0].resources.iter().any(|resource| {
            resource.access == teal_agent::ActionResourceAccess::Write
                && resource.resource.starts_with("file://")
        }));
    }

    fn changeset_tools(root: &Path) -> (mpsc::Receiver<MutationRequest>, WorkspaceTools) {
        let (mutations, mutations_rx) = mpsc::channel(1);
        let (changesets, _changesets_rx) = mpsc::channel(1);
        let tools = WorkspaceTools::new(
            root,
            root.join("checkpoints"),
            mutations,
            changesets,
            teal_lsp_client::DiagnosticStore::default(),
        );
        (mutations_rx, tools)
    }

    #[test]
    fn changeset_preparation_binds_every_declared_file_and_rejects_escapes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("first.tl"), "one\n").unwrap();
        let (_mutations, tools) = changeset_tools(root.path());
        let patch = "--- a/first.tl\n+++ b/first.tl\n@@ -1 +1 @@\n-one\n+ONE";
        let changes = json!({
            "files": [
                {"path": "first.tl", "patch": patch},
                {"path": "nested/second.tl", "patch": patch},
            ]
        })
        .to_string();

        let prepared = tools
            .prepare(
                WorkspaceOperation::WorkspaceApplyChangeset,
                &[HostValue::String(changes)],
            )
            .unwrap();
        assert_eq!(prepared.resources.len(), 2);
        assert!(prepared.resources.iter().all(|request| matches!(
            request,
            HostResourceRequest {
                access: ResourceAccess::Write,
                resource: HostResource::Path(_),
            }
        )));
        let HostValue::String(serialized) = &prepared.arguments[0] else {
            panic!("prepared ChangeSet arguments must stay a JSON string");
        };
        let resolved: serde_json::Value = serde_json::from_str(serialized).unwrap();
        for (index, suffix) in ["first.tl", "nested\\second.tl"].into_iter().enumerate() {
            let path = resolved["files"][index]["path"].as_str().unwrap();
            assert!(
                path.replace('\\', "/")
                    .ends_with(&suffix.replace('\\', "/")),
                "{path} must resolve under the root"
            );
        }

        let escape = json!({"files": [{"path": "../escape.tl", "patch": patch}]}).to_string();
        assert!(
            tools
                .prepare(
                    WorkspaceOperation::WorkspaceApplyChangeset,
                    &[HostValue::String(escape)],
                )
                .is_err()
        );
    }

    #[test]
    fn changeset_host_tool_rejects_stale_digests_limits_and_outside_paths() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("first.tl"), "one\n").unwrap();
        let (mut receiver, tools) = changeset_tools(root.path());
        let patch = "--- a/first.tl\n+++ b/first.tl\n@@ -1 +1 @@\n-one\n+ONE";
        let digest = "a".repeat(64);
        let outside_path = outside.path().join("escape.tl");
        let cases = [
            (
                "expected_sha256",
                json!({"files": [
                    {"path": "first.tl", "patch": patch, "expected_sha256": digest}
                ]}),
            ),
            (
                "new_sha256",
                json!({"files": [
                    {"path": "first.tl", "patch": patch, "new_sha256": digest}
                ]}),
            ),
            ("empty", json!({"files": []})),
            (
                "missing hunks",
                json!({"files": [{"path": "first.tl", "patch": "no hunks here"}]}),
            ),
            (
                "outside root",
                json!({"files": [
                    {"path": outside_path.to_string_lossy(), "patch": patch}
                ]}),
            ),
        ];

        for (name, input) in cases {
            assert!(
                tools
                    .invoke(
                        WorkspaceOperation::WorkspaceApplyChangeset,
                        &[HostValue::String(input.to_string())],
                        None,
                    )
                    .is_err(),
                "{name} must fail closed"
            );
        }
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn changeset_host_tool_binds_digests_and_queues_one_review() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("first.tl"), "one\ntwo\n").unwrap();
        let (mutations, _mutations_rx) = mpsc::channel(1);
        let (changesets, mut reviews) = mpsc::channel(1);
        let tools = Arc::new(WorkspaceTools::new(
            root.path(),
            root.path().join("checkpoints"),
            mutations,
            changesets,
            teal_lsp_client::DiagnosticStore::default(),
        ));
        let changes = json!({
            "files": [
                {
                    "path": "first.tl",
                    "patch": "--- a/first.tl\n+++ b/first.tl\n@@ -1,2 +1,2 @@\n-one\n+ONE\n two",
                },
                {
                    "path": "nested/created.tl",
                    "patch": "--- a/nested/created.tl\n+++ b/nested/created.tl\n@@ -0,0 +1 @@\n+created",
                },
            ]
        })
        .to_string();

        // 工具会阻塞等待用户审查，因此从其他线程调用并在此处回复。
        let caller = Arc::clone(&tools);
        let invoked = std::thread::spawn(move || {
            caller.invoke(
                WorkspaceOperation::WorkspaceApplyChangeset,
                &[HostValue::String(changes)],
                None,
            )
        });

        let request = reviews.blocking_recv().expect("ChangeSet review request");
        assert_eq!(request.change_set.files.len(), 2);
        let edited = &request.change_set.files[0];
        assert_eq!(edited.path, PathBuf::from("first.tl"));
        assert_eq!(edited.original_sha256, crate::digest(b"one\ntwo\n"));
        assert_eq!(edited.new_sha256, crate::digest(b"ONE\ntwo\n"));
        assert_eq!(edited.hunks.len(), 1);
        let created = &request.change_set.files[1];
        assert_eq!(created.path, PathBuf::from("nested").join("created.tl"));
        assert_eq!(created.original_sha256, crate::digest(b""));
        assert_eq!(created.new_sha256, crate::digest(b"created\n"));
        request
            .response
            .send(Ok("allowed-all".into()))
            .expect("review response");

        assert_eq!(
            invoked.join().unwrap().unwrap(),
            HostValue::String("allowed-all".into())
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("first.tl")).unwrap(),
            "one\ntwo\n",
            "the host tool must leave writes to the shared transaction"
        );
    }

    #[test]
    fn patch_host_tool_binds_the_current_digest_when_none_is_claimed() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("main.tl"), "old\n").unwrap();
        let (mut receiver, tools) = changeset_tools(root.path());
        let tools = Arc::new(tools);
        let patch = "--- a/main.tl\n+++ b/main.tl\n@@ -1 +1 @@\n-old\n+new";

        // 模型无法自行计算规范化文本摘要，空值必须绑定当前内容而不是被拒绝。
        let caller = Arc::clone(&tools);
        let queued = std::thread::spawn(move || {
            caller.invoke(
                WorkspaceOperation::WorkspaceApplyPatch,
                &[
                    HostValue::String("main.tl".into()),
                    HostValue::String(String::new()),
                    HostValue::String(patch.into()),
                ],
                None,
            )
        });
        let request = receiver
            .blocking_recv()
            .expect("bound patch must queue a mutation");
        assert_eq!(
            request.expected_content_sha256,
            Some(crate::digest(b"old\n"))
        );
        assert!(matches!(
            request.kind,
            MutationKind::ApplyPatch { ref expected_sha256, .. }
                if expected_sha256 == &crate::digest(b"old\n")
        ));
        request
            .response
            .send(Ok(()))
            .expect("mutation response channel");
        assert!(queued.join().unwrap().is_ok());

        assert!(
            tools
                .invoke(
                    WorkspaceOperation::WorkspaceApplyPatch,
                    &[
                        HostValue::String("main.tl".into()),
                        HostValue::String("not-a-digest".into()),
                        HostValue::String(patch.into()),
                    ],
                    None,
                )
                .is_err()
        );
    }
}
