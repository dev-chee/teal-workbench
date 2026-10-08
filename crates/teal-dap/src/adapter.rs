//! 《标准输入调试器协议》终点。

use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, BufWriter, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

use serde_json::{Value, json};

fn launch_csv(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => Some(value.clone()),
        Value::Array(values) => Some(
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(","),
        ),
        _ => None,
    }
}

fn validate_launch_arguments(arguments: &Value) -> io::Result<()> {
    launch_terminal_kind(arguments)?;
    let removed_modules = ["allow", "Modules"].concat();
    if arguments.get("entry").is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "launch field `entry` was removed; use fn main() or task main()",
        ));
    }
    if [
        "allowCapabilities",
        "allowReadFiles",
        "allowReadDirectories",
        "allowWriteFiles",
        "allowWriteDirectories",
        "allowExecutables",
        "allowEnvironment",
    ]
    .iter()
    .any(|field| arguments.get(*field).is_some())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "legacy allow* launch fields were removed; use permissionFile",
        ));
    }
    if arguments.get("features").is_some()
        || arguments.get("extensions").is_some()
        || arguments.get(&removed_modules).is_some()
    {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "launch fields `features`, `extensions`, and `{removed_modules}` were removed; use `runtimeProfile` and `permissionFile`"
            ),
        ))
    } else {
        Ok(())
    }
}

/// 将编辑器终端选项转换为 DAP 值；省略时保留独立适配器的外部终端行为。
fn launch_terminal_kind(arguments: &Value) -> io::Result<&'static str> {
    match arguments.get("console") {
        None => Ok("external"),
        Some(Value::String(value)) if value == "integratedTerminal" => Ok("integrated"),
        Some(Value::String(value)) if value == "externalTerminal" => Ok("external"),
        Some(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "launch `console` must be integratedTerminal or externalTerminal",
        )),
    }
}

use crate::wire::{Packet, read_packet, write_packet};

type PendingResponse = mpsc::Sender<Result<Value, String>>;
type PendingResponses = Arc<Mutex<HashMap<u64, PendingResponse>>>;

/// 运行 Teal 调试调试器,高于标准输入和输出。
///
/// # Errors
/// 返回错误的 DAP 设置、终端发射失败或破损调试器传输的 I/O 错误。
pub fn run_stdio() -> io::Result<()> {
    let stdin = io::stdin();
    let output = DapOutput::new(io::stdout());
    let mut reader = BufReader::new(stdin.lock());
    let mut adapter = Adapter {
        output,
        connection: None,
    };
    adapter.run(&mut reader)
}

struct Adapter {
    output: DapOutput,
    connection: Option<Connection>,
}

impl Adapter {
    fn run(&mut self, reader: &mut impl BufRead) -> io::Result<()> {
        while let Some(message) = read_dap(reader)? {
            if message.get("type").and_then(Value::as_str) != Some("request") {
                continue;
            }
            let request_seq = message.get("seq").and_then(Value::as_u64).unwrap_or(0);
            let command = message
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let arguments = message
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let span = tracing::info_span!(
                "teal.dap.request",
                request.id = i64::try_from(request_seq).unwrap_or(i64::MAX),
                operation.result = tracing::field::Empty
            );
            let _entered = span.enter();
            match command {
                "initialize" => self.respond(request_seq, command, true, capabilities(), None)?,
                "launch" => self.launch(reader, request_seq, &arguments)?,
                "attach" => self.attach(request_seq, &arguments)?,
                "threads" => self.respond(
                    request_seq,
                    command,
                    true,
                    json!({ "threads": [{ "id": 1, "name": "Teal main" }] }),
                    None,
                )?,
                "disconnect" | "terminate" => {
                    let result = self.remote(command, arguments);
                    let (success, body, message) = unpack(result);
                    self.respond(request_seq, command, success, body, message)?;
                    if command == "disconnect" {
                        break;
                    }
                }
                "setBreakpoints"
                | "setExceptionBreakpoints"
                | "configurationDone"
                | "stackTrace"
                | "scopes"
                | "variables"
                | "evaluate"
                | "setVariable"
                | "continue"
                | "next"
                | "stepIn"
                | "stepOut"
                | "pause"
                | "exceptionInfo"
                | "source" => {
                    let result = self.remote(command, arguments);
                    let (success, body, message) = unpack(result);
                    self.respond(request_seq, command, success, body, message)?;
                }
                _ => self.respond(
                    request_seq,
                    command,
                    false,
                    json!({}),
                    Some(format!("unsupported DAP request `{command}`")),
                )?,
            }
        }
        Ok(())
    }

    fn launch(
        &mut self,
        reader: &mut impl BufRead,
        request_seq: u64,
        arguments: &Value,
    ) -> io::Result<()> {
        if let Err(error) = validate_launch_arguments(arguments) {
            return self.respond(
                request_seq,
                "launch",
                false,
                json!({}),
                Some(error.to_string()),
            );
        }
        let terminal_kind = launch_terminal_kind(arguments)?;
        let program = arguments
            .get("program")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "launch requires `program`")
            })?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let endpoint = listener.local_addr()?;
        let token = random_token()?;
        let executable = std::env::current_exe()?;
        let mut terminal_args = vec![
            executable.to_string_lossy().into_owned(),
            "--debuggee".to_owned(),
            "--connect".to_owned(),
            endpoint.to_string(),
            "--token".to_owned(),
            token.clone(),
            "--program".to_owned(),
            program.to_owned(),
        ];
        if arguments
            .get("stopOnEntry")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            terminal_args.push("--stop-on-entry".to_owned());
        }
        if let Some(profile) = arguments.get("runtimeProfile").and_then(Value::as_str) {
            terminal_args.push("--runtime-profile".to_owned());
            terminal_args.push(profile.to_owned());
        }
        if let Some(permission_file) = arguments.get("permissionFile").and_then(Value::as_str) {
            terminal_args.push("--permission-file".to_owned());
            terminal_args.push(permission_file.to_owned());
        }
        if let Some(optimization) = arguments.get("optimization").and_then(Value::as_str) {
            terminal_args.push("--optimization".to_owned());
            terminal_args.push(optimization.to_owned());
        }
        if let Some(debug) = arguments.get("compilerDebug").and_then(Value::as_bool) {
            terminal_args.push("--compiler-debug".to_owned());
            terminal_args.push(debug.to_string());
        }
        if let Some(debug_info) = arguments.get("debugInfo").and_then(Value::as_bool) {
            terminal_args.push("--debug-info".to_owned());
            terminal_args.push(debug_info.to_string());
        }
        for (key, flag) in [
            ("allow", "--allow"),
            ("warn", "--warn"),
            ("deny", "--deny"),
            ("forbid", "--forbid"),
        ] {
            if let Some(values) = launch_csv(arguments.get(key)) {
                terminal_args.push(flag.to_owned());
                terminal_args.push(values);
            }
        }
        if let Some(args) = arguments.get("args").and_then(Value::as_array) {
            terminal_args.push("--".to_owned());
            terminal_args.extend(args.iter().filter_map(Value::as_str).map(str::to_owned));
        }
        let reverse_seq = self.output.next_sequence();
        let audit_event = teal_security::Event::new(
            teal_security::Action::DebugLaunch,
            teal_security::Phase::Intent,
            teal_security::Outcome::Pending,
            teal_security::new_correlation_id(),
        )
        .map_err(|error| io::Error::new(io::ErrorKind::PermissionDenied, error.to_string()))?
        .with_actor(teal_security::Actor::User);
        let audit = teal_security::process_context()
            .begin(audit_event)
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("Audit unavailable; debug launch was not requested: {error}"),
                )
            })?;
        self.output.write(&json!({
            "seq": reverse_seq,
            "type": "request",
            "command": "runInTerminal",
            "arguments": {
                "kind": terminal_kind,
                "title": "Teal Debuggee",
                "cwd": arguments.get("cwd").and_then(Value::as_str).unwrap_or("."),
                "args": terminal_args
            }
        }))?;

        loop {
            let response = read_dap(reader)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "client closed before runInTerminal response",
                )
            })?;
            if response.get("type").and_then(Value::as_str) == Some("response")
                && response.get("request_seq").and_then(Value::as_u64) == Some(reverse_seq)
            {
                if !response
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    audit.finish(teal_security::Outcome::Denied).map_err(|error| {
                        io::Error::other(format!(
                            "Audit result unavailable; debug launch state may be unknown: {error}"
                        ))
                    })?;
                    self.respond(
                        request_seq,
                        "launch",
                        false,
                        json!({}),
                        Some("client rejected runInTerminal".to_owned()),
                    )?;
                    return Ok(());
                }
                break;
            }
        }

        audit
            .finish(teal_security::Outcome::Succeeded)
            .map_err(|error| {
                io::Error::other(format!(
                    "Audit result unavailable; debug launch state may be unknown: {error}"
                ))
            })?;

        listener.set_nonblocking(true)?;
        let started = std::time::Instant::now();
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        && started.elapsed() < Duration::from_secs(30) =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error),
            }
        };
        self.connection = Some(Connection::establish(
            stream,
            &token,
            self.output.clone(),
            crate::authentication::Role::Listener,
        )?);
        self.event("initialized", json!({}))?;
        self.respond(request_seq, "launch", true, json!({}), None)
    }

    fn attach(&mut self, request_seq: u64, arguments: &Value) -> io::Result<()> {
        let host = arguments
            .get("host")
            .and_then(Value::as_str)
            .unwrap_or("127.0.0.1");
        let port = arguments
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "attach requires a TCP `port`")
            })?;
        let token = arguments
            .get("token")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "attach requires `token`")
            })?;
        let endpoint: SocketAddr = format!("{host}:{port}")
            .parse()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let stream = TcpStream::connect_timeout(&endpoint, Duration::from_secs(10))?;
        self.connection = Some(Connection::establish(
            stream,
            token,
            self.output.clone(),
            crate::authentication::Role::Connector,
        )?);
        self.event("initialized", json!({}))?;
        self.respond(request_seq, "attach", true, json!({}), None)
    }

    fn remote(&self, command: &str, arguments: Value) -> Result<Value, String> {
        self.connection
            .as_ref()
            .ok_or_else(|| "debuggee is not connected".to_owned())?
            .request(command, arguments)
    }

    fn event(&mut self, event: &str, body: Value) -> io::Result<()> {
        let sequence = self.output.next_sequence();
        self.output
            .write(&json!({ "seq": sequence, "type": "event", "event": event, "body": body }))
    }

    fn respond(
        &mut self,
        request_seq: u64,
        command: &str,
        success: bool,
        body: Value,
        message: Option<String>,
    ) -> io::Result<()> {
        let sequence = self.output.next_sequence();
        self.output.write(&json!({
            "seq": sequence,
            "type": "response",
            "request_seq": request_seq,
            "command": command,
            "success": success,
            "body": body,
            "message": message
        }))
    }
}

struct Connection {
    writer: Arc<Mutex<TcpStream>>,
    pending: PendingResponses,
    next_id: AtomicU64,
}

impl Connection {
    fn establish(
        mut stream: TcpStream,
        token: &str,
        output: DapOutput,
        role: crate::authentication::Role,
    ) -> io::Result<Self> {
        let mut reader = crate::authentication::authenticate(&mut stream, token, role)?;
        let writer = Arc::new(Mutex::new(stream));
        let pending: PendingResponses = Arc::new(Mutex::new(HashMap::new()));
        let reader_pending = Arc::clone(&pending);
        std::thread::Builder::new()
            .name("teal-debug-event-reader".to_owned())
            .spawn(move || {
                let mut terminated = false;
                while let Ok(Some(packet)) = read_packet(&mut reader) {
                    match packet {
                        Packet::Response {
                            id,
                            success,
                            body,
                            message,
                        } => {
                            if let Some(sender) = reader_pending
                                .lock()
                                .expect("pending response lock poisoned")
                                .remove(&id)
                            {
                                let result = if success {
                                    Ok(body)
                                } else {
                                    Err(message.unwrap_or_else(|| {
                                        "debuggee rejected the request".to_owned()
                                    }))
                                };
                                let _ = sender.send(result);
                            }
                        }
                        Packet::Event { event, body } => {
                            if event == "terminated" {
                                if terminated {
                                    continue;
                                }
                                terminated = true;
                            }
                            let _ = output.event(&event, body);
                        }
                        _ => {}
                    }
                }
                let mut pending = reader_pending
                    .lock()
                    .expect("pending response lock poisoned");
                for (_, sender) in pending.drain() {
                    let _ = sender.send(Err("debuggee connection closed".to_owned()));
                }
                if !terminated {
                    let _ = output.event("terminated", json!({}));
                }
            })?;
        Ok(Self {
            writer,
            pending,
            next_id: AtomicU64::new(1),
        })
    }

    fn request(&self, command: &str, arguments: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| "response table lock is poisoned".to_owned())?
            .insert(id, sender);
        let packet = Packet::Command {
            id,
            command: command.to_owned(),
            arguments,
        };
        write_packet(
            &mut *self
                .writer
                .lock()
                .map_err(|_| "debug transport lock is poisoned".to_owned())?,
            &packet,
        )
        .map_err(|error| error.to_string())?;
        receiver
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| format!("timed out waiting for `{command}`"))?
    }
}

#[derive(Clone)]
struct DapOutput {
    writer: Arc<Mutex<BufWriter<Box<dyn Write + Send>>>>,
    event_sequence: Arc<AtomicU64>,
}

impl DapOutput {
    fn new(writer: impl Write + Send + 'static) -> Self {
        Self {
            writer: Arc::new(Mutex::new(BufWriter::new(Box::new(writer)))),
            event_sequence: Arc::new(AtomicU64::new(1)),
        }
    }

    fn write(&self, value: &Value) -> io::Result<()> {
        let body = serde_json::to_vec(value).map_err(io::Error::other)?;
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("DAP output lock is poisoned"))?;
        writer.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())?;
        writer.write_all(&body)?;
        writer.flush()
    }

    fn event(&self, event: &str, body: Value) -> io::Result<()> {
        let sequence = self.next_sequence();
        self.write(&json!({ "seq": sequence, "type": "event", "event": event, "body": body }))
    }

    fn next_sequence(&self) -> u64 {
        self.event_sequence.fetch_add(1, Ordering::Relaxed)
    }
}

fn read_dap(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        let count = reader.read_line(&mut line)?;
        if count == 0 {
            return if length.is_none() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete DAP header",
                ))
            };
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("Content-Length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let length = length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    if length > 16 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "DAP message is too large",
        ));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn capabilities() -> Value {
    json!({
        "supportsConfigurationDoneRequest": true,
        "supportsFunctionBreakpoints": false,
        "supportsConditionalBreakpoints": false,
        "supportsHitConditionalBreakpoints": false,
        "supportsEvaluateForHovers": true,
        "supportsSetVariable": true,
        "supportsSetExpression": false,
        "supportsTerminateRequest": true,
        "supportsExceptionInfoRequest": true,
        "supportsLoadedSourcesRequest": false,
        "exceptionBreakpointFilters": [
            { "filter": "uncaught", "label": "Uncaught exceptions", "default": true },
            { "filter": "caught", "label": "Caught exceptions", "default": false }
        ]
    })
}

fn unpack(result: Result<Value, String>) -> (bool, Value, Option<String>) {
    match result {
        Ok(body) => (true, body, None),
        Err(message) => (false, json!({}), Some(message)),
    }
}

fn random_token() -> io::Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::{
        capabilities, launch_terminal_kind, read_dap, Connection, DapOutput,
        validate_launch_arguments,
    };
    use crate::wire::{Packet, read_packet, write_packet};
    use std::{
        io::{BufReader, Cursor, Write},
        net::{TcpListener, TcpStream},
        sync::{Arc, Mutex},
        thread,
        time::Duration,
    };

    #[derive(Clone)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn capabilities_match_the_implemented_source_debugger() {
        let capabilities = capabilities();
        assert_eq!(capabilities["supportsSetVariable"], true);
        assert_eq!(capabilities["supportsEvaluateForHovers"], true);
        assert_eq!(capabilities["supportsConditionalBreakpoints"], false);
    }

    #[test]
    fn permission_file_is_the_only_launch_permission_input() {
        assert!(
            validate_launch_arguments(&serde_json::json!({
                "permissionFile": "permissions.toml"
            }))
            .is_ok()
        );
        for field in [
            "allowCapabilities",
            "allowReadFiles",
            "allowReadDirectories",
            "allowWriteFiles",
            "allowWriteDirectories",
            "allowExecutables",
            "allowEnvironment",
        ] {
            let mut arguments = serde_json::Map::new();
            arguments.insert(field.to_owned(), serde_json::json!([]));
            let error = validate_launch_arguments(&serde_json::Value::Object(arguments))
                .expect_err("legacy allow field should be rejected");
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains("permissionFile"));
        }
    }

    #[test]
    fn dap_framing_round_trips_utf8_json() {
        let body =
            br#"{"seq":1,"type":"request","command":"initialize","arguments":{"locale":"zh-CN"}}"#;
        let mut framed = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        framed.extend_from_slice(body);
        let message = read_dap(&mut Cursor::new(framed)).unwrap().unwrap();
        assert_eq!(message["command"], "initialize");
    }

    #[test]
    fn removed_launch_fields_are_rejected() {
        assert!(validate_launch_arguments(&serde_json::json!({"features": []})).is_err());
        assert!(validate_launch_arguments(&serde_json::json!({"extensions": []})).is_err());
        let mut removed = serde_json::json!({});
        removed
            .as_object_mut()
            .unwrap()
            .insert(["allow", "Modules"].concat(), serde_json::json!([]));
        assert!(validate_launch_arguments(&removed).is_err());
        assert!(
            validate_launch_arguments(&serde_json::json!({"runtimeProfile": "native"})).is_ok()
        );
    }

    #[test]
    fn launch_console_selects_a_terminal_kind_and_rejects_other_values() {
        assert_eq!(
            launch_terminal_kind(&serde_json::json!({})).unwrap(),
            "external"
        );
        assert_eq!(
            launch_terminal_kind(&serde_json::json!({"console":"externalTerminal"})).unwrap(),
            "external"
        );
        assert_eq!(
            launch_terminal_kind(&serde_json::json!({"console":"integratedTerminal"})).unwrap(),
            "integrated"
        );
        for value in [
            serde_json::json!("internalConsole"),
            serde_json::json!(null),
            serde_json::json!(true),
        ] {
            let arguments = serde_json::json!({"console": value});
            let error = launch_terminal_kind(&arguments).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains("console"));
            assert!(validate_launch_arguments(&arguments).is_err());
        }
    }

    #[test]
    fn accepted_nonblocking_stream_waits_for_a_delayed_hello() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let debuggee = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            thread::sleep(Duration::from_millis(50));
            crate::authentication::authenticate(
                &mut stream,
                "delayed-token",
                crate::authentication::Role::Connector,
            )
            .unwrap();
        });
        let (stream, _) = listener.accept().unwrap();
        stream.set_nonblocking(true).unwrap();
        let connection = Connection::establish(
            stream,
            "delayed-token",
            DapOutput::new(std::io::sink()),
            crate::authentication::Role::Listener,
        );
        assert!(
            connection.is_ok(),
            "a delayed hello on an accepted nonblocking stream must authenticate"
        );
        drop(connection);
        debuggee.join().unwrap();
    }

    #[test]
    fn duplicate_debuggee_terminated_events_are_forwarded_once() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let debuggee = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            crate::authentication::authenticate(
                &mut stream,
                "terminated-token",
                crate::authentication::Role::Connector,
            )
            .unwrap();
            for _ in 0..2 {
                write_packet(
                    &mut stream,
                    &Packet::Event {
                        event: "terminated".into(),
                        body: serde_json::json!({}),
                    },
                )
                .unwrap();
            }
        });
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (stream, _) = listener.accept().unwrap();
        let connection = Connection::establish(
            stream,
            "terminated-token",
            DapOutput::new(SharedWriter(Arc::clone(&bytes))),
            crate::authentication::Role::Listener,
        )
        .unwrap();
        debuggee.join().unwrap();
        for _ in 0..100 {
            let mut reader = BufReader::new(Cursor::new(bytes.lock().unwrap().clone()));
            let mut terminated = 0;
            while let Some(value) = read_dap(&mut reader).unwrap() {
                terminated += usize::from(value["event"] == "terminated");
            }
            if terminated == 1 {
                drop(connection);
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("exactly one terminated event should be forwarded");
    }
}
