//! Teal 脚本准备、能力检查、执行及自动化状态持久化。
//!
//! 准备结果拥有编译程序、完整导入闭包和能力快照；执行前使用这一快照检查信任。
//! 宿主返回拥有所有权的值，界面副作用由根应用提供的桥接队列延后应用。

use std::io::Write;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use teal::{
    CompileOptions, CompiledProgram, DylibPolicy, ExternalCallRecovery, HostCallSignature,
    HostError, HostFunctionDefinition, HostRegistry, HostValue, NativeHostRuntime,
    ProfileFsSourceLoader, RuntimeTier, RuntimeType, Source, VmOptions,
};
use teal_workbench_core::syntax::GrammarSpec;
use thiserror::Error;

use crate::{GrammarHost, TrustDecision, TrustStore, builtin_grammar_scripts};

#[cfg(test)]
mod example_tests;

fn automation_compile_options() -> CompileOptions {
    CompileOptions::default().with_tier(RuntimeTier::Agentic)
}

fn register_host<F>(
    registry: &mut HostRegistry,
    module: &'static str,
    function: impl Into<String>,
    signature: HostCallSignature,
    capability: &'static str,
    callback: F,
) -> Result<(), teal::HostRegistryError>
where
    F: Fn(&[HostValue]) -> Result<HostValue, HostError> + Send + Sync + 'static,
{
    registry.register(HostFunctionDefinition::new(
        module,
        function,
        signature,
        capability,
        ExternalCallRecovery::Unsupported,
        callback,
    ))
}

/// 一个 Teal 脚本的来源与默认信任策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptKind {
    /// 用户级配置作为用户自己的代码受信任。
    UserConfig,

    /// 工作区配置需要内容/能力批准。
    WorkspaceConfig,

    /// 内置文法包只获得语法注册表。
    BuiltinGrammar,
}

/// Teal 激活脚本注册的命令或钩子入口。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationCommand {
    /// 面向用户的命令/事件名称。
    pub name: String,

    /// 调用时执行的脚本路径；相对路径由注册配置所在目录解析。
    pub script_path: String,
}

/// Teal 配置脚本注册的一个命名工作台 Agent。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSettings {
    /// 在工作台命令和选择器中显示的唯一名称。
    pub name: String,
    /// Teal 认证供应商 ID。
    pub provider: String,
    /// 供应商下的显式 profile；省略时由认证管理器解析活动 profile。
    pub profile: Option<String>,
    /// 模型工厂接收的显式模型 ID。
    pub model: String,
    /// 独立摘要模型在 model.toml 中的选择预设；省略时确定性压缩。
    #[serde(default)]
    pub compaction_profile: Option<String>,
    /// 每次请求使用的固定指令。
    pub instructions: String,
}

/// Teal 配置声明的非秘密 MCP 服务器。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSettings {
    /// 稳定服务器名称。
    pub name: String,
    /// `http` 或 `stdio`。
    pub transport: String,
    /// HTTP 地址或直接启动的程序路径。
    pub target: String,
    /// stdio 独立参数；HTTP 必须为空。
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct AutomationState {
    values: BTreeMap<String, String>,

    #[serde(skip)]
    commands: Vec<AutomationCommand>,

    #[serde(skip)]
    hooks: Vec<AutomationCommand>,

    #[serde(skip)]
    panels: Vec<AutomationCommand>,

    #[serde(skip)]
    session_restore: Option<String>,

    #[serde(skip)]
    agents: Vec<AgentSettings>,

    #[serde(skip)]
    active_agent: Option<String>,

    #[serde(skip)]
    mcp_servers: Vec<McpSettings>,
}

/// 脚本执行后的注册项与持久键值状态。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    /// 最近一次入口执行的 Teal 退出码；配置快照读取默认为零。
    pub exit_code: i32,
    /// 已注册命令。
    pub commands: Vec<AutomationCommand>,

    /// 已注册事件钩子。
    pub hooks: Vec<AutomationCommand>,

    /// 已注册自定义面板。
    pub panels: Vec<AutomationCommand>,

    /// 带命名空间的持久字符串值。
    pub values: BTreeMap<String, String>,

    /// 配置请求的启动会话恢复策略。
    pub session_restore: Option<String>,

    /// 当前脚本注册的命名 Agent。
    pub agents: Vec<AgentSettings>,

    /// 当前脚本请求的活动 Agent 名称。
    pub active_agent: Option<String>,
    /// 当前脚本显式注册的 MCP 服务器。
    pub mcp_servers: Vec<McpSettings>,
}

/// 已编译脚本以及精确的预检标识与能力。
#[derive(Clone)]
pub struct PreparedScript {
    path: PathBuf,
    contents: Vec<u8>,
    hash: [u8; 32],
    capabilities: BTreeSet<String>,
    program: CompiledProgram,
    kind: ScriptKind,
    source_inputs: BTreeMap<PathBuf, String>,
    user_source_identities: BTreeSet<PathBuf>,
}

impl PreparedScript {
    /// 返回实际文件输入及摘要，供已编译快照执行前轻量检查；保留导入别名路径。
    #[must_use]
    pub fn source_inputs(&self) -> &BTreeMap<PathBuf, String> {
        &self.source_inputs
    }
    /// 返回不透明的编译程序，供扩展后台执行门面复用相同快照。
    #[must_use]
    pub const fn program(&self) -> &CompiledProgram {
        &self.program
    }
    /// 返回信任授权使用的带版本根源码及导入闭包。
    pub fn trust_contents(&self) -> &[u8] {
        &self.contents
    }

    /// 返回规范或请求的脚本路径。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 返回精确源码哈希。
    #[must_use]
    pub const fn hash(&self) -> [u8; 32] {
        self.hash
    }

    /// 返回已解析的宿主能力标签。
    #[must_use]
    pub const fn capabilities(&self) -> &BTreeSet<String> {
        &self.capabilities
    }

    /// 返回脚本来源/信任类别。
    #[must_use]
    pub const fn kind(&self) -> ScriptKind {
        self.kind
    }
}

/// 脚本编译、信任、宿主注册表或执行失败。
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// 必需的持久审计写入失败。
    #[error("Teal automation audit failed: {0}")]
    Audit(#[from] teal_security::AuditError),
    /// 自动化动作可能已发生，但审计结果无法确认。
    #[error("Teal automation audit result unavailable; effect may be unknown: {0}")]
    AuditUnknown(teal_security::AuditError),
    /// 文件系统访问失败。
    #[error("Teal automation I/O failed: {0}")]
    Io(#[from] std::io::Error),

    /// Teal 编译/加载/运行时诊断。
    #[error("Teal automation failed: {0}")]
    Teal(String),

    /// 宿主注册失败。
    #[error("Teal host registry failed: {0}")]
    Registry(#[from] teal::HostRegistryError),

    /// 工作区脚本尚未获准运行此快照。
    #[error("Workspace Teal scripts require capability approval")]
    ApprovalRequired,

    /// 持久状态 JSON 失败。
    #[error("Teal automation status format error: {0}")]
    Json(#[from] serde_json::Error),

    /// 宿主回调状态被先前的恐慌污染。
    #[error("Teal automation status is unavailable")]
    Poisoned,
}

/// 感知能力的 Teal 注册表、状态与文法集成。
#[derive(Clone)]
pub struct WorkbenchAutomationRuntime {
    audit: teal_security::AuditContext,
    meter: Option<opentelemetry::metrics::Meter>,
    function_spans: bool,
    registry: HostRegistry,
    grammar_host: GrammarHost,
    state: Arc<Mutex<AutomationState>>,
    state_path: PathBuf,
    execution: Arc<Mutex<()>>,
}

impl WorkbenchAutomationRuntime {
    /// 用独立注册表建立扩展执行上下文，不安装旧配置状态或自动化注册回调。
    #[must_use]
    pub fn for_extension(mut registry: HostRegistry) -> Self {
        crate::extension::register_evidence_preflight(&mut registry)
            .expect("static evidence preflight signatures");
        if !registry.contains("std.bench.extension", "event") {
            let instance = crate::extension::ExtensionInstance::new("preflight", "preflight", "");
            let event = crate::extension::ExtensionEvent {
                id: "preflight".into(),
                kind: "preflight".into(),
                name: String::new(),
                fields: BTreeMap::new(),
            };
            crate::extension::ExtensionCall::new(instance, event, BTreeMap::new(), BTreeSet::new())
                .register(&mut registry)
                .expect("fresh extension SDK registry");
        }
        Self {
            audit: teal_security::process_context(),
            meter: None,
            function_spans: false,
            registry,
            grammar_host: GrammarHost::new(),
            state: Arc::new(Mutex::new(AutomationState::default())),
            state_path: PathBuf::new(),
            execution: Arc::new(Mutex::new(())),
        }
    }

    /// 执行扩展短事件；不清空注册，不写旧自动化状态。
    ///
    /// # Errors
    /// 编译快照要求的权限不足、Host 绑定或 VM 执行失败时返回错误。
    pub fn execute_event(
        &self,
        prepared: &PreparedScript,
        grant: Arc<teal::HostExecutionGrant>,
    ) -> Result<i32, RuntimeError> {
        let event = teal_security::Event::new(
            teal_security::Action::Extension,
            teal_security::Phase::Intent,
            teal_security::Outcome::Pending,
            teal_security::new_correlation_id(),
        )?
        .with_actor(teal_security::Actor::Extension);
        let audit = self.audit.begin(event)?;
        let result = self.execute_event_inner(prepared, grant);
        audit
            .finish(if result.is_ok() {
                teal_security::Outcome::Succeeded
            } else {
                teal_security::Outcome::Failed
            })
            .map_err(RuntimeError::AuditUnknown)?;
        result
    }

    fn execute_event_inner(
        &self,
        prepared: &PreparedScript,
        grant: Arc<teal::HostExecutionGrant>,
    ) -> Result<i32, RuntimeError> {
        let mut host = NativeHostRuntime::builder()
            .audit_context(self.audit.clone())
            .standard_registry(self.registry.clone())
            .dylib_policy(DylibPolicy::Deny)
            .authorization(grant.clone())
            .echo(false)
            .build()
            .map_err(|error| RuntimeError::Teal(error.to_string()))?;
        let request = prepared
            .program
            .execution_request(&mut host)
            .map_err(|e| RuntimeError::Teal(e.to_string()))?;
        grant
            .authorize(&request)
            .map_err(|e| RuntimeError::Teal(e.to_string()))?;
        self.observation()
            .execute_program(
                prepared.program.clone(),
                &mut host,
                VmOptions::for_tier(RuntimeTier::Agentic),
            )
            .map(|result| result.exit_code)
            .map_err(|e| RuntimeError::Teal(e.to_string()))
    }
    /// 创建标准 `std.bench.config` 和 `std.bench.syntax` 宿主模块。
    pub fn new(state_path: impl Into<PathBuf>) -> Result<Self, RuntimeError> {
        let state_path = state_path.into();
        let state = match fs::read(&state_path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                AutomationState::default()
            }
            Err(error) => return Err(error.into()),
        };
        let state = Arc::new(Mutex::new(state));
        let grammar_host = GrammarHost::new();
        let mut registry = HostRegistry::standard();
        grammar_host.register(&mut registry)?;
        register_config_host(&mut registry, &state)?;
        Ok(Self {
            audit: teal_security::process_context(),
            meter: None,
            function_spans: false,
            registry,
            grammar_host,
            state,
            state_path,
            execution: Arc::new(Mutex::new(())),
        })
    }

    /// 为扩展执行和状态持久化指定宿主审计接收器。
    #[must_use]
    pub fn with_audit_context(mut self, audit: teal_security::AuditContext) -> Self {
        self.audit = audit;
        self
    }

    /// 编译及执行复用宿主 Meter 和当前 tracing 订阅器。不配置导出器、不改变能力授权，也不负责关闭提供器；克隆会保留这些设置。
    pub fn set_observation(
        &mut self,
        meter: Option<opentelemetry::metrics::Meter>,
        function_spans: bool,
    ) {
        self.meter = meter;
        self.function_spans = function_spans;
    }

    fn observation(&self) -> teal::observability::ObservationSession {
        teal::observability::ObservationSession::from_host(self.meter.clone(), self.function_spans)
            .with_audit_context(self.audit.clone())
    }

    /// 返回注册表，使应用可以添加编辑器/网络/智能体模块。
    pub const fn registry_mut(&mut self) -> &mut HostRegistry {
        &mut self.registry
    }

    /// 读取并编译文件入口 `main`，收集完整导入闭包及所需宿主能力。
    ///
    /// 此步骤不执行脚本。来源分类由调用方传入，路径规范化本身不会把用户配置重新分类为工作区代码。
    /// 返回的准备结果必须与后续信任检查和执行使用的同一份快照对应。
    /// 调用方可通过 `prepare(path, ScriptKind::WorkspaceConfig)` 准备工作区脚本，检查信任后再执行。
    ///
    /// # Errors
    /// 文件读取、UTF-8 解码、编译或宿主能力解析失败时返回错误。
    pub fn prepare(
        &self,
        path: impl AsRef<Path>,
        kind: ScriptKind,
    ) -> Result<PreparedScript, RuntimeError> {
        let path = path
            .as_ref()
            .canonicalize()
            .unwrap_or_else(|_| path.as_ref().to_path_buf());
        let contents = fs::read(&path)?;
        let code = String::from_utf8(contents.clone())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        self.prepare_source(path, contents, code, kind)
    }

    /// 检查已准备脚本的配置式信任策略。
    #[must_use]
    pub fn trust_decision(&self, prepared: &PreparedScript, trust: &TrustStore) -> TrustDecision {
        match prepared.kind {
            ScriptKind::UserConfig => {
                let trusted_root = prepared
                    .path
                    .parent()
                    .and_then(|parent| parent.canonicalize().ok());
                // 导入身份在编译读取时冻结；不能在批准时重新解链接而把旧的越界源码误分类。
                if trusted_root.is_some_and(|root| {
                    prepared
                        .user_source_identities
                        .iter()
                        .all(|identity| identity.starts_with(&root))
                }) {
                    TrustDecision::Trusted
                } else {
                    trust.check_with_inputs(
                        &prepared.path,
                        &prepared.contents,
                        &prepared.capabilities,
                        prepared.source_inputs.keys().map(PathBuf::as_path),
                    )
                }
            }
            ScriptKind::BuiltinGrammar => TrustDecision::Trusted,
            ScriptKind::WorkspaceConfig => trust.check_with_inputs(
                &prepared.path,
                &prepared.contents,
                &prepared.capabilities,
                prepared.source_inputs.keys().map(PathBuf::as_path),
            ),
        }
    }

    /// 在调用方完成信任检查后运行一个已准备脚本。
    ///
    /// 本方法不会自行调用 [`Self::trust_decision`] 或弹出批准提示。执行锁串行化脚本，
    /// 每次先清空注册结果，成功运行后持久化自动化状态，再返回注册快照。
    /// 执行或持久化失败不能证明宿主副作用从未发生，调用方应按实际错误和状态处理。
    pub fn execute(
        &self,
        prepared: PreparedScript,
        context_json: &str,
    ) -> Result<RuntimeSnapshot, RuntimeError> {
        self.execute_legacy(prepared, context_json, true, None)
    }

    /// 使用调用方按 Host 资源边界组合的授权执行脚本。
    ///
    /// # Errors
    /// 返回与 [`Self::execute`] 相同的授权、执行或持久化错误。
    pub fn execute_with_grant(
        &self,
        prepared: PreparedScript,
        context_json: &str,
        grant: Arc<teal::HostExecutionGrant>,
    ) -> Result<RuntimeSnapshot, RuntimeError> {
        self.execute_legacy(prepared, context_json, true, Some(grant))
    }

    /// 旧命令、Hook 和面板的事件适配，不重置已建立的动态注册快照。
    ///
    /// # Errors
    /// 返回与 execute 相同的授权、执行或持久化错误。
    pub fn execute_legacy_event(
        &self,
        prepared: PreparedScript,
        context_json: &str,
    ) -> Result<RuntimeSnapshot, RuntimeError> {
        self.execute_legacy(prepared, context_json, false, None)
    }

    /// 使用调用方资源授权执行旧命令、Hook 或面板事件。
    ///
    /// # Errors
    /// 返回与 [`Self::execute_legacy_event`] 相同的授权、执行或持久化错误。
    pub fn execute_legacy_event_with_grant(
        &self,
        prepared: PreparedScript,
        context_json: &str,
        grant: Arc<teal::HostExecutionGrant>,
    ) -> Result<RuntimeSnapshot, RuntimeError> {
        self.execute_legacy(prepared, context_json, false, Some(grant))
    }

    #[tracing::instrument(name = "teal.automation.execute", skip_all)]
    fn execute_legacy(
        &self,
        prepared: PreparedScript,
        context_json: &str,
        activation: bool,
        grant: Option<Arc<teal::HostExecutionGrant>>,
    ) -> Result<RuntimeSnapshot, RuntimeError> {
        let event = teal_security::Event::new(
            teal_security::Action::Extension,
            teal_security::Phase::Intent,
            teal_security::Outcome::Pending,
            teal_security::new_correlation_id(),
        )?
        .with_actor(teal_security::Actor::Extension);
        let audit = self.audit.begin(event)?;
        let result = self.execute_legacy_inner(prepared, context_json, activation, grant);
        audit
            .finish(if result.is_ok() {
                teal_security::Outcome::Succeeded
            } else {
                teal_security::Outcome::Failed
            })
            .map_err(RuntimeError::AuditUnknown)?;
        result
    }

    fn execute_legacy_inner(
        &self,
        prepared: PreparedScript,
        context_json: &str,
        activation: bool,
        grant: Option<Arc<teal::HostExecutionGrant>>,
    ) -> Result<RuntimeSnapshot, RuntimeError> {
        let _execution = self.execution.lock().map_err(|_| RuntimeError::Poisoned)?;
        let grant = grant.unwrap_or_else(|| {
            Arc::new(
                teal::HostExecutionGrant::deny_all("teal-bench/automation/v2")
                    .allow_permissions(prepared.capabilities.iter().cloned()),
            )
        });
        let mut host = NativeHostRuntime::builder()
            .audit_context(self.audit.clone())
            .arguments(vec![context_json.to_owned()])
            .echo(false)
            .standard_registry(self.registry.clone())
            .dylib_policy(DylibPolicy::Deny)
            .authorization(grant.clone())
            .build()
            .map_err(|error| RuntimeError::Teal(error.to_string()))?;
        host.install_service(tracing::Span::current())
            .map_err(|error| RuntimeError::Teal(error.to_string()))?;
        let request = prepared
            .program
            .execution_request(&mut host)
            .map_err(|error| RuntimeError::Teal(error.to_string()))?;
        grant
            .authorize(&request)
            .map_err(|error| RuntimeError::Teal(error.to_string()))?;
        if activation {
            let mut state = self.state.lock().map_err(|_| RuntimeError::Poisoned)?;
            state.commands.clear();
            state.hooks.clear();
            state.panels.clear();
            state.session_restore = None;
            state.agents.clear();
            state.active_agent = None;
            state.mcp_servers.clear();
        }
        let result = self
            .observation()
            .execute_program(
                prepared.program,
                &mut host,
                VmOptions::for_tier(RuntimeTier::Agentic),
            )
            .map_err(|error| RuntimeError::Teal(error.to_string()))?;
        self.persist_state()?;
        let mut snapshot = self.snapshot()?;
        snapshot.exit_code = result.exit_code;
        Ok(snapshot)
    }

    /// 编译并执行所有随附 Teal 文法包。
    pub fn load_builtin_grammars(&self) -> Result<Vec<GrammarSpec>, RuntimeError> {
        for (language, code) in builtin_grammar_scripts() {
            let path = PathBuf::from(format!("<builtin:{language}>"));
            let prepared = self
                .prepare_source(
                    path,
                    code.as_bytes().to_vec(),
                    code,
                    ScriptKind::BuiltinGrammar,
                )
                .map_err(|error| {
                    RuntimeError::Teal(format!("Built-in grammar `{language}`: {error}"))
                })?;
            let grant = Arc::new(
                teal::HostExecutionGrant::deny_all("teal-bench/builtin-grammar/v1")
                    .allow_permissions(prepared.capabilities.iter().cloned()),
            );
            let mut host = NativeHostRuntime::builder()
                .audit_context(self.audit.clone())
                .standard_registry(self.registry.clone())
                .dylib_policy(DylibPolicy::Deny)
                .authorization(grant)
                .build()
                .map_err(|error| RuntimeError::Teal(error.to_string()))?;
            self.observation()
                .execute_program(
                    prepared.program,
                    &mut host,
                    VmOptions::for_tier(RuntimeTier::Agentic),
                )
                .map_err(|error| RuntimeError::Teal(error.to_string()))?;
        }
        self.grammar_host
            .take_grammars()
            .map_err(|error| RuntimeError::Teal(error.to_string()))
    }

    /// 返回当前临时注册项和持久值。
    pub fn snapshot(&self) -> Result<RuntimeSnapshot, RuntimeError> {
        let state = self.state.lock().map_err(|_| RuntimeError::Poisoned)?;
        Ok(RuntimeSnapshot {
            exit_code: 0,
            commands: state.commands.clone(),
            hooks: state.hooks.clone(),
            panels: state.panels.clone(),
            values: state.values.clone(),
            session_restore: state.session_restore.clone(),
            agents: state.agents.clone(),
            active_agent: state.active_agent.clone(),
            mcp_servers: state.mcp_servers.clone(),
        })
    }

    fn prepare_source(
        &self,
        path: PathBuf,
        contents: Vec<u8>,
        code: String,
        kind: ScriptKind,
    ) -> Result<PreparedScript, RuntimeError> {
        let loader = ClosureLoader::default();
        let program = self
            .observation()
            .compile_source(
                Source::new(path.to_string_lossy(), code),
                &loader,
                automation_compile_options(),
            )
            .map_err(|error| RuntimeError::Teal(error.to_string()))?;

        let mut host = NativeHostRuntime::builder()
            .audit_context(self.audit.clone())
            .standard_registry(self.registry.clone())
            .dylib_policy(DylibPolicy::Deny)
            .build()
            .map_err(|error| RuntimeError::Teal(error.to_string()))?;
        let request = program
            .execution_request(&mut host)
            .map_err(|error| RuntimeError::Teal(error.to_string()))?;
        let capabilities = request.required_permissions().clone();
        let mut source_inputs = loader
            .inputs
            .into_inner()
            .map_err(|_| RuntimeError::Poisoned)?;
        let user_source_identities = loader
            .user_source_identities
            .into_inner()
            .map_err(|_| RuntimeError::Poisoned)?;
        if path.exists() {
            source_inputs.insert(path.clone(), crate::extension::sha256(&contents));
        }
        let mut sources = loader
            .sources
            .into_inner()
            .map_err(|_| RuntimeError::Poisoned)?;
        sources.insert(
            path.to_string_lossy().into_owned(),
            trust_source_bytes(&teal::SourceProvenance::User, &contents)?,
        );

        // 信任覆盖入口及其实际导入闭包；未使用包模块变化不应使当前脚本失效。
        let mut contents = b"teal-bench-trust-closure-v3".to_vec();
        for (identity, source) in sources {
            // 长度前缀消除哈希输入中相邻标识和内容的歧义。
            contents.extend_from_slice(&(identity.len() as u64).to_le_bytes());
            contents.extend_from_slice(identity.as_bytes());
            contents.extend_from_slice(&(source.len() as u64).to_le_bytes());
            contents.extend_from_slice(&source);
        }

        Ok(PreparedScript {
            path,
            hash: Sha256::digest(&contents).into(),
            contents,
            capabilities,
            program,
            kind,
            source_inputs,
            user_source_identities,
        })
    }

    fn persist_state(&self) -> Result<(), RuntimeError> {
        if let Some(parent) = self.state_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let state = self.state.lock().map_err(|_| RuntimeError::Poisoned)?;
        let parent = self.state_path.parent().unwrap_or_else(|| Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut temporary, &*state)?;
        temporary.write_all(b"\n")?;
        temporary.as_file_mut().sync_all()?;
        temporary
            .persist(&self.state_path)
            .map_err(|error| RuntimeError::Io(error.error))?;
        Ok(())
    }
}

#[derive(Default)]
struct ClosureLoader {
    sources: Mutex<BTreeMap<String, Vec<u8>>>,
    inputs: Mutex<BTreeMap<PathBuf, String>>,
    user_source_identities: Mutex<BTreeSet<PathBuf>>,
}

impl teal::SourceLoader for ClosureLoader {
    fn load(
        &self,
        importer: &Path,
        import: &str,
    ) -> Result<teal::LoadedSource, teal::SourceLoadError> {
        let loaded = ProfileFsSourceLoader::for_workbench().load(importer, import)?;
        if matches!(loaded.source.provenance, teal::SourceProvenance::User) {
            let requested = importer.parent().unwrap_or(Path::new(".")).join(import);
            self.user_source_identities
                .lock()
                .map_err(|_| teal::SourceLoadError {
                    message: "Source identity lock is poisoned".into(),
                })?
                .insert(loaded.identity.clone());
            self.inputs
                .lock()
                .map_err(|_| teal::SourceLoadError {
                    message: "Source input lock is poisoned".into(),
                })?
                .insert(
                    requested,
                    crate::extension::sha256(loaded.source.contents().as_bytes()),
                );
        }
        self.sources
            .lock()
            .map_err(|_| teal::SourceLoadError {
                message: "Source closure lock is poisoned".into(),
            })?
            .insert(
                loaded.identity.to_string_lossy().into_owned(),
                trust_source_bytes(
                    &loaded.source.provenance,
                    loaded.source.contents().as_bytes(),
                )
                .map_err(|error| teal::SourceLoadError {
                    message: error.to_string(),
                })?,
            );
        Ok(loaded)
    }
}

fn trust_source_bytes(
    provenance: &teal::SourceProvenance,
    contents: &[u8],
) -> Result<Vec<u8>, serde_json::Error> {
    let mut output = serde_json::to_vec(provenance)?;
    output.push(0);
    output.extend_from_slice(contents);
    Ok(output)
}

fn register_config_host(
    registry: &mut HostRegistry,
    state: &Arc<Mutex<AutomationState>>,
) -> Result<(), teal::HostRegistryError> {
    for (function, target) in [
        ("register_command", RegistrationTarget::Command),
        ("register_hook", RegistrationTarget::Hook),
        ("register_panel", RegistrationTarget::Panel),
    ] {
        let state = Arc::clone(state);
        register_host(
            registry,
            "std.bench.config",
            function,
            signature(
                [RuntimeType::String, RuntimeType::String],
                RuntimeType::None,
            ),
            "ui.panel",
            move |arguments| {
                let command = AutomationCommand {
                    name: string_arg(arguments, 0)?,
                    script_path: string_arg(arguments, 1)?,
                };
                let mut state = state
                    .lock()
                    .map_err(|_| host_error("Automation status lock is poisoned"))?;
                match target {
                    RegistrationTarget::Command => state.commands.push(command),
                    RegistrationTarget::Hook => state.hooks.push(command),
                    RegistrationTarget::Panel => state.panels.push(command),
                }
                Ok(HostValue::None)
            },
        )?;
    }
    let state_get = Arc::clone(state);
    register_host(
        registry,
        "std.bench.config",
        "state_get",
        signature([RuntimeType::String], RuntimeType::String),
        "editor.read",
        move |arguments| {
            let key = string_arg(arguments, 0)?;
            Ok(HostValue::String(
                state_get
                    .lock()
                    .map_err(|_| host_error("Automation status lock is poisoned"))?
                    .values
                    .get(&key)
                    .cloned()
                    .unwrap_or_default(),
            ))
        },
    )?;
    let state_set = Arc::clone(state);
    register_host(
        registry,
        "std.bench.config",
        "state_set",
        signature(
            [RuntimeType::String, RuntimeType::String],
            RuntimeType::None,
        ),
        "editor.write",
        move |arguments| {
            let key = string_arg(arguments, 0)?;
            let value = string_arg(arguments, 1)?;
            state_set
                .lock()
                .map_err(|_| host_error("Automation status lock is poisoned"))?
                .values
                .insert(key, value);
            Ok(HostValue::None)
        },
    )?;
    let session_restore = Arc::clone(state);
    register_host(
        registry,
        "std.bench.config",
        "set_session_restore",
        signature([RuntimeType::String], RuntimeType::None),
        "ui.panel",
        move |arguments| {
            let policy = string_arg(arguments, 0)?.trim().to_ascii_lowercase();
            if !matches!(policy.as_str(), "prompt" | "always" | "never") {
                return Err(host_error(
                    "Session recovery policy must be prompt, always, or never",
                ));
            }
            session_restore
                .lock()
                .map_err(|_| host_error("Automation status lock is poisoned"))?
                .session_restore = Some(policy);
            Ok(HostValue::None)
        },
    )?;

    let agents = Arc::clone(state);
    register_host(
        registry,
        "std.bench.config",
        "register_agent",
        signature(
            [
                RuntimeType::String,
                RuntimeType::String,
                RuntimeType::String,
                RuntimeType::String,
                RuntimeType::String,
            ],
            RuntimeType::None,
        ),
        "agent.configure",
        move |arguments| {
            let profile = string_arg(arguments, 2)?;
            let settings = AgentSettings {
                name: string_arg(arguments, 0)?,
                provider: string_arg(arguments, 1)?,
                profile: (!profile.is_empty()).then_some(profile),
                model: string_arg(arguments, 3)?,
                compaction_profile: None,
                instructions: string_arg(arguments, 4)?,
            };
            if settings.name.trim().is_empty()
                || settings.provider.trim().is_empty()
                || settings
                    .profile
                    .as_ref()
                    .is_some_and(|profile| profile.trim().is_empty())
                || settings.model.trim().is_empty()
                || settings.instructions.trim().is_empty()
            {
                return Err(host_error(
                    "Agent name, provider, profile, model, and fixed instructions cannot be empty",
                ));
            }
            let mut state = agents
                .lock()
                .map_err(|_| host_error("Automation status lock is poisoned"))?;
            if state.agents.iter().any(|agent| agent.name == settings.name) {
                return Err(host_error(format!(
                    "Agent {} is registered more than once",
                    settings.name
                )));
            }
            state.agents.push(settings);
            Ok(HostValue::None)
        },
    )?;

    let mcp_servers = Arc::clone(state);
    register_host(
        registry,
        "std.bench.config",
        "register_mcp",
        signature(
            [
                RuntimeType::String,
                RuntimeType::String,
                RuntimeType::String,
                RuntimeType::String,
            ],
            RuntimeType::None,
        ),
        "agent.configure",
        move |arguments| {
            let name = string_arg(arguments, 0)?;
            let transport = string_arg(arguments, 1)?.to_ascii_lowercase();
            let target = string_arg(arguments, 2)?;
            let arguments_json = string_arg(arguments, 3)?;
            if name.trim().is_empty()
                || target.trim().is_empty()
                || !matches!(transport.as_str(), "http" | "stdio")
            {
                return Err(host_error("Invalid MCP name, transport or destination"));
            }
            let parsed: Vec<String> = serde_json::from_str(&arguments_json)
                .map_err(|error| host_error(format!("Invalid MCP parameter JSON: {error}")))?;
            if transport == "http" && !parsed.is_empty() {
                return Err(host_error("HTTP MCP does not accept process parameters"));
            }
            let settings = McpSettings {
                name,
                transport,
                target,
                arguments: parsed,
            };
            let mut state = mcp_servers
                .lock()
                .map_err(|_| host_error("Automation status lock is poisoned"))?;
            if state
                .mcp_servers
                .iter()
                .any(|server| server.name == settings.name)
            {
                return Err(host_error(format!(
                    "MCP server {} is registered more than once",
                    settings.name
                )));
            }
            state.mcp_servers.push(settings);
            Ok(HostValue::None)
        },
    )?;

    let active_agent = Arc::clone(state);
    register_host(
        registry,
        "std.bench.config",
        "set_active_agent",
        signature([RuntimeType::String], RuntimeType::None),
        "agent.configure",
        move |arguments| {
            active_agent
                .lock()
                .map_err(|_| host_error("Automation status lock is poisoned"))?
                .active_agent = Some(string_arg(arguments, 0)?);
            Ok(HostValue::None)
        },
    )?;
    Ok(())
}

#[derive(Clone, Copy)]
enum RegistrationTarget {
    Command,
    Hook,
    Panel,
}

fn signature<const N: usize>(
    arguments: [RuntimeType; N],
    return_type: RuntimeType,
) -> HostCallSignature {
    HostCallSignature::new(arguments, return_type)
        .expect("Static Teal Bench host signature is valid")
}

fn string_arg(arguments: &[HostValue], index: usize) -> Result<String, HostError> {
    match arguments.get(index) {
        Some(HostValue::String(value)) => Ok(value.clone()),
        _ => Err(host_error(format!("Parameter {index} must be string"))),
    }
}

fn host_error(message: impl Into<String>) -> HostError {
    HostError::new(std::io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tempfile::tempdir;

    use crate::{ScriptKind, WorkbenchAutomationRuntime};

    #[derive(Default)]
    struct TestAuditSink(Mutex<Vec<teal_security::Event>>);

    impl teal_security::AuditSink for TestAuditSink {
        fn append(
            &self,
            event: &teal_security::Event,
        ) -> Result<teal_security::Receipt, teal_security::AuditError> {
            let mut events = self.0.lock().unwrap();
            events.push(event.clone());
            Ok(teal_security::Receipt {
                sequence: events.len() as u64,
                hash: format!("{:064x}", events.len()),
            })
        }
    }

    fn audit_context() -> teal_security::AuditContext {
        teal_security::AuditContext::new(Arc::new(TestAuditSink::default()))
    }

    #[test]
    fn execute_rejects_before_extension_work_without_an_audit_sink() {
        let directory = tempdir().unwrap();
        let script = directory.path().join("config.tl");
        std::fs::write(&script, "fn main() {}").unwrap();
        let runtime = WorkbenchAutomationRuntime::new(directory.path().join("state.json")).unwrap();
        let prepared = runtime.prepare(&script, ScriptKind::UserConfig).unwrap();

        let error = runtime.execute(prepared, "{}").unwrap_err();
        assert!(matches!(
            error,
            crate::RuntimeError::Audit(teal_security::AuditError::MissingSink)
        ));
    }

    #[test]
    fn bench_script_namespace_is_available_and_legacy_ide_identifiers_are_rejected() {
        let directory = tempdir().expect("temporary directory");
        let runtime = WorkbenchAutomationRuntime::new(directory.path().join("state.json"))
            .expect("runtime")
            .with_audit_context(audit_context());
        let bench = directory.path().join("bench.tl");
        std::fs::write(
            &bench,
            r#"host "std.bench.config" as config { fn state_set(arg0: string, arg1: string); }
               fn main() { config::state_set("namespace", "bench"); }"#,
        )
        .expect("bench script");
        let snapshot = runtime
            .execute(
                runtime
                    .prepare(&bench, ScriptKind::UserConfig)
                    .expect("Bench host namespace should prepare"),
                "{}",
            )
            .expect("Bench host namespace should execute");
        assert_eq!(snapshot.values["namespace"], "bench");

        let legacy_host = directory.path().join("legacy-host.tl");
        std::fs::write(
            &legacy_host,
            r#"host "teal.ide.config" as config { fn state_set(arg0: string, arg1: string); }
               fn main() { config::state_set("namespace", "ide"); }"#,
        )
        .expect("legacy host script");
        assert!(
            runtime
                .prepare(&legacy_host, ScriptKind::UserConfig)
                .is_err(),
            "legacy teal.ide host identity must not be registered"
        );

        let legacy_import = directory.path().join("legacy-import.tl");
        std::fs::write(
            &legacy_import,
            "import \"teal/ide/ui.tl\" as ui; fn main() { ui::event(); }",
        )
        .expect("legacy import script");
        assert!(
            runtime
                .prepare(&legacy_import, ScriptKind::UserConfig)
                .is_err(),
            "legacy teal/ide import must not be available"
        );
    }

    #[test]
    fn imported_source_changes_invalidate_the_complete_trust_fingerprint() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.tl");
        let import = dir.path().join("helper.tl");
        std::fs::write(&import, "pub fn value() -> int { return 1; }").unwrap();
        std::fs::write(&path, "import \"helper.tl\" as helper; import \"std/ui.tl\" as ui; task main() { print(helper::value()); }").unwrap();
        let runtime = WorkbenchAutomationRuntime::new(dir.path().join("state.json"))
            .unwrap()
            .with_audit_context(audit_context());
        let mut trust = crate::TrustStore::load(dir.path().join("trust.json")).unwrap();
        let first = runtime.prepare(&path, ScriptKind::WorkspaceConfig).unwrap();
        trust
            .approve(&path, first.trust_contents(), first.capabilities().clone())
            .unwrap();
        assert!(!matches!(
            runtime.trust_decision(&first, &trust),
            crate::TrustDecision::ApprovalRequired { .. }
        ));
        std::fs::write(&import, "pub fn value() -> int { return 2; }").unwrap();
        let second = runtime.prepare(&path, ScriptKind::WorkspaceConfig).unwrap();
        assert_ne!(first.trust_contents(), second.trust_contents());
        assert!(matches!(
            runtime.trust_decision(&second, &trust),
            crate::TrustDecision::ApprovalRequired { .. }
        ));
    }

    #[test]
    fn executes_config_registrations_state_and_builtin_teal_grammars() {
        let directory = tempdir().expect("temporary directory");
        let script = directory.path().join("config.teal");
        std::fs::write(
            &script,
            r#"
            host "std.bench.config" as config {
                fn register_command(arg0: string, arg1: string);
                fn register_agent(arg0: string, arg1: string, arg2: string, arg3: string, arg4: string);
                fn register_mcp(arg0: string, arg1: string, arg2: string, arg3: string);
                fn set_active_agent(arg0: string);
                fn state_set(arg0: string, arg1: string);
                fn set_session_restore(arg0: string);
            }
             fn main() {
                config::register_command("hello", "command.hello");
                config::register_agent("coding", "codex", "default", "test-model", "fixed instructions");
                config::register_mcp("docs", "http", "https://example.com/mcp", "[]");
                config::set_active_agent("coding");
                config::state_set("theme", "dark");
                config::set_session_restore("always");
            }
            "#,
        )
        .expect("Script test data");
        let runtime = WorkbenchAutomationRuntime::new(directory.path().join("state.json"))
            .expect("runtime")
            .with_audit_context(audit_context());
        let prepared = runtime
            .prepare(&script, ScriptKind::UserConfig)
            .expect("compile script");
        assert!(prepared.capabilities().contains("ui.panel"));
        assert!(prepared.capabilities().contains("agent.configure"));
        let snapshot = runtime.execute(prepared, "{}").expect("Execute script");
        assert_eq!(snapshot.commands[0].name, "hello");
        assert_eq!(snapshot.values["theme"], "dark");
        assert_eq!(snapshot.session_restore.as_deref(), Some("always"));
        assert_eq!(snapshot.agents[0].name, "coding");
        assert_eq!(snapshot.agents[0].provider, "codex");
        assert_eq!(snapshot.active_agent.as_deref(), Some("coding"));
        assert_eq!(snapshot.mcp_servers[0].name, "docs");
        assert_eq!(snapshot.exit_code, 0);

        let grammars = runtime
            .load_builtin_grammars()
            .expect("Built-in grammar script");
        assert_eq!(grammars.len(), 11);
        assert!(grammars.iter().any(|grammar| grammar.language == "rust"));
        for grammar in &grammars {
            let parser =
                teal_workbench_core::syntax::Parser::new(grammar).unwrap_or_else(|error| {
                    panic!(
                        "{} The grammar should be able to construct: {error}",
                        grammar.language
                    )
                });
            let tree = parser.parse("fn main() { return 42; }\n");
            assert_eq!(
                tree.error_count(),
                0,
                "{} Grammar should be able to accept lexical flow",
                grammar.language
            );
        }
    }
}
