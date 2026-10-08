//! 仓库随附工作台 Teal 自动化脚本通过真实模块加载器验证。

use super::*;

#[derive(Default)]
struct TestAuditSink(std::sync::Mutex<Vec<teal_security::Event>>);

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

#[test]
fn shipped_bench_automation_examples_prepare_and_execute() {
    let directory = tempfile::tempdir().unwrap();
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/showcases/bench");
    let mut runtime = WorkbenchAutomationRuntime::new(directory.path().join("state.json"))
        .unwrap()
        .with_audit_context(teal_security::AuditContext::new(std::sync::Arc::new(
            TestAuditSink::default(),
        )));
    let messages = Arc::new(Mutex::new(Vec::new()));
    let received = messages.clone();
    runtime
        .registry_mut()
        .register(HostFunctionDefinition::new(
            "std.bench.editor",
            "message",
            signature([RuntimeType::String], RuntimeType::None),
            "ui.panel",
            ExternalCallRecovery::Unsupported,
            move |args| {
                received.lock().unwrap().push(string_arg(args, 0)?);
                Ok(HostValue::None)
            },
        ))
        .unwrap();
    let rendered = Arc::new(Mutex::new(Vec::new()));
    let received = rendered.clone();
    runtime
        .registry_mut()
        .register(HostFunctionDefinition::new(
            "std.bench.panel",
            "render",
            signature([RuntimeType::String], RuntimeType::None),
            "ui.panel",
            ExternalCallRecovery::Unsupported,
            move |args| {
                received.lock().unwrap().push(string_arg(args, 0)?);
                Ok(HostValue::None)
            },
        ))
        .unwrap();
    let event = Arc::new(Mutex::new("mount".to_owned()));
    let sent = event.clone();
    runtime
        .registry_mut()
        .register(HostFunctionDefinition::new(
            "std.bench.panel",
            "event",
            signature([], RuntimeType::Map),
            "ui.panel",
            ExternalCallRecovery::Unsupported,
            move |_| {
                Ok(HostValue::Map(vec![
                    (
                        HostValue::String("kind".into()),
                        HostValue::String(sent.lock().unwrap().clone()),
                    ),
                    (
                        HostValue::String("id".into()),
                        HostValue::String("go".into()),
                    ),
                ]))
            },
        ))
        .unwrap();

    let execute = |name: &str| {
        let prepared = runtime
            .prepare(examples.join(name), ScriptKind::UserConfig)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        runtime.execute(prepared, "{}").unwrap()
    };
    execute("hello.tl");
    assert_eq!(*messages.lock().unwrap(), ["Hello from Teal Bench"]);
    let panel = execute("ui-demo.tl");
    assert_eq!(panel.panels[0].name, "demo");
    assert!(panel.panels[0].script_path.ends_with("ui-demo-panel.tl"));
    execute("ui-demo-panel.tl");
    *event.lock().unwrap() = "activate".into();
    execute("ui-demo-panel.tl");
    let rendered = rendered.lock().unwrap();
    assert_eq!(rendered.len(), 2);
    assert!(rendered[0].contains("Teal Bench UI demo"));
    assert!(rendered[1].contains("Activated: go"));
}
