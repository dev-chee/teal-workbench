//! 工作台扩展包、实例事务和 Teal SDK 的黑盒回归测试。

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    sync::Arc,
};

use teal::{
    CompileOptions, ExecutionBindings, FsSourceLoader, HostExecutionGrant, HostRegistry,
    NativeHostRuntime, RuntimeProfile, Source, prepare_execution,
};
use teal_agent::{ActionContext, RunState, RuntimeServices, RuntimeStore, UserQuestion};
use teal_model::{AuthProviderRegistry, ModelFactoryRegistry};
use teal_workbench_automation::{
    WorkbenchAutomationRuntime,
    extension::{
        ApprovalReceipt, ExtensionCall, ExtensionEvent, ExtensionInstance, WorkbenchPluginPackage,
    },
};

fn write(root: &Path, name: &str, contents: &str) {
    fs::write(root.join(name), contents).expect("extension fixture should be written");
}

struct TestAuditSink;

impl teal_security::AuditSink for TestAuditSink {
    fn append(
        &self,
        _: &teal_security::Event,
    ) -> Result<teal_security::Receipt, teal_security::AuditError> {
        Ok(teal_security::Receipt {
            sequence: 1,
            hash: "0".repeat(64),
        })
    }
}

fn runtime() -> WorkbenchAutomationRuntime {
    WorkbenchAutomationRuntime::for_extension(HostRegistry::standard())
        .with_audit_context(teal_security::AuditContext::new(Arc::new(TestAuditSink)))
}

fn manifest(operations: &str, entry: &str) -> String {
    format!(
        r#"id = "demo"
version = "1.0.0"
api_version = 1
operations = [{operations}]

[commands]
run = "{entry}"

[tasks]
work = "worker.tl"
"#
    )
}

fn package(root: &Path, operations: &str, entry: &str) -> WorkbenchPluginPackage {
    write(root, "extension.toml", &manifest(operations, entry));
    write(root, "worker.tl", "fn main() {}\n");
    WorkbenchPluginPackage::load(root, &runtime()).expect("plugin package should load")
}

fn persistent_program() -> teal::CompiledProgram {
    teal::compile_source(
        Source::new(
            "extension-task.tl",
            "fn main() { entry_task(); } task entry_task() { print(\"done\"); }",
        ),
        &FsSourceLoader,
        CompileOptions::default().with_runtime_profile(RuntimeProfile::agent()),
    )
    .expect("persistent extension task should compile")
}

fn context_digest(value: char) -> String {
    std::iter::repeat_n(value, 64).collect()
}

#[test]
fn package_loads_declared_entries_and_fingerprint_tracks_entry_source() {
    let temporary = tempfile::tempdir().expect("temporary extension root");
    let root = temporary.path();
    write(root, "entry.tl", "fn main() {}\n");
    let first = package(root, "", "entry.tl");

    assert!(first.entry("command", "run").is_some());
    assert!(first.entry("task", "work").is_some());
    assert!(first.entry("hook", "run").is_none());
    assert!(first.operations.is_empty());

    write(root, "entry.tl", "fn main() { print(1); }\n");
    let second =
        WorkbenchPluginPackage::load(root, &runtime()).expect("changed package should load");
    assert_ne!(first.fingerprint, second.fingerprint);
}

#[test]
fn package_rejects_the_removed_capabilities_manifest_field() {
    let temporary = tempfile::tempdir().expect("temporary plugin root");
    let root = temporary.path();
    write(root, "entry.tl", "fn main() {}\n");
    write(root, "worker.tl", "fn main() {}\n");
    write(
        root,
        "extension.toml",
        r#"id = "demo"
version = "1.0.0"
api_version = 1
capabilities = ["editor.read"]

[commands]
run = "entry.tl"
"#,
    );

    let error = match WorkbenchPluginPackage::load(root, &runtime()) {
        Ok(_) => panic!("removed capabilities field must fail closed"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("capabilities"), "{error}");
}

#[test]
fn package_rejects_undeclared_operations_and_entries_outside_its_root() {
    let temporary = tempfile::tempdir().expect("temporary extension parent");
    let root = temporary.path().join("extension");
    fs::create_dir(&root).expect("extension root");
    write(
        &root,
        "entry.tl",
        "import \"bench/extension.tl\" as extension; fn main() { extension::set(\"phase\", \"ready\"); }\n",
    );
    write(&root, "worker.tl", "fn main() {}\n");
    write(&root, "extension.toml", &manifest("", "entry.tl"));
    let error = match WorkbenchPluginPackage::load(&root, &runtime()) {
        Ok(_) => panic!("missing operation must fail closed"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("undeclared operations"),
        "{error}"
    );

    fs::write(temporary.path().join("outside.tl"), "fn main() {}\n").expect("outside fixture");
    write(&root, "extension.toml", &manifest("", "../outside.tl"));
    let error = match WorkbenchPluginPackage::load(&root, &runtime()) {
        Ok(_) => panic!("outside entry must fail closed"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("entry escapes"), "{error}");
}

#[test]
fn declared_model_tool_operation_is_preserved_while_undeclared_static_requirement_is_rejected() {
    let temporary = tempfile::tempdir().expect("temporary extension root");
    let root = temporary.path();
    write(root, "entry.tl", "fn main() {}\n");
    write(root, "worker.tl", "fn main() {}\n");
    write(
        root,
        "extension.toml",
        r#"id = "demo"
version = "1.0.0"
api_version = 1
operations = ["editor.read"]

[commands]
run = "entry.tl"

[tasks]
work = "worker.tl"

[task_tools]
work = ["workspace.read_file"]
"#,
    );

    let package = WorkbenchPluginPackage::load(root, &runtime())
        .expect("a declared model-tool operation must survive static compilation");
    assert_eq!(package.operations, BTreeSet::from(["editor.read".into()]));
    assert_eq!(
        package.manifest.task_tools["work"],
        BTreeSet::from(["workspace.read_file".into()])
    );

    write(
        root,
        "entry.tl",
        "import \"bench/extension.tl\" as extension; fn main() { extension::set(\"phase\", \"ready\"); }\n",
    );
    let error = match WorkbenchPluginPackage::load(root, &runtime()) {
        Ok(_) => panic!("a source-required operation missing from the manifest must fail closed"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("undeclared operations"),
        "{error}"
    );
    assert!(error.to_string().contains("extension.write"), "{error}");
}

#[test]
fn instance_event_identity_is_idempotent_conflict_detected_and_stale_commit_preserves_saved_state()
{
    let temporary = tempfile::tempdir().expect("temporary state root");
    let state = temporary.path().join("instance.json");
    let event = ExtensionEvent {
        id: "event-1".into(),
        kind: "command".into(),
        name: "run".into(),
        fields: BTreeMap::new(),
    };
    let mut first = ExtensionInstance::new("demo", "instance-1", "fingerprint");
    let mut values = BTreeMap::new();
    values.insert("phase".into(), "planned".into());
    first
        .commit(&state, &event, values, Vec::new(), Vec::new())
        .expect("first event should commit");
    assert!(
        first
            .contains_event(&event)
            .expect("same event should be recognized")
    );

    let mut changed_event = event.clone();
    changed_event
        .fields
        .insert("source".into(), "different".into());
    let error = first
        .contains_event(&changed_event)
        .expect_err("same event ID with changed content must fail");
    assert!(error.to_string().contains("different content"), "{error}");

    let mut stale = ExtensionInstance::new("demo", "instance-1", "fingerprint");
    let next_event = ExtensionEvent {
        id: "event-2".into(),
        kind: "command".into(),
        name: "run".into(),
        fields: BTreeMap::new(),
    };
    let mut stale_values = BTreeMap::new();
    stale_values.insert("phase".into(), "published".into());
    let error = stale
        .commit(&state, &next_event, stale_values, Vec::new(), Vec::new())
        .expect_err("stale transaction must not overwrite current state");
    assert!(
        error.to_string().contains("transaction is stale"),
        "{error}"
    );
    let saved = ExtensionInstance::load(&state).expect("saved instance");
    assert_eq!(saved.values["phase"], "planned");
    assert!(saved.events.contains_key("event-1"));
    assert!(!saved.events.contains_key("event-2"));
}

#[test]
fn extension_sdk_executes_in_teal_vm_and_returns_only_pending_host_requests() {
    let temporary = tempfile::tempdir().expect("temporary extension root");
    let root = temporary.path();
    write(
        root,
        "entry.tl",
        r#"
import "bench/extension.tl" as extension;
fn main() {
    extension::set("phase", "planning");
    extension::start_bound("work", "draft plan", "[" + extension::quote("approved-plan") + "]");
    extension::complete("plan ready");
}
"#,
    );
    let package = package(
        root,
        "\"extension.approve\", \"extension.read\", \"extension.task\", \"extension.write\"",
        "entry.tl",
    );
    let mut instance = ExtensionInstance::new("demo", "instance-1", &package.fingerprint);
    instance.approvals.insert(
        "approved-plan".into(),
        ApprovalReceipt {
            callback: None,
            id: "approved-plan".into(),
            prompt: "Review plan".into(),
            artifacts: Vec::new(),
            operation: "approve-plan".into(),
            environment: String::new(),
            approved: Some(true),
        },
    );
    let event = ExtensionEvent {
        id: "event-1".into(),
        kind: "command".into(),
        name: "run".into(),
        fields: BTreeMap::new(),
    };
    let tasks = package
        .manifest
        .tasks
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let call = ExtensionCall::new(instance, event, package.manifest.config.clone(), tasks);
    let mut registry = HostRegistry::standard();
    call.register(&mut registry).expect("SDK host registration");
    let executing = WorkbenchAutomationRuntime::for_extension(registry)
        .with_audit_context(teal_security::AuditContext::new(Arc::new(TestAuditSink)));
    let grant = Arc::new(
        HostExecutionGrant::core_only("test/extension-sdk/v1")
            .allow_capabilities(package.operations.iter().cloned()),
    );

    let exit = executing
        .execute_event(
            package.entry("command", "run").expect("command entry"),
            grant,
        )
        .expect("Teal extension event should execute");
    assert_eq!(exit, 0);
    let output = call.output().expect("extension output");
    assert_eq!(output.values["phase"], "planning");
    assert_eq!(output.result.as_deref(), Some("plan ready"));
    assert_eq!(output.tasks.len(), 1);
    let key = &output.tasks[0].key;
    assert!(key.starts_with("task-"));
    assert_eq!(key.len(), "task-".len() + 64);
    assert_eq!(output.tasks[0].task, "work");
    assert_eq!(output.tasks[0].input, "draft plan");
    assert_eq!(output.tasks[0].approval_ids, ["approved-plan"]);
    assert!(output.tasks[0].run_id.is_none());
    assert!(!output.tasks[0].delivered);
    assert!(output.approvals.is_empty());
}

#[test]
fn prepared_execution_reuses_the_stable_key_and_rejects_changed_context() {
    let temporary = tempfile::tempdir().expect("temporary runtime root");
    let store = Arc::new(RuntimeStore::open(temporary.path()).expect("runtime store"));
    let program = persistent_program();
    let bindings = ExecutionBindings::default();

    let first = prepare_execution(
        &program,
        &store,
        "extension-1:event-1:0",
        &context_digest('a'),
        bindings.clone(),
    )
    .expect("first execution preparation");
    let second = prepare_execution(
        &program,
        &store,
        "extension-1:event-1:0",
        &context_digest('a'),
        bindings,
    )
    .expect("identical preparation should reuse the run");
    assert_eq!(first.run_id, second.run_id);
    let prepared = store
        .prepared_run(&first.run_id)
        .expect("prepared run lookup")
        .expect("prepared run exists");
    assert!(!prepared.started);
    assert!(prepared.result.is_none());

    let error = match prepare_execution(
        &program,
        &store,
        "extension-1:event-1:0",
        &context_digest('b'),
        ExecutionBindings::default(),
    ) {
        Ok(_) => panic!("the stable key must not accept changed inputs"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("prepared run key is bound to different inputs"),
        "{error}"
    );
}

#[test]
fn terminal_prepared_execution_cannot_be_started_again() {
    let temporary = tempfile::tempdir().expect("temporary runtime root");
    let store = Arc::new(RuntimeStore::open(temporary.path()).expect("runtime store"));
    let program = persistent_program();
    let prepared = prepare_execution(
        &program,
        store.as_ref(),
        "extension-1:event-2:0",
        &context_digest('c'),
        ExecutionBindings::default(),
    )
    .expect("execution preparation");
    store
        .set_run_state(&prepared.run_id, RunState::Completed)
        .expect("test run should become terminal");
    let run_id = prepared.run_id.clone();

    let grant = Arc::new(HostExecutionGrant::core_only(
        "test/persistent-extension/v1",
    ));
    let mut host = NativeHostRuntime::builder()
        .trusted()
        .trusted()
        .authorization(grant.clone())
        .build()
        .expect("test Host runtime should build");
    let error = match prepared.bind(
        &mut host,
        teal_model::TomlAuthManager::open(temporary.path(), AuthProviderRegistry::new())
            .expect("auth store"),
        ModelFactoryRegistry::new(),
        Arc::clone(&store),
        grant,
        RuntimeServices::new(),
    ) {
        Ok(_) => panic!("terminal prepared run must not restart"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("terminal run cannot execute again"),
        "{error}"
    );
    assert!(
        !store
            .prepared_run(&run_id)
            .expect("prepared run lookup")
            .expect("prepared run exists")
            .started
    );
}

#[test]
fn failed_terminal_run_keeps_a_candidate_result_without_becoming_successful() {
    let temporary = tempfile::tempdir().expect("temporary runtime root");
    let store = Arc::new(RuntimeStore::open(temporary.path()).expect("runtime store"));
    let prepared = prepare_execution(
        &persistent_program(),
        &store,
        "extension-1:event-4:0",
        &context_digest('e'),
        ExecutionBindings::default(),
    )
    .expect("execution preparation");
    let claim = store.claim_run(&prepared.run_id).expect("run claim");
    claim.start_prepared().expect("prepared run starts once");
    store
        .submit_prepared_result(&prepared.run_id, "candidate result")
        .expect("candidate result persists before terminal state");
    claim
        .set_run_state(&prepared.run_id, RunState::CompletedWithErrors)
        .expect("failed terminal state");

    let saved = store
        .prepared_run(&prepared.run_id)
        .expect("prepared result lookup")
        .expect("prepared run exists");
    assert_eq!(saved.result.as_deref(), Some("candidate result"));
    assert_eq!(
        store
            .run(&prepared.run_id)
            .expect("run lookup")
            .expect("run exists")
            .state,
        RunState::CompletedWithErrors
    );
}
