//! 每次事件或后台任务拥有独立 Host 上下文。脚本只能提交业务数据，不能签发宿主收据。

use std::sync::{Arc, Mutex};
use super::*;
use teal::{
    ExternalCallRecovery, HostCallSignature, HostError, HostFunctionDefinition, HostRegistry,
    HostValue, RuntimeType,
};

/// 证据 SDK 的唯一签名目录：名称、字符串参数数、能力、是否为 task。
pub const EVIDENCE_SIGNATURES: [(&str, usize, &str, bool); 3] = [
    ("capture", 1, "editor.read", false),
    ("command", 2, "extension.command", true),
    ("git", 4, "extension.git", true),
];

/// 为独立包预编译补齐证据 SDK 声明。占位实现始终拒绝执行，不授予权限。
///
/// # Errors
/// 注册表拒绝静态 Host 定义时返回错误。
pub fn register_evidence_preflight(
    registry: &mut HostRegistry,
) -> Result<(), teal::HostRegistryError> {
    for (name, count, capability, task) in EVIDENCE_SIGNATURES {
        if registry.contains("std.bench.evidence", name) {
            continue;
        }
        let signature = signature(vec![RuntimeType::String; count], RuntimeType::String);
        let definition = if task {
            HostFunctionDefinition::asynchronous(
                "std.bench.evidence",
                name,
                signature,
                capability,
                ExternalCallRecovery::Journaled,
                |_| Box::pin(async { Err(error("evidence execution requires a Bench task Host")) }),
            )
        } else {
            HostFunctionDefinition::new(
                "std.bench.evidence",
                name,
                signature,
                capability,
                ExternalCallRecovery::Pure,
                |_| Err(error("evidence read requires a Bench task Host")),
            )
        };
        registry.register(definition)?;
    }
    Ok(())
}

/// 一次脚本调用尚未提交的结果；成功返回后才由应用提交业务事务。
#[derive(Clone, Debug, Default)]
pub struct ExtensionCallOutput {
    /// 与业务事务一起保存的后台任务控制请求。
    pub controls: BTreeMap<String, ExtensionTaskControl>,
    /// 更新后的实例业务数据。
    pub values: BTreeMap<String, String>,
    /// 待关联 RuntimeStore 的任务请求。
    pub tasks: Vec<ExtensionTaskRef>,
    /// 等待用户决定的审批请求。
    pub approvals: Vec<ApprovalReceipt>,
    /// 任务显式提交的结果；没有结果不等于完成。
    pub result: Option<String>,
    /// 可选的标准 UI JSON 树。
    pub rendered: Option<String>,
    /// 供面板展示的消息。
    pub messages: Vec<String>,
}

/// 固定事件、配置和业务状态的调用上下文。
#[derive(Clone)]
pub struct ExtensionCall {
    instance: ExtensionInstance,
    event: ExtensionEvent,
    config: BTreeMap<String, String>,
    task_names: BTreeSet<String>,
    output: Arc<Mutex<ExtensionCallOutput>>,
    result_sink: Option<(Arc<teal_agent::RuntimeStore>, String)>,
    task_states: BTreeMap<String, String>,
}

impl ExtensionCall {
    /// 建立独立调用，不执行任务、不写文件、不授予权限。
    #[must_use]
    pub fn new(
        instance: ExtensionInstance,
        event: ExtensionEvent,
        config: BTreeMap<String, String>,
        task_names: BTreeSet<String>,
    ) -> Self {
        let output = ExtensionCallOutput {
            values: instance.values.clone(),
            ..Default::default()
        };
        Self {
            instance,
            event,
            config,
            task_names,
            output: Arc::new(Mutex::new(output)),
            result_sink: None,
            task_states: BTreeMap::new(),
        }
    }

    /// 将显式结果写到对应运行的持久记录；只有该运行成功终止后宿主才接受结果。
    #[must_use]
    pub fn with_result_store(
        mut self,
        store: Arc<teal_agent::RuntimeStore>,
        run_id: String,
    ) -> Self {
        self.result_sink = Some((store, run_id));
        self
    }

    /// 安装事件入队时捕获的任务状态，展示信息不充当运行终态。
    #[must_use]
    pub fn with_task_states(mut self, states: BTreeMap<String, String>) -> Self {
        self.task_states = states;
        self
    }

    /// 返回本次调用的拥有所有权结果。
    ///
    /// # Errors
    /// 调用上下文被恐慌污染时返回错误。
    pub fn output(&self) -> Result<ExtensionCallOutput, ExtensionError> {
        self.output
            .lock()
            .map(|v| v.clone())
            .map_err(|_| ExtensionError::Invalid("call context is poisoned".into()))
    }

    /// 登记 SDK 和面板桥；签名相同但每次调用捕获不同实例快照。
    ///
    /// # Errors
    /// 注册表已存在同名接口或定义不合法时返回错误。
    pub fn register(&self, registry: &mut HostRegistry) -> Result<(), teal::HostRegistryError> {
        if !registry.contains("std.json", "read_string") {
            teal::register_json_module(registry)?;
        }
        for (name, arguments, capability) in [
            ("event", 1, "extension.read"),
            ("config", 1, "extension.read"),
            ("get", 1, "extension.read"),
            ("set", 2, "extension.write"),
            ("start", 2, "extension.task"),
            ("complete", 1, "extension.write"),
            ("message", 1, "ui.panel"),
            ("request_approval", 4, "extension.approve"),
            ("approval", 1, "extension.read"),
            ("quote", 1, "extension.read"),
            ("start_bound", 3, "extension.task"),
            ("task_status", 1, "extension.read"),
            ("cancel", 1, "extension.task"),
            ("resume", 1, "extension.task"),
        ] {
            let context = self.clone();
            registry.register(HostFunctionDefinition::new(
                "std.bench.extension",
                name,
                signature(vec![RuntimeType::String; arguments], RuntimeType::String),
                capability,
                if name == "complete" && self.result_sink.is_some() {
                    ExternalCallRecovery::Journaled
                } else {
                    ExternalCallRecovery::Pure
                },
                move |args| context.call(name, args),
            ))?;
        }
        let context = self.clone();
        registry.register(HostFunctionDefinition::new(
            "std.bench.panel",
            "render",
            signature(vec![RuntimeType::String], RuntimeType::None),
            "ui.panel",
            ExternalCallRecovery::Pure,
            move |args| {
                let value = string(args, 0)?;
                if value.len() > 2 * 1024 * 1024 {
                    return Err(error("UI tree exceeds 2 MiB"));
                }
                context
                    .output
                    .lock()
                    .map_err(|_| error("call context is poisoned"))?
                    .rendered = Some(value);
                Ok(HostValue::None)
            },
        ))?;
        let event = self.event.clone();
        registry.register(HostFunctionDefinition::new(
            "std.bench.panel",
            "event",
            signature(vec![], RuntimeType::Map),
            "ui.panel",
            ExternalCallRecovery::Pure,
            move |_| {
                let mut fields = event.fields.clone();
                fields
                    .entry("kind".into())
                    .or_insert_with(|| "mount".into());
                Ok(HostValue::Map(
                    fields
                        .into_iter()
                        .map(|(k, v)| (HostValue::String(k), HostValue::String(v)))
                        .collect(),
                ))
            },
        ))?;
        Ok(())
    }

    fn call(&self, name: &str, arguments: &[HostValue]) -> Result<HostValue, HostError> {
        let key = string(arguments, 0)?;
        let mut output = self
            .output
            .lock()
            .map_err(|_| error("call context is poisoned"))?;
        let value = match name {
            "event" => match key.as_str() {
                "id" => self.event.id.clone(),
                "kind" => self.event.kind.clone(),
                "name" => self.event.name.clone(),
                "instance" => self.instance.id.clone(),
                _ => self.event.fields.get(&key).cloned().unwrap_or_default(),
            },
            "config" => self.config.get(&key).cloned().unwrap_or_default(),
            "quote" => {
                serde_json::to_string(&key).map_err(|_| error("cannot encode JSON string"))?
            }
            "task_status" => {
                if !self.instance.tasks.contains_key(&key) {
                    "missing".into()
                } else {
                    self.task_states
                        .get(&key)
                        .cloned()
                        .unwrap_or_else(|| "pending".into())
                }
            }
            "cancel" | "resume" => {
                if self.result_sink.is_some() || !self.instance.tasks.contains_key(&key) {
                    return Err(error(
                        "event handler can only control its own declared task",
                    ));
                }
                output.controls.insert(
                    key,
                    if name == "cancel" {
                        ExtensionTaskControl::Cancel
                    } else {
                        ExtensionTaskControl::Resume
                    },
                );
                String::new()
            }
            "get" => output.values.get(&key).cloned().unwrap_or_default(),
            "set" => {
                if key.is_empty() || key.len() > 256 {
                    return Err(error("invalid state key"));
                }
                output.values.insert(key, string(arguments, 1)?);
                String::new()
            }
            "start" | "start_bound" => {
                if self.result_sink.is_some() {
                    return Err(error("start is only available in short event handlers"));
                }
                if !self.task_names.contains(&key) {
                    return Err(error("task is not declared in extension manifest"));
                }
                if output.tasks.len() >= 16 {
                    return Err(error("too many task requests in one event"));
                }
                let input = string(arguments, 1)?;
                let approval_ids: Vec<String> = if name == "start_bound" {
                    serde_json::from_str(&string(arguments, 2)?)
                        .map_err(|_| error("approval IDs must be a JSON string array"))?
                } else {
                    Vec::new()
                };
                for id in &approval_ids {
                    if !self
                        .instance
                        .approvals
                        .get(id)
                        .is_some_and(|r| r.approved == Some(true))
                    {
                        return Err(error("required approval is not accepted"));
                    }
                }
                let id = format!(
                    "task-{}",
                    sha256(
                        serde_json::to_string(&(
                            &self.instance.id,
                            &self.event.id,
                            output.tasks.len()
                        ))
                        .map_err(|_| error("cannot encode task identity"))?
                        .as_bytes()
                    )
                );
                let state = output.values.clone();
                output.tasks.push(ExtensionTaskRef {
                    callback: Some(ExtensionCallback {
                        kind: self.event.kind.clone(),
                        name: self.event.name.clone(),
                    }),
                    key: id.clone(),
                    task: key,
                    input,
                    approval_ids,
                    state,
                    run_id: None,
                    delivered: false,
                    control: None,
                });
                id
            }
            "complete" => {
                if output.result.is_some() {
                    return Err(error("task result has already been submitted"));
                }
                if let Some((store, run)) = &self.result_sink {
                    store
                        .submit_prepared_result(run, &key)
                        .map_err(|e| error(&e.to_string()))?;
                }
                output.result = Some(key);
                String::new()
            }
            "message" => {
                output.messages.push(key);
                String::new()
            }
            "request_approval" => {
                // 文件摘要由应用在展示前采集；脚本不能提供任意 sha256 作为宿主证据。
                let paths: Vec<String> = serde_json::from_str(&string(arguments, 1)?)
                    .map_err(|_| error("approval paths must be a JSON string array"))?;
                let id = format!(
                    "{}:{}:approval:{}",
                    self.instance.id,
                    self.event.id,
                    output.approvals.len()
                );
                output.approvals.push(ApprovalReceipt {
                    callback: Some(ExtensionCallback {
                        kind: self.event.kind.clone(),
                        name: self.event.name.clone(),
                    }),
                    id: id.clone(),
                    prompt: key,
                    artifacts: paths
                        .into_iter()
                        .map(|path| ArtifactReceipt {
                            path,
                            sha256: String::new(),
                        })
                        .collect(),
                    operation: string(arguments, 2)?,
                    environment: string(arguments, 3)?,
                    approved: None,
                });
                id
            }
            "approval" => self
                .instance
                .approvals
                .get(&key)
                .map_or("missing", |r| match r.approved {
                    Some(true) => "approved",
                    Some(false) => "rejected",
                    None => "pending",
                })
                .into(),
            _ => return Err(error("unknown extension SDK function")),
        };
        if output.values.values().map(String::len).sum::<usize>() > MAX_STATE_BYTES {
            return Err(error("state exceeds 8 MiB"));
        }
        Ok(HostValue::String(value))
    }
}

fn string(values: &[HostValue], index: usize) -> Result<String, HostError> {
    match values.get(index) {
        Some(HostValue::String(value)) if value.len() <= 1024 * 1024 => Ok(value.clone()),
        _ => Err(error("expected string of at most 1 MiB")),
    }
}
fn error(message: &str) -> HostError {
    HostError::new(std::io::ErrorKind::InvalidInput, message)
}
fn signature(arguments: Vec<RuntimeType>, result: RuntimeType) -> HostCallSignature {
    HostCallSignature::new(arguments, result).expect("SDK only exposes Host values")
}
