//! 由调试调试器控制的外部终止 VM 进程。

use std::{
    collections::{BTreeSet, HashMap},
    error::Error,
    fmt,
    io::{self, BufReader},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        mpsc::{self, Receiver, TryRecvError},
    },
};

use serde_json::{Value as JsonValue, json};
use teal_compiler::{
    CompileOptions, ExtensionSet, LintLevel, LoadedSource, OptimizationLevel, RuntimeProfile,
    Source, SourceLoadError, SourceLoader, compile,
};
use teal_core::{ExecutionGrant, ExecutionRequest, ResourceAccess, ResourceGrant, ResourceScope};
use teal_host::NativeHostRuntime;
use teal_vm::{
    DebugSession, DebugStopReason, ExternalCallTarget, HostResource, HostRuntime,
    NominalTypeCatalog, StandardHostCall, StaticType, Value, Vm, VmDebugControl, VmDebugException,
    VmDebugState, VmDebugTarget, VmDebugVariable, VmDebugger,
};

use crate::{
    expression,
    wire::{Packet, read_packet, write_packet},
};

/// 一个终端自有调试器程序的配置。
pub struct DebuggeeOptions {
    /// 要连接的适应器端点, 或收听附件时的 `None`。
    pub connect: Option<SocketAddr>,
    /// 远程附加模式使用的捆绑端点。
    pub listen: Option<SocketAddr>,
    /// 任何调试命令之前需要共享的验证符号。
    pub token: String,
    /// 输入 Teal 源文件。
    pub program: PathBuf,
    /// 编译和加载使用的运行时配置档。
    pub runtime_profile: RuntimeProfile,
    /// 统一权限规则；调试入口不会从协议字段隐式扩大授权。
    pub permission_rules: Vec<teal_security::PermissionRule>,
    /// 发射配置中的可选编译器调试请求。
    pub compiler_debug: Option<bool>,
    /// 发射配置的有顺序的终端光线反转程序。
    pub lints: Vec<(String, LintLevel)>,
    /// 是否在第一个源代码指令之前停止发射。
    pub stop_on_entry: bool,
    /// 与Teal程序接触的参数。
    pub arguments: Vec<String>,
}

/// 在远程调试器控制下编译和执行 Teal 程序。
///
/// # Errors
/// 当源无法读取或编译, 安全握手失败, 或 VM 无法加载时返回错误。
pub fn run(options: DebuggeeOptions) -> Result<i32, Box<dyn std::error::Error>> {
    let mut stream = if let Some(endpoint) = options.connect {
        TcpStream::connect(endpoint)?
    } else if let Some(endpoint) = options.listen {
        let listener = TcpListener::bind(endpoint)?;
        eprintln!("teal-dap debuggee listening on {}", listener.local_addr()?);
        listener.accept()?.0
    } else {
        return Err("debuggee needs --connect or --listen".into());
    };
    stream.set_nodelay(true)?;
    let role = if options.connect.is_some() {
        crate::authentication::Role::Connector
    } else {
        crate::authentication::Role::Listener
    };
    let reader = crate::authentication::authenticate(&mut stream, &options.token, role)?;

    let program_path = options.program.canonicalize().unwrap_or(options.program);
    let contents = std::fs::read_to_string(&program_path)?;
    let source = Source::new(program_path.to_string_lossy(), contents);
    let mut compile_options =
        CompileOptions::default().with_runtime_profile(options.runtime_profile.clone());
    if let Some(debug) = options.compiler_debug {
        compile_options = compile_options.with_debug(debug);
    }
    for (name, level) in options.lints {
        compile_options = compile_options.with_lint(name, level);
    }
    // 宿主最终配置必须保证调试正确性：启动或源码请求
    // 不能重新启用优化，也不能省略绑定快照。
    compile_options = compile_options
        .with_optimization(OptimizationLevel::None)
        .with_debug_info(true);
    let output = compile(source, &DebugSourceLoader, compile_options).map_err(|source| {
        DapCompileError {
            code: source.diagnostic_code(),
            source,
        }
    })?;
    for warning in &output.warnings {
        eprintln!("warning[{}]: {}", warning.lint.name(), warning.message);
    }
    let debug_session = DebugSession::new(&output.program.source_map, options.stop_on_entry);
    let mut grant = ExecutionGrant::core_only("teal-dap/permissions-v3");
    let workspace = std::env::current_dir()?;
    for rule in options.permission_rules.iter().filter(|rule| {
        rule.effect == teal_security::PermissionEffect::Allow
            && debug_workspace_subject(&rule.subject, &workspace)
            && !options.permission_rules.iter().any(|blocked| {
                blocked.effect != teal_security::PermissionEffect::Allow
                    && blocked.subject == rule.subject
                    && blocked.operation == rule.operation
                    && (blocked.resource.covers(&rule.resource)
                        || rule.resource.covers(&blocked.resource))
            })
    }) {
        grant = allow_debug_rule(grant, rule)?;
    }
    let grant = Arc::new(grant);
    let mut host = NativeHostRuntime::builder()
        .trusted()
        .trusted()
        .arguments(options.arguments)
        .echo(true)
        .authorization(grant.clone())
        .build()?;
    let used_calls = output
        .program
        .instructions
        .iter()
        .filter_map(|instruction| match instruction {
            teal_abi::Instruction::CallExternal(_, id) => Some(id.as_usize()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let nominal = Arc::new(NominalTypeCatalog::from_program(&output.program));
    let context = host
        .binding_context()
        .with_type_catalog(Arc::clone(&nominal));
    let mut required_capabilities = BTreeSet::new();
    for (index, call) in output.program.external_calls.iter().enumerate() {
        if !used_calls.contains(&index) {
            continue;
        }
        let ExternalCallTarget::Host { module, function } = &call.target else {
            continue;
        };
        if let Some(capability) = StandardHostCall::from_module_function(module, function)
            .and_then(StandardHostCall::required_capability)
        {
            required_capabilities.insert(capability.to_owned());
            continue;
        }
        let binding = host.bind_host_call(call, &nominal, context.clone())?;
        if !binding.capability().is_empty() {
            required_capabilities.insert(binding.capability().to_owned());
        }
    }
    let required_extensions = output
        .program
        .required_extensions
        .iter()
        .map(|requirement| requirement.extension)
        .collect::<ExtensionSet>();
    let request = ExecutionRequest::new(required_extensions, required_capabilities);
    grant.authorize(&request)?;
    let mut vm = Vm::load_with_host_runtime(
        output.program,
        teal_vm::VmOptions::for_profile(options.runtime_profile),
        &mut host,
    )?;
    let mut debugger = RemoteDebugger::new(stream, reader, debug_session)?;
    let result = vm.execute_debugged(&mut host, &mut debugger);
    match result {
        Ok(outcome) => {
            debugger.event("exited", json!({ "exitCode": outcome.exit_code }));
            debugger.event("terminated", json!({}));
            Ok(outcome.exit_code)
        }
        Err(_error) if debugger.terminated => {
            debugger.event("terminated", json!({}));
            Ok(0)
        }
        Err(error) => {
            debugger.event(
                "output",
                json!({ "category": "stderr", "output": format!("{error}\n") }),
            );
            debugger.event("exited", json!({ "exitCode": 1 }));
            debugger.event("terminated", json!({}));
            Err(error.into())
        }
    }
}

/// 编译失败在 DAP 启动边界上的稳定包装；保留完整编译错误作为 source，
/// 同时让 adapter 进程输出可供编辑器识别的诊断码。
#[derive(Debug)]
struct DapCompileError {
    code: &'static str,
    source: teal_compiler::CompileError,
}

impl fmt::Display for DapCompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "error[{}]: {}", self.code, self.source.message)
    }
}

impl Error for DapCompileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

fn allow_debug_rule(
    mut grant: ExecutionGrant,
    rule: &teal_security::PermissionRule,
) -> Result<ExecutionGrant, io::Error> {
    let (capability, mut access) = match rule.operation.as_str() {
        "filesystem.read" => (Some("filesystem.inspect"), ResourceAccess::Read),
        "filesystem.write" | "filesystem.remove" => {
            (Some("filesystem.mutate"), ResourceAccess::Write)
        }
        "environment.read" => (Some("environment.read"), ResourceAccess::Read),
        "clock.read" => (Some("clock.read"), ResourceAccess::Read),
        "random.generate" => (Some("crypto.random"), ResourceAccess::Read),
        "process.inspect" => (Some("process.inspect"), ResourceAccess::Read),
        "process.spawn" => (Some("process.execute"), ResourceAccess::Execute),
        "process.control" => (Some("process.control"), ResourceAccess::Write),
        "network.connect" => (Some("network.http"), ResourceAccess::Network),
        "network.listen" => (Some("network.oauth_callback"), ResourceAccess::Network),
        _ => (None, ResourceAccess::Any),
    };
    if let Some(capability) = capability {
        grant = grant.allow_capability(capability);
    }
    if rule.operation.as_str() == "process.inspect"
        && matches!(
            &rule.resource,
            teal_security::PermissionResource::Path { .. }
                | teal_security::PermissionResource::Process { .. }
        )
    {
        access = ResourceAccess::Execute;
    }
    let (resource, scope) = match &rule.resource {
        teal_security::PermissionResource::Any
        | teal_security::PermissionResource::Dylib { .. }
        | teal_security::PermissionResource::Model { .. } => return Ok(grant),
        teal_security::PermissionResource::Path { path, scope } => {
            (HostResource::Path(path.clone()), debug_scope(*scope))
        }
        teal_security::PermissionResource::Environment { name } => (
            HostResource::Environment(name.clone()),
            ResourceScope::Exact,
        ),
        teal_security::PermissionResource::Network {
            origin,
            path,
            scope,
        } => (
            HostResource::Network(format!("{origin}{path}")),
            debug_scope(*scope),
        ),
        teal_security::PermissionResource::Process { executable, .. } => {
            (HostResource::Path(executable.clone()), ResourceScope::Exact)
        }
        teal_security::PermissionResource::Application {
            namespace,
            identity,
            scope,
        } => (
            HostResource::Application {
                namespace: namespace.clone(),
                identity: identity.clone(),
            },
            debug_scope(*scope),
        ),
    };
    if rule.operation.as_str() == "process.control"
        && matches!(&resource, HostResource::Application { .. })
    {
        grant = grant.allow_resource(ResourceGrant {
            access: ResourceAccess::Read,
            resource: resource.clone(),
            scope,
        });
        access = ResourceAccess::Write;
    }
    Ok(grant.allow_resource(ResourceGrant {
        access,
        resource,
        scope,
    }))
}

fn debug_workspace_subject(subject: &teal_security::PermissionSubject, workspace: &Path) -> bool {
    let teal_security::PermissionSubject::Workspace { root } = subject else {
        return false;
    };
    let root = root.canonicalize().unwrap_or_else(|_| root.clone());
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    #[cfg(windows)]
    {
        root.to_string_lossy()
            .replace('/', "\\")
            .trim_start_matches(r"\\?\")
            .eq_ignore_ascii_case(
                workspace
                    .to_string_lossy()
                    .replace('/', "\\")
                    .trim_start_matches(r"\\?\"),
            )
    }
    #[cfg(not(windows))]
    {
        root == workspace
    }
}

const fn debug_scope(scope: teal_security::ResourceScope) -> ResourceScope {
    match scope {
        teal_security::ResourceScope::Exact => ResourceScope::Exact,
        teal_security::ResourceScope::Descendants => ResourceScope::Descendants,
    }
}

struct DebugSourceLoader;

impl SourceLoader for DebugSourceLoader {
    fn load(&self, importer: &Path, import: &str) -> Result<LoadedSource, SourceLoadError> {
        if let Some(source) = package_catalog()?.load_package(importer, import)? {
            return Ok(source);
        }
        let local = importer
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(import);
        let path = local.canonicalize().map_err(|error| SourceLoadError {
            message: format!("cannot resolve import {import}: {error}"),
        })?;
        let contents = std::fs::read_to_string(&path).map_err(|error| SourceLoadError {
            message: format!("cannot read {}: {error}", path.display()),
        })?;
        Ok(LoadedSource {
            identity: path.clone(),
            source: Source::new(path.to_string_lossy(), contents),
            precompiled: None,
        })
    }
}

fn package_catalog() -> Result<&'static teal_packages::PackageCatalog, SourceLoadError> {
    static CATALOG: OnceLock<Result<teal_packages::PackageCatalog, String>> = OnceLock::new();
    CATALOG
        .get_or_init(|| {
            teal_packages::PackageCatalog::discover().map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|message| SourceLoadError {
            message: message.clone(),
        })
}

struct RemoteDebugger {
    receiver: Receiver<Packet>,
    writer: TcpStream,
    session: DebugSession,
    configured: bool,
    handles: HashMap<u64, DebugHandle>,
    next_handle: u64,
    exception: Option<VmDebugException>,
    terminated: bool,
}

#[derive(Clone)]
struct DebugHandle {
    value: Value,
    static_type: StaticType,
}

impl RemoteDebugger {
    fn new(
        stream: TcpStream,
        mut reader: BufReader<TcpStream>,
        session: DebugSession,
    ) -> io::Result<Self> {
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("teal-debug-command-reader".to_owned())
            .spawn(move || {
                while let Ok(Some(packet)) = read_packet(&mut reader) {
                    if sender.send(packet).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            receiver,
            writer: stream,
            session,
            configured: false,
            handles: HashMap::new(),
            next_handle: 10_000,
            exception: None,
            terminated: false,
        })
    }

    fn event(&mut self, event: &str, body: JsonValue) {
        let _ = write_packet(
            &mut self.writer,
            &Packet::Event {
                event: event.to_owned(),
                body,
            },
        );
    }

    fn response(&mut self, id: u64, success: bool, body: JsonValue, message: Option<String>) {
        let _ = write_packet(
            &mut self.writer,
            &Packet::Response {
                id,
                success,
                body,
                message,
            },
        );
    }

    fn process_running(&mut self) -> VmDebugControl {
        loop {
            match self.receiver.try_recv() {
                Ok(Packet::Command {
                    id,
                    command,
                    arguments,
                }) => {
                    if self.running_command(id, &command, &arguments) == VmDebugControl::Terminate {
                        return VmDebugControl::Terminate;
                    }
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) => return VmDebugControl::Continue,
                Err(TryRecvError::Disconnected) => {
                    self.terminated = true;
                    return VmDebugControl::Terminate;
                }
            }
        }
    }

    fn running_command(&mut self, id: u64, command: &str, arguments: &JsonValue) -> VmDebugControl {
        match command {
            "setBreakpoints" => self.set_breakpoints(id, arguments),
            "setExceptionBreakpoints" => {
                let filters = arguments
                    .get("filters")
                    .and_then(JsonValue::as_array)
                    .cloned()
                    .unwrap_or_default();
                self.session.set_exception_breakpoints(
                    filters
                        .iter()
                        .any(|filter| filter.as_str() == Some("caught")),
                    filters
                        .iter()
                        .any(|filter| filter.as_str() == Some("uncaught")),
                );
                self.response(id, true, json!({}), None);
            }
            "configurationDone" => {
                self.configured = true;
                self.response(id, true, json!({}), None);
            }
            "pause" => {
                self.session.request_pause();
                self.response(id, true, json!({}), None);
            }
            "disconnect" | "terminate" => {
                self.terminated = true;
                self.response(id, true, json!({}), None);
                return VmDebugControl::Terminate;
            }
            _ => self.response(id, false, json!({}), Some("program is running".to_owned())),
        }
        VmDebugControl::Continue
    }

    fn wait_for_configuration(&mut self) -> VmDebugControl {
        while !self.configured {
            match self.receiver.recv() {
                Ok(Packet::Command {
                    id,
                    command,
                    arguments,
                }) => {
                    if self.running_command(id, &command, &arguments) == VmDebugControl::Terminate {
                        return VmDebugControl::Terminate;
                    }
                }
                Ok(_) => {}
                Err(_) => {
                    self.terminated = true;
                    return VmDebugControl::Terminate;
                }
            }
        }
        VmDebugControl::Continue
    }

    fn should_stop(&mut self, state: &VmDebugState<'_>) -> Option<&'static str> {
        self.session
            .before_instruction(state)
            .map(DebugStopReason::name)
    }

    fn pause(&mut self, state: &mut VmDebugState<'_>, reason: &str) -> VmDebugControl {
        self.session.mark_paused(state);
        self.handles.clear();
        self.next_handle = 10_000;
        self.event(
            "stopped",
            json!({ "reason": reason, "threadId": 1, "allThreadsStopped": true }),
        );
        loop {
            match self.receiver.recv() {
                Ok(Packet::Command {
                    id,
                    command,
                    arguments,
                }) => {
                    let control = self.paused_command(state, id, &command, &arguments);
                    if control != PauseControl::Stay {
                        return if control == PauseControl::Terminate {
                            self.terminated = true;
                            VmDebugControl::Terminate
                        } else {
                            VmDebugControl::Continue
                        };
                    }
                }
                Ok(_) => {}
                Err(_) => {
                    self.terminated = true;
                    return VmDebugControl::Terminate;
                }
            }
        }
    }

    fn paused_command(
        &mut self,
        state: &mut VmDebugState<'_>,
        id: u64,
        command: &str,
        arguments: &JsonValue,
    ) -> PauseControl {
        match command {
            "continue" => {
                self.session.continue_run();
                self.response(id, true, json!({ "allThreadsContinued": true }), None);
                PauseControl::Resume
            }
            "next" | "stepIn" | "stepOut" => {
                match command {
                    "next" => self.session.next(state),
                    "stepIn" => self.session.step_in(state),
                    _ => self.session.step_out(state),
                }
                self.response(id, true, json!({}), None);
                PauseControl::Resume
            }
            "disconnect" | "terminate" => {
                self.response(id, true, json!({}), None);
                PauseControl::Terminate
            }
            "stackTrace" => {
                let frames = state.frames();
                let stack_frames = frames
                    .iter()
                    .rev()
                    .map(|frame| {
                        let source = frame.source_name.clone().unwrap_or_else(|| "<bytecode>".to_owned());
                        let line = self.session.source_line(&source, frame.start.unwrap_or(0));
                        json!({
                            "id": frame.index + 1,
                            "name": frame.name,
                            "line": line,
                            "column": 1,
                            "source": { "name": Path::new(&source).file_name().and_then(|name| name.to_str()).unwrap_or(&source), "path": source }
                        })
                    })
                    .collect::<Vec<_>>();
                self.response(
                    id,
                    true,
                    json!({ "stackFrames": stack_frames, "totalFrames": frames.len() }),
                    None,
                );
                PauseControl::Stay
            }
            "scopes" => {
                let frame = arguments
                    .get("frameId")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(1);
                self.response(
                    id,
                    true,
                    json!({ "scopes": [{
                    "name": "Locals", "presentationHint": "locals", "variablesReference": frame,
                    "expensive": false
                }] }),
                    None,
                );
                PauseControl::Stay
            }
            "variables" => {
                let reference = arguments
                    .get("variablesReference")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0);
                match self.variables(state, reference) {
                    Ok(variables) => {
                        self.response(id, true, json!({ "variables": variables }), None)
                    }
                    Err(message) => self.response(id, false, json!({}), Some(message)),
                }
                PauseControl::Stay
            }
            "evaluate" => {
                let expression_text = arguments
                    .get("expression")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default();
                let frame = arguments
                    .get("frameId")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(1)
                    .saturating_sub(1) as usize;
                match expression::parse(expression_text).and_then(|expression| {
                    state
                        .evaluate(frame, &expression)
                        .map_err(|error| error.to_string())
                }) {
                    Ok(variable) => {
                        let reference = self.remember(
                            variable.value,
                            variable.static_type.clone(),
                            variable.has_children,
                        );
                        self.response(
                            id,
                            true,
                            json!({
                                "result": variable.display, "type": variable.declared_type,
                                "variablesReference": reference
                            }),
                            None,
                        );
                    }
                    Err(message) => self.response(id, false, json!({}), Some(message)),
                }
                PauseControl::Stay
            }
            "setVariable" => {
                let reference = arguments
                    .get("variablesReference")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0);
                let name = arguments
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default();
                let value = arguments
                    .get("value")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default();
                match self.set_variable(state, reference, name, value) {
                    Ok(variable) => {
                        let child_reference = self.remember(
                            variable.value,
                            variable.static_type.clone(),
                            variable.has_children,
                        );
                        self.response(
                            id,
                            true,
                            json!({
                                "value": variable.display, "type": variable.declared_type,
                                "variablesReference": child_reference
                            }),
                            None,
                        );
                    }
                    Err(message) => self.response(id, false, json!({}), Some(message)),
                }
                PauseControl::Stay
            }
            "source" => {
                let source = arguments
                    .get("source")
                    .and_then(|source| source.get("path"))
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default();
                match self.session.source_content(source) {
                    Some(content) => self.response(
                        id,
                        true,
                        json!({ "content": content, "mimeType": "text/x-teal" }),
                        None,
                    ),
                    None => self.response(id, false, json!({}), Some("unknown source".to_owned())),
                }
                PauseControl::Stay
            }
            "exceptionInfo" => {
                let body = self.exception.as_ref().map_or_else(
                    || json!({ "exceptionId": "none", "breakMode": "never" }),
                    |exception| {
                        json!({
                            "exceptionId": exception.kind, "description": exception.message,
                            "breakMode": if exception.will_be_caught { "always" } else { "unhandled" }
                        })
                    },
                );
                self.response(id, true, body, None);
                PauseControl::Stay
            }
            "setBreakpoints" => {
                self.set_breakpoints(id, arguments);
                PauseControl::Stay
            }
            "setExceptionBreakpoints" => {
                self.running_command(id, command, arguments);
                PauseControl::Stay
            }
            _ => {
                self.response(
                    id,
                    false,
                    json!({}),
                    Some(format!("unsupported debug command `{command}`")),
                );
                PauseControl::Stay
            }
        }
    }

    fn variables(
        &mut self,
        state: &VmDebugState<'_>,
        reference: u64,
    ) -> Result<Vec<JsonValue>, String> {
        let variables = if (1..10_000).contains(&reference) {
            state
                .variables(reference.saturating_sub(1) as usize)
                .map_err(|error| error.to_string())?
        } else {
            let handle = self
                .handles
                .get(&reference)
                .cloned()
                .ok_or_else(|| "stale variablesReference".to_owned())?;
            state.children(handle.value, &handle.static_type)
        };
        Ok(self.render_variables(variables))
    }

    fn render_variables(&mut self, variables: Vec<VmDebugVariable>) -> Vec<JsonValue> {
        variables
            .into_iter()
            .map(|variable| {
                let reference = self.remember(
                    variable.value,
                    variable.static_type.clone(),
                    variable.has_children,
                );
                json!({
                    "name": variable.name, "value": variable.display, "type": variable.declared_type,
                    "variablesReference": reference
                })
            })
            .collect()
    }

    fn remember(&mut self, value: Value, static_type: StaticType, has_children: bool) -> u64 {
        if !has_children {
            return 0;
        }
        let reference = self.next_handle;
        self.next_handle += 1;
        self.handles
            .insert(reference, DebugHandle { value, static_type });
        reference
    }

    fn set_variable(
        &mut self,
        state: &mut VmDebugState<'_>,
        reference: u64,
        name: &str,
        expression_text: &str,
    ) -> Result<VmDebugVariable, String> {
        let variables = if (1..10_000).contains(&reference) {
            state
                .variables(reference.saturating_sub(1) as usize)
                .map_err(|error| error.to_string())?
        } else {
            let handle = self
                .handles
                .get(&reference)
                .cloned()
                .ok_or_else(|| "stale variablesReference".to_owned())?;
            state.children(handle.value, &handle.static_type)
        };
        let target: VmDebugTarget = variables
            .into_iter()
            .find(|variable| variable.name == name)
            .and_then(|variable| variable.target)
            .ok_or_else(|| format!("variable `{name}` is read-only or unknown"))?;
        let expression = expression::parse(expression_text)?;
        state
            .set_target(&target, &expression)
            .map_err(|error| error.to_string())
    }

    fn set_breakpoints(&mut self, id: u64, arguments: &JsonValue) {
        let source = arguments
            .get("source")
            .and_then(|source| source.get("path").or_else(|| source.get("name")))
            .and_then(JsonValue::as_str)
            .unwrap_or_default();
        let normalized = DebugSession::normalize_source(source);
        let requested = arguments
            .get("breakpoints")
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default();
        let requested_lines = requested
            .iter()
            .filter_map(|breakpoint| breakpoint.get("line").and_then(JsonValue::as_u64))
            .filter_map(|line| u32::try_from(line).ok())
            .collect::<Vec<_>>();
        let response = self
            .session
            .set_breakpoints(&normalized, requested_lines)
            .into_iter()
            .map(|breakpoint| {
                json!({
                    "verified": breakpoint.verified,
                    "line": breakpoint.line,
                    "message": if breakpoint.verified { JsonValue::Null } else { json!("line is outside the source") }
                })
            })
            .collect::<Vec<_>>();
        self.response(id, true, json!({ "breakpoints": response }), None);
    }
}

impl VmDebugger for RemoteDebugger {
    const ENABLED: bool = true;

    fn before_instruction(&mut self, state: &mut VmDebugState<'_>) -> VmDebugControl {
        if self.process_running() == VmDebugControl::Terminate
            || self.wait_for_configuration() == VmDebugControl::Terminate
        {
            return VmDebugControl::Terminate;
        }
        let stop = self.should_stop(state);
        stop.map_or(VmDebugControl::Continue, |reason| self.pause(state, reason))
    }

    fn exception(
        &mut self,
        state: &mut VmDebugState<'_>,
        exception: &VmDebugException,
    ) -> VmDebugControl {
        self.exception = Some(exception.clone());
        self.session
            .exception(exception)
            .map_or(VmDebugControl::Continue, |reason| {
                self.pause(state, reason.name())
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PauseControl {
    Stay,
    Resume,
    Terminate,
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufReader, Write},
        net::TcpListener,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde_json::json;

    use super::{DapCompileError, DebugSourceLoader, DebuggeeOptions, compile, run};
    use teal_compiler::SourceLoader;
    use crate::wire::{Packet, read_packet, write_packet};

    #[test]
    fn compile_failures_preserve_stable_diagnostic_codes() {
        let error = compile(
            teal_compiler::Source::new("invalid.tl", "fn main( {"),
            &DebugSourceLoader,
            teal_compiler::CompileOptions::default(),
        )
        .expect_err("invalid source should fail compilation");
        let wrapped = DapCompileError {
            code: error.diagnostic_code(),
            source: error,
        };
        assert!(wrapped.to_string().starts_with("error[teal::parse]:"));
    }

    #[test]
    fn authenticated_debuggee_pauses_serves_stack_and_resumes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap();
        let path = temporary_source();
        std::fs::write(&path, "task main() { print(\"native debuggee\"); }").unwrap();
        let debuggee_path = path.clone();
        let thread = std::thread::spawn(move || {
            run(DebuggeeOptions {
                connect: Some(endpoint),
                listen: None,
                token: "test-token".to_owned(),
                program: debuggee_path,

                runtime_profile: teal_compiler::RuntimeProfile::agent(),
                permission_rules: Vec::new(),
                compiler_debug: None,
                lints: Vec::new(),
                stop_on_entry: true,
                arguments: Vec::new(),
            })
            .map_err(|error| error.to_string())
        });

        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = crate::authentication::authenticate(
            &mut stream,
            "test-token",
            crate::authentication::Role::Listener,
        )
        .unwrap();
        write_packet(
            &mut stream,
            &Packet::Command {
                id: 0,
                command: "setBreakpoints".to_owned(),
                arguments: json!({
                    "source": { "path": path.to_string_lossy() },
                    "breakpoints": [{ "line": 1 }, { "line": 2 }, { "line": 1 }]
                }),
            },
        )
        .unwrap();
        match read_packet(&mut reader).unwrap() {
            Some(Packet::Response {
                id: 0,
                success: true,
                body,
                ..
            }) => {
                let breakpoints = body["breakpoints"].as_array().unwrap();
                assert_eq!(
                    breakpoints
                        .iter()
                        .map(|breakpoint| breakpoint["line"].as_u64())
                        .collect::<Vec<_>>(),
                    [Some(1), Some(2), Some(1)]
                );
                assert_eq!(
                    breakpoints
                        .iter()
                        .map(|breakpoint| breakpoint["verified"].as_bool())
                        .collect::<Vec<_>>(),
                    [Some(true), Some(false), Some(true)]
                );
            }
            packet => panic!("unexpected breakpoint response: {packet:?}"),
        }
        write_packet(
            &mut stream,
            &Packet::Command {
                id: 1,
                command: "configurationDone".to_owned(),
                arguments: json!({}),
            },
        )
        .unwrap();
        assert!(matches!(
            read_packet(&mut reader).unwrap(),
            Some(Packet::Response {
                id: 1,
                success: true,
                ..
            })
        ));
        assert!(matches!(
            read_packet(&mut reader).unwrap(),
            Some(Packet::Event { event, .. }) if event == "stopped"
        ));
        write_packet(
            &mut stream,
            &Packet::Command {
                id: 2,
                command: "stackTrace".to_owned(),
                arguments: json!({}),
            },
        )
        .unwrap();
        match read_packet(&mut reader).unwrap() {
            Some(Packet::Response {
                id: 2,
                success: true,
                body,
                ..
            }) => {
                assert_eq!(body["stackFrames"][0]["name"], "main");
            }
            packet => panic!("unexpected stack response: {packet:?}"),
        }
        write_packet(
            &mut stream,
            &Packet::Command {
                id: 3,
                command: "continue".to_owned(),
                arguments: json!({}),
            },
        )
        .unwrap();
        assert!(matches!(
            read_packet(&mut reader).unwrap(),
            Some(Packet::Response {
                id: 3,
                success: true,
                ..
            })
        ));
        let mut exited = false;
        let mut terminated = false;
        while let Some(packet) = read_packet(&mut reader).unwrap() {
            match packet {
                Packet::Event { event, .. } if event == "exited" => exited = true,
                Packet::Event { event, .. } if event == "terminated" => {
                    terminated = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(exited && terminated);
        assert_eq!(thread.join().unwrap().unwrap(), 0);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn debuggee_resolves_the_embedded_standard_library() {
        let source = DebugSourceLoader
            .load(std::path::Path::new("missing/main.tl"), "std/ui.tl")
            .expect("debugger should resolve the built-in standard module");
        assert_eq!(source.identity, std::path::Path::new("std/ui.tl"));
        assert!(source.source.contents().contains("pub enum Component"));
    }

    fn temporary_source() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("teal-dap-{nonce}-{}.tl", std::process::id()))
    }
}
