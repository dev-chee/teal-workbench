//! 通用扩展在工作台应用线程中的装配、审批和终态投递回归。

use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Notify;

use teal::{
    CompileOptions, ExecutionBindings, FsSourceLoader, RuntimeProfile, Source, prepare_execution,
};
use teal_agent::{AgentError, ExternalOperationKind, ExternalOperationState, RunState, RuntimeStore};
use teal_model::{
    AuthAccount, AuthContext, AuthError, AuthFuture, AuthMethodDescriptor, AuthMethodId,
    AuthMethodKind, AuthProfileName, AuthProvider, AuthProviderDescriptor, AuthProviderId,
    AuthProviderRegistry, AuthStatus, Cancellation, LoginEvent, LoginInput,
    AuthenticatedModelFactory, ModelEvent, ModelProvider, ModelRequest, ProviderFuture, StopReason,
};
use teal_workbench_automation::WorkbenchAutomationRuntime;
use teal_workbench_automation::extension::{ExtensionTaskControl, ExtensionTaskRef};
use teal_workbench_core::{InputEvent, Key, KeyModifiers};

use super::*;

struct ModelConfigAuditSink;

impl teal_security::AuditSink for ModelConfigAuditSink {
    fn append(
        &self,
        _: &teal_security::Event,
    ) -> Result<teal_security::Receipt, teal_security::AuditError> {
        Ok(teal_security::Receipt {
            sequence: 1,
            hash: format!("{:064x}", 1),
        })
    }
}

fn ensure_model_config_audit() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = teal_security::install_process_sink(Arc::new(ModelConfigAuditSink));
    });
}

fn input(key: Key) -> InputEvent {
    InputEvent {
        key,
        modifiers: KeyModifiers::default(),
    }
}

async fn app_for(workspace: &Path, state: &Path) -> App {
    ensure_model_config_audit();
    let paths = SessionPaths {
        config: state.join("config"),
        state: state.join("state"),
        cache: state.join("cache"),
    };
    let mut app = App::new_with_session_paths(
        AppOptions {
            start_file: None,
            path: Some(workspace.to_owned()),
            safe_mode: true,
            no_config: true,
            session: None,
            model_protocol_capture: None,
        },
        None,
        false,
        teal_workbench_services::agent_algorithms::ConnectivityMode::Online,
        Some(paths.clone()),
    )
    .await
    .expect("Bench app");
    app.paths = paths;
    app.session_path = state.join("session.json");
    app.mutations = WorkspaceMutationCoordinator::new(
        app.workspace.clone(),
        app.paths.state.join("checkpoints"),
    )
    .expect("workspace coordinator");
    app.teal = WorkbenchAutomationRuntime::new(app.paths.state.join("automation.json"))
        .expect("automation runtime");
    register_automation_bridge(app.teal.registry_mut(), &app.bridge).expect("automation bridge");
    app.trust = TrustStore::load(app.paths.state.join("trust.json")).expect("trust store");
    app
}

async fn next_event(app: &mut App) {
    let event = tokio::time::timeout(Duration::from_secs(10), app.runtime_rx.recv())
        .await
        .unwrap_or_else(|_| {
            let loaded = app
                .extensions
                .loaded
                .iter()
                .map(|(id, extension)| {
                    (
                        id.as_str(),
                        &extension.instance.values,
                        extension.instance.tasks.keys().collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();
            let pending = match &app.pending_overlay {
                Some(PendingOverlay::Confirmation(PendingConfirmation::ExtensionApproval {
                    ..
                })) => "extension-approval",
                Some(PendingOverlay::Confirmation(PendingConfirmation::AgentMutation {
                    ..
                })) => "agent-mutation",
                Some(PendingOverlay::Confirmation(PendingConfirmation::Agent(_))) => "agent",
                Some(PendingOverlay::Confirmation(_)) => "other-confirmation",
                Some(_) => "other-overlay",
                None => "none",
            };
            panic!(
                "extension event should arrive; messages={:?}; loaded={loaded:?}; authenticating={:?}; running={:?}; pending_overlay={pending}",
                app.messages
                    .iter()
                    .map(|message| message.text.as_str())
                    .collect::<Vec<_>>(),
                app.extensions.authenticating.keys().collect::<Vec<_>>(),
                app.extensions.running.keys().collect::<Vec<_>>(),
            )
        })
        .expect("runtime sender should stay open");
    app.handle_runtime_event(event).await;
}

async fn load_approved(app: &mut App, root: &Path, id: &str) {
    app.load_extension(root.to_owned(), BTreeMap::new());
    next_event(app).await;
    assert!(
        matches!(
            app.pending_overlay,
            Some(PendingOverlay::Confirmation(
                PendingConfirmation::ExtensionLoad(_)
            ))
        ),
        "extension load messages: {:?}",
        app.messages
    );
    app.handle_input(input(Key::Enter));
    assert!(app.extensions.loaded.contains_key(id));
}

fn write(root: &Path, name: &str, contents: &str) {
    std::fs::write(root.join(name), contents).expect("extension fixture");
}

fn seed_sdlc_auth(root: &Path) -> teal_model::SharedModelConfiguration {
    ensure_model_config_audit();
    let configuration = Arc::new(teal_model::ModelConfiguration::toml(
        teal_model::TomlModelConfiguration::from_root(root).expect("test model configuration"),
    ));
    let snapshot = configuration.load().expect("empty model configuration");
    let revision = snapshot.layer_revision(teal_model::ModelConfigScope::User);
    let provider = teal_model::ProviderId::new("sdlc-test").expect("provider ID");
    let model = teal_model::ModelId::new("sdlc-test-model").expect("model ID");
    let mut document = snapshot.user;
    document.providers.insert(
        provider,
        teal_model::ProviderConfig {
            auth_profiles: ["default", "work"]
                .into_iter()
                .map(|profile| {
                    (
                        AuthProfileName::new(profile).expect("auth profile"),
                        teal_model::AuthProfileConfig {
                            method: Some(teal_model::AuthMethodId::new("test").unwrap()),
                            account_id: Some("sdlc-test-account".into()),
                            available_models: Some([model.clone()].into_iter().collect()),
                            ..teal_model::AuthProfileConfig::default()
                        },
                    )
                })
                .collect(),
            models: [(model, teal_model::ModelConfig::default())]
                .into_iter()
                .collect(),
            ..teal_model::ProviderConfig::default()
        },
    );
    configuration
        .replace(teal_model::ModelConfigScope::User, &revision, &document)
        .expect("seed test model configuration");
    configuration
}

async fn model_extension_waiting_for_auth() -> (tempfile::TempDir, App, PathBuf, String, Arc<Notify>)
{
    let directory = tempfile::tempdir().expect("workspace");
    let state = directory.path().join("state");
    let extension = directory.path().join("model-extension");
    std::fs::create_dir_all(&extension).expect("extension root");
    write(
        &extension,
        "extension.toml",
        r#"id = "model-auth"
version = "1.0.0"
api_version = 1
operations = ["extension.read", "extension.task"]

[commands]
advance = "command.tl"

[tasks]
work = "work.tl"
"#,
    );
    write(
        &extension,
        "command.tl",
        r#"import "bench/extension.tl" as extension;
fn main() { extension::start("work", "review"); }
"#,
    );
    write(
        &extension,
        "work.tl",
        r#"@model("sdlc-test", "sdlc-test-model", "work") agent Writer {}
impl Writer { pub task write(prompt: string) -> string { return self.think(prompt).await; } }
fn main() { entry_task(); }
task entry_task() { let writer = Writer {}; print(writer.write("review").await); }
"#,
    );

    let mut app = app_for(directory.path(), &state).await;
    install_sdlc_model(&mut app, &state).await;
    let release = Arc::new(Notify::new());
    let providers = AuthProviderRegistry::new();
    providers
        .register(Arc::new(SdlcDelayedSignedInAuth {
            release: Arc::clone(&release),
        }))
        .expect("register delayed authentication");
    let auth_root = state.join("delayed-auth");
    let configuration = seed_sdlc_auth(&auth_root);
    let auth = teal_model::AuthManager::open(Arc::clone(&configuration), providers)
        .expect("delayed authentication manager");
    app.set_configured_model_access_for_test(
        configuration,
        auth,
        teal_model::ModelFactoryRegistry::new(),
    );
    let provider = AuthProviderId::new("sdlc-test").expect("provider ID");
    let profile = AuthProfileName::new("default").expect("profile");
    app.auth_for_test()
        .logout(&provider, &profile, Cancellation::new())
        .await
        .expect("test profile should persist before refresh");
    app.auth_for_test()
        .set_active_in_scope(&provider, &profile, teal_model::ModelConfigScope::User)
        .expect("test profile should be active");
    load_approved(&mut app, &extension, "model-auth").await;
    assert!(app.route_extension_command("advance", &[]));
    next_event(&mut app).await;
    let key = app.extensions.loaded["model-auth"]
        .instance
        .tasks
        .keys()
        .next()
        .expect("extension command should start its task")
        .clone();
    assert!(
        app.extensions.authenticating.contains_key(&key),
        "a model task must wait for authentication before allocating its run"
    );
    (directory, app, extension, key, release)
}

fn write_extension(root: &Path, command: &str) {
    std::fs::create_dir_all(root).expect("extension root");
    write(
        root,
        "extension.toml",
        r#"id = "demo"
version = "1.0.0"
api_version = 1
operations = ["extension.approve", "extension.read", "extension.task", "extension.write"]

[commands]
advance = "command.tl"

[modes]
workflow = "mode.tl"

[tasks]
work = "work.tl"
"#,
    );
    write(root, "command.tl", command);
    write(
        root,
        "mode.tl",
        r#"
import "bench/extension.tl" as extension;
fn main() {
    let action = extension::event("action");
    if action == "task_completed" {
        extension::set("phase", "published");
    } else {
        extension::set("outcome", action);
    }
}

"#,
    );
    write(root, "work.tl", "fn main() {}\n");
}

#[cfg(unix)]
fn link_directory(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn link_directory(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[test]
fn workspace_path_guard_allows_new_nested_paths_and_rejects_escape_or_link_parents() {
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside directory");
    let root = workspace.path();

    assert!(ensure_workspace_path(root, &root.join("intent/flow-1/plan.md")).is_ok());
    assert!(ensure_workspace_path(root, &outside.path().join("escape.md")).is_err());

    let link = root.join("linked-outside");
    match link_directory(outside.path(), &link) {
        Ok(()) => {}
        Err(error) if error.raw_os_error() == Some(1314) => return,
        Err(error) => panic!("create test directory link: {error}"),
    }
    assert!(
        ensure_workspace_path(root, &link.join("nested/artifact.md")).is_err(),
        "a missing nested path below a directory link must not escape the workspace"
    );
}

fn persistent_program() -> teal::CompiledProgram {
    teal::compile_source(
        Source::new(
            "bench-extension-result.tl",
            "fn main() { entry_task(); } task entry_task() { print(\"done\"); }",
        ),
        &FsSourceLoader,
        CompileOptions::default().with_runtime_profile(RuntimeProfile::agent()),
    )
    .expect("persistent task program")
}

#[derive(Clone)]
struct SdlcFakeAuth;

impl AuthProvider for SdlcFakeAuth {
    fn descriptor(&self) -> AuthProviderDescriptor {
        AuthProviderDescriptor {
            id: AuthProviderId::new("sdlc-test").expect("provider id"),
            display_name: "SDLC test authentication".into(),
            methods: vec![AuthMethodDescriptor {
                id: AuthMethodId::new("test").expect("method id"),
                kind: AuthMethodKind::ExternalIdentity,
                label: "Test identity".into(),
            }],
            supports_remote_verification: false,
        }
    }

    fn status(&self, _: AuthContext, _: bool, _: Cancellation) -> AuthFuture<AuthStatus> {
        Box::pin(async {
            Ok(AuthStatus::SignedIn(AuthAccount {
                account_id: Some("sdlc-test-account".into()),
                email: None,
                plan_type: Some("test".into()),
            }))
        })
    }

    fn login(
        &self,
        _: AuthContext,
        _: AuthMethodId,
        _: tokio::sync::mpsc::Sender<LoginEvent>,
        _: tokio::sync::mpsc::Receiver<LoginInput>,
        _: Cancellation,
    ) -> AuthFuture<AuthAccount> {
        Box::pin(async { Err(AuthError::Denied) })
    }

    fn logout(&self, _: AuthContext, _: Cancellation) -> AuthFuture<()> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Clone)]
struct SdlcSignedOutAuth;

impl AuthProvider for SdlcSignedOutAuth {
    fn descriptor(&self) -> AuthProviderDescriptor {
        AuthProviderDescriptor {
            id: AuthProviderId::new("sdlc-test").expect("provider id"),
            display_name: "SDLC test authentication".into(),
            methods: vec![AuthMethodDescriptor {
                id: AuthMethodId::new("test").expect("method id"),
                kind: AuthMethodKind::ExternalIdentity,
                label: "Test identity".into(),
            }],
            supports_remote_verification: false,
        }
    }

    fn status(&self, _: AuthContext, _: bool, _: Cancellation) -> AuthFuture<AuthStatus> {
        Box::pin(async { Ok(AuthStatus::SignedOut) })
    }

    fn login(
        &self,
        _: AuthContext,
        _: AuthMethodId,
        _: tokio::sync::mpsc::Sender<LoginEvent>,
        _: tokio::sync::mpsc::Receiver<LoginInput>,
        _: Cancellation,
    ) -> AuthFuture<AuthAccount> {
        Box::pin(async { Err(AuthError::Denied) })
    }

    fn logout(&self, _: AuthContext, _: Cancellation) -> AuthFuture<()> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Clone)]
struct SdlcDelayedSignedInAuth {
    release: Arc<Notify>,
}

impl AuthProvider for SdlcDelayedSignedInAuth {
    fn descriptor(&self) -> AuthProviderDescriptor {
        AuthProviderDescriptor {
            id: AuthProviderId::new("sdlc-test").expect("provider id"),
            display_name: "Delayed SDLC test authentication".into(),
            methods: vec![AuthMethodDescriptor {
                id: AuthMethodId::new("test").expect("method id"),
                kind: AuthMethodKind::ExternalIdentity,
                label: "Test identity".into(),
            }],
            supports_remote_verification: false,
        }
    }

    fn status(&self, _: AuthContext, _: bool, _: Cancellation) -> AuthFuture<AuthStatus> {
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            release.notified().await;
            Ok(AuthStatus::SignedIn(AuthAccount {
                account_id: Some("sdlc-test-account".into()),
                email: None,
                plan_type: None,
            }))
        })
    }

    fn login(
        &self,
        _: AuthContext,
        _: AuthMethodId,
        _: tokio::sync::mpsc::Sender<LoginEvent>,
        _: tokio::sync::mpsc::Receiver<LoginInput>,
        _: Cancellation,
    ) -> AuthFuture<AuthAccount> {
        Box::pin(async { Err(AuthError::Denied) })
    }

    fn logout(&self, _: AuthContext, _: Cancellation) -> AuthFuture<()> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Clone)]
struct SdlcModelFactory {
    responses: Arc<Mutex<VecDeque<Vec<ModelEvent>>>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl AuthenticatedModelFactory for SdlcModelFactory {
    fn provider(&self) -> AuthProviderId {
        AuthProviderId::new("sdlc-test").expect("provider id")
    }

    fn describe(
        &self,
        _: &AuthContext,
        _: Option<&teal_model::ModelConfig>,
    ) -> Result<(teal_model::ModelServiceContract, String), AgentError> {
        Ok((
            teal_model::ModelServiceContract {
                endpoint: "https://models.example.test/".into(),
                protocol: teal_model::ProtocolId::new("responses").unwrap(),
                transport: "http".into(),
            },
            "sdlc-test-account".into(),
        ))
    }

    fn create(
        &self,
        _: AuthContext,
        _model_config: Option<&teal_model::ModelConfig>,
    ) -> ProviderFuture<Result<teal_model::PreparedModel, AgentError>> {
        let model = SdlcModel {
            responses: Arc::clone(&self.responses),
            requests: Arc::clone(&self.requests),
        };
        Box::pin(async move {
            Ok(teal_model::PreparedModel {
                provider: Arc::new(model),
                service: teal_model::ModelServiceContract {
                    endpoint: "https://models.example.test/".into(),
                    protocol: teal_model::ProtocolId::new("responses").unwrap(),
                    transport: "http".into(),
                },
                account_identity: "sdlc-test-account".into(),
            })
        })
    }
}

struct SdlcModel {
    responses: Arc<Mutex<VecDeque<Vec<ModelEvent>>>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl ModelProvider for SdlcModel {
    fn name(&self) -> &str {
        "sdlc-test-model"
    }

    fn stream(
        &self,
        request: ModelRequest,
        events: tokio::sync::mpsc::Sender<ModelEvent>,
        _: Cancellation,
    ) -> ProviderFuture<Result<(), AgentError>> {
        self.requests.lock().expect("request log").push(request);
        let response = self.responses.lock().expect("response script").pop_front();
        Box::pin(async move {
            for event in response
                .ok_or_else(|| AgentError::Provider("SDLC model script exhausted".into()))?
            {
                events
                    .send(event)
                    .await
                    .map_err(|_| AgentError::Cancelled)?;
            }
            Ok(())
        })
    }
}

fn completed(value: serde_json::Value) -> Vec<ModelEvent> {
    vec![
        ModelEvent::ToolStart {
            id: "complete".into(),
            name: "__teal_complete".into(),
        },
        ModelEvent::ToolArguments {
            id: "complete".into(),
            delta: serde_json::json!({"value": value}).to_string(),
        },
        ModelEvent::ToolEnd {
            id: "complete".into(),
        },
        ModelEvent::End {
            reason: StopReason::ToolUse,
        },
    ]
}

fn write_then_continue(path: &str, content: &str) -> Vec<ModelEvent> {
    vec![
        ModelEvent::ToolStart {
            id: "write".into(),
            name: "workspace.write_file".into(),
        },
        ModelEvent::ToolArguments {
            id: "write".into(),
            delta: serde_json::json!({"path": path, "content": content}).to_string(),
        },
        ModelEvent::ToolEnd { id: "write".into() },
        ModelEvent::End {
            reason: StopReason::ToolUse,
        },
    ]
}

async fn install_sdlc_model(app: &mut App, state: &Path) -> Arc<Mutex<Vec<ModelRequest>>> {
    let providers = AuthProviderRegistry::new();
    providers
        .register(Arc::new(SdlcFakeAuth))
        .expect("register fake authentication");
    let auth_root = state.join("auth");
    let configuration = seed_sdlc_auth(&auth_root);
    let auth = teal_model::AuthManager::open(Arc::clone(&configuration), providers)
        .expect("test auth manager");
    app.set_configured_model_access_for_test(
        configuration,
        auth,
        teal_model::ModelFactoryRegistry::new(),
    );
    let provider = AuthProviderId::new("sdlc-test").expect("provider id");
    let profile = AuthProfileName::new("default").expect("profile");
    app.auth_for_test()
        .status(&provider, &profile, false, Cancellation::new())
        .await
        .expect("fake profile status");

    let requests = Arc::new(Mutex::new(Vec::new()));
    let source = "fn feature() { print(\"planned implementation\"); }\n";
    let responses = VecDeque::from([
        completed(
            serde_json::json!({"markdown":"# Intent\n\nImplement the planned feature.","files":[]}),
        ),
        completed(
            serde_json::json!({"markdown":"# Spec\n\nThe feature prints its contract.","files":[]}),
        ),
        completed(
            serde_json::json!({"markdown":"# Plan\n\nWrite the approved feature.","files":["src/feature.tl"]}),
        ),
        write_then_continue("src/feature.tl", source),
        completed(
            serde_json::json!({"summary":"Wrote src/feature.tl through the approved workspace tool.","plan_changed":false}),
        ),
        completed(serde_json::json!({"important":false,"summary":"No boundary defect found."})),
        completed(
            serde_json::json!({"important":false,"summary":"No authorization defect found."}),
        ),
        completed(
            serde_json::json!({"important":false,"summary":"Conforms to the approved plan."}),
        ),
        completed(serde_json::json!(
            "# Incident intent\n\nFacts: imported alert.\n\nRegression: add a durable evaluation."
        )),
        completed(
            serde_json::json!({"markdown":"# Incident follow-up intent\n\nAdd the approved durable regression.","files":[]}),
        ),
    ]);
    app.models_for_test()
        .register(Arc::new(SdlcModelFactory {
            responses: Arc::new(Mutex::new(responses)),
            requests: Arc::clone(&requests),
        }))
        .expect("register scripted model");
    app.agent_settings.insert(
        "sdlc-test".into(),
        (
            state.join("sdlc-agent.toml"),
            teal_workbench_automation::AgentSettings {
                name: "sdlc-test".into(),
                provider: "sdlc-test".into(),
                profile: Some("default".into()),
                model: "sdlc-test-model".into(),
                compaction_profile: None,
                instructions: "Return the requested structured completion.".into(),
            },
        ),
    );
    app.active_agent = Some("sdlc-test".into());
    requests
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("start git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git output")
}

fn test_command(script: &str) -> String {
    serde_json::json!({
        "program":"powershell",
        "args":["-NoProfile","-NonInteractive","-File",script],
        "cwd":".",
        "timeout_ms":30_000,
        "inputs":["src/feature.tl"]
    })
    .to_string()
}

fn has_changeset_review(app: &App) -> bool {
    app.panels.iter().any(|(_, panel)| {
        panel.kind() == PanelKind::Agent
            && panel.as_text().is_some_and(|panel| {
                panel.agent_blocks().iter().any(|block| {
                    matches!(block, crate::presentation::AgentBlock::ChangeSetReview(_))
                })
            })
    })
}

async fn drive_until(
    app: &mut App,
    description: &str,
    accept_agent_mutations: bool,
    predicate: impl Fn(&App) -> bool,
) {
    for _ in 0..128 {
        if predicate(app) {
            return;
        }
        next_event(app).await;
        app.settle_mutations().await;
        match &app.pending_overlay {
            Some(PendingOverlay::Confirmation(PendingConfirmation::Agent(_))) => {
                // Git 和命名检查经 ChannelApproval 到达真实确认浮层。
                app.handle_input(input(Key::Enter));
            }
            Some(PendingOverlay::Confirmation(PendingConfirmation::AgentMutation { .. }))
                if accept_agent_mutations =>
            {
                assert!(
                    has_changeset_review(app),
                    "an Agent mutation approval must expose its ChangeSet review"
                );
                app.handle_input(input(Key::Enter));
                app.settle_mutations().await;
            }
            _ => {}
        }
    }
    panic!(
        "timed out while waiting for {description}: {:?}",
        app.messages
    );
}

async fn accept_sdlc_approval(app: &mut App, operation: &str) {
    drive_until(app, operation, true, |app| {
        matches!(
            app.pending_overlay,
            Some(PendingOverlay::Confirmation(
                PendingConfirmation::ExtensionApproval { .. }
            ))
        )
    })
    .await;
    let Some(PendingOverlay::Confirmation(PendingConfirmation::ExtensionApproval {
        extension,
        receipt,
    })) = &app.pending_overlay
    else {
        unreachable!("the wait predicate checked the concrete overlay type");
    };
    assert_eq!(extension, "sdlc");
    assert_eq!(
        app.extensions.loaded[extension].instance.approvals[receipt].operation,
        operation
    );
    app.handle_input(input(Key::Enter));
}

#[tokio::test]
async fn approved_extension_loads_and_runs_its_declared_command() {
    let directory = tempfile::tempdir().expect("workspace");
    let extension = directory.path().join("demo-extension");
    write_extension(
        &extension,
        r#"
import "bench/extension.tl" as extension;
fn main() { extension::set("phase", "planning"); }
"#,
    );
    let mut app = app_for(directory.path(), &directory.path().join("state")).await;
    load_approved(&mut app, &extension, "demo").await;

    assert!(app.route_extension_command("advance", &[]));
    assert!(
        app.extensions.active,
        "extension execution is background work"
    );
    app.select_work_mode("");
    assert!(
        app.messages
            .iter()
            .any(|message| message.text.contains("chat, task, workflow")),
        "the UI remains able to process a mode query while the extension is active"
    );
    next_event(&mut app).await;
    let loaded = app.extensions.loaded.get("demo").expect("loaded extension");
    assert_eq!(
        loaded.instance.values.get("phase"),
        Some(&"planning".into())
    );
    assert!(loaded.instance.events.values().count() == 1);
    app.tasks.shutdown().await;
}

#[tokio::test]
async fn extension_model_task_obeys_user_before_model_hook_before_provider_dispatch() {
    let directory = tempfile::tempdir().expect("workspace");
    let state = directory.path().join("state");
    let extension = directory.path().join("model-extension");
    std::fs::create_dir_all(&extension).expect("extension root");
    write(
        &extension,
        "extension.toml",
        r#"id = "model-demo"
version = "1.0.0"
api_version = 1
operations = ["extension.read", "extension.task", "extension.write"]

[commands]
advance = "command.tl"

[tasks]
work = "work.tl"
"#,
    );
    write(
        &extension,
        "command.tl",
        r#"import "bench/extension.tl" as extension;
fn main() {
    let action = extension::event("action");
    if action == "" { extension::start("work", "review"); }
    else { extension::set("outcome", action); }
}
"#,
    );
    write(
        &extension,
        "work.tl",
        r#"@model("sdlc-test", "sdlc-test-model", "work") agent Writer {}
impl Writer { pub task write(prompt: string) -> string { return self.think(prompt).await; } }
fn main() { entry_task(); }
task entry_task() { let writer = Writer {}; print(writer.write("review").await); }
"#,
    );

    let mut app = app_for(directory.path(), &state).await;
    let requests = install_sdlc_model(&mut app, &state).await;
    let owner = state.join("user-config.tl");
    std::fs::write(&owner, "fn main() {}\n").expect("user configuration owner");
    let owner = owner
        .canonicalize()
        .expect("canonical user configuration owner");
    let hook = state.join("deny-before-model.tl");
    std::fs::write(&hook, "fn main() { throw \"deny model\"; }\n").expect("hook script");
    let hook = hook.canonicalize().expect("canonical hook script");
    app.scripting
        .hooks
        .entry("agent.before_model".into())
        .or_default()
        .push(super::scripting::Registration {
            path: hook.clone(),
            owner,
            kind: ScriptKind::UserConfig,
        });
    load_approved(&mut app, &extension, "model-demo").await;

    assert!(app.route_extension_command("advance", &[]));
    next_event(&mut app).await;
    next_event(&mut app).await;
    next_event(&mut app).await;
    assert!(
        requests.lock().expect("model requests").is_empty(),
        "before_model denial must happen before the provider stream is called"
    );
    app.tasks.shutdown().await;
}

#[tokio::test]
async fn extension_model_task_authentication_failure_creates_no_run_or_running_entry() {
    let directory = tempfile::tempdir().expect("workspace");
    let state = directory.path().join("state");
    let extension = directory.path().join("model-extension");
    std::fs::create_dir_all(&extension).expect("extension root");
    write(
        &extension,
        "extension.toml",
        r#"id = "model-auth"
version = "1.0.0"
api_version = 1
operations = ["extension.read", "extension.task"]

[commands]
advance = "command.tl"

[tasks]
work = "work.tl"
"#,
    );
    write(
        &extension,
        "command.tl",
        r#"import "bench/extension.tl" as extension;
fn main() { extension::start("work", "review"); }
"#,
    );
    write(
        &extension,
        "work.tl",
        r#"@model("sdlc-test", "sdlc-test-model", "work") agent Writer {}
impl Writer { pub task write(prompt: string) -> string { return self.think(prompt).await; } }
fn main() { entry_task(); }
task entry_task() { let writer = Writer {}; print(writer.write("review").await); }
"#,
    );

    let mut app = app_for(directory.path(), &state).await;
    let providers = AuthProviderRegistry::new();
    providers
        .register(Arc::new(SdlcSignedOutAuth))
        .expect("register signed-out provider");
    let auth_root = state.join("auth");
    let configuration = seed_sdlc_auth(&auth_root);
    let auth = teal_model::AuthManager::open(Arc::clone(&configuration), providers)
        .expect("test auth manager");
    app.set_configured_model_access_for_test(
        configuration,
        auth,
        teal_model::ModelFactoryRegistry::new(),
    );
    let provider = AuthProviderId::new("sdlc-test").expect("provider ID");
    let profile = AuthProfileName::new("default").expect("profile");
    app.auth_for_test()
        .logout(&provider, &profile, Cancellation::new())
        .await
        .expect("signed-out profile should persist");
    app.auth_for_test()
        .set_active_in_scope(&provider, &profile, teal_model::ModelConfigScope::User)
        .expect("profile should become active");
    app.agent_settings.insert(
        "sdlc-test".into(),
        (
            state.join("sdlc-agent.toml"),
            teal_workbench_automation::AgentSettings {
                name: "sdlc-test".into(),
                provider: "sdlc-test".into(),
                profile: None,
                model: "sdlc-test-model".into(),
                compaction_profile: None,
                instructions: "Return the requested structured completion.".into(),
            },
        ),
    );
    app.active_agent = Some("sdlc-test".into());
    load_approved(&mut app, &extension, "model-auth").await;

    assert!(app.route_extension_command("advance", &[]));
    drive_until(&mut app, "extension authentication failure", false, |app| {
        app.messages
            .iter()
            .any(|message| message.text.contains("requires user authentication"))
    })
    .await;

    let task = app.extensions.loaded["model-auth"]
        .instance
        .tasks
        .values()
        .next()
        .expect("extension task should remain inspectable after preflight failure");
    assert!(
        task.run_id.is_none(),
        "authentication failure must precede run allocation"
    );
    assert!(app.extensions.authenticating.is_empty());
    assert!(app.extensions.running.is_empty());
    assert!(
        !directory
            .path()
            .join(".teal/runtime/agent.sqlite3")
            .exists(),
        "authentication failure must not open RuntimeStore"
    );
    app.tasks.shutdown().await;
}

#[tokio::test]
async fn cancelled_extension_authentication_result_cannot_start_a_run() {
    let (directory, mut app, _extension, key, release) = model_extension_waiting_for_auth().await;

    let cancellation = app.extensions.authenticating[&key].1.clone();
    cancellation.cancel();
    release.notify_one();
    next_event(&mut app).await;

    let task = app.extensions.loaded["model-auth"]
        .instance
        .tasks
        .get(&key)
        .expect("started task remains inspectable");
    assert!(
        task.run_id.is_none(),
        "an authentication result delivered after cancellation must not allocate a run"
    );
    assert!(app.extensions.authenticating.is_empty());
    assert!(app.extensions.running.is_empty());
    assert!(
        !directory
            .path()
            .join(".teal/runtime/agent.sqlite3")
            .exists(),
        "a cancelled authentication result must not open RuntimeStore"
    );
    app.tasks.shutdown().await;
}

#[tokio::test]
async fn extension_source_change_while_authentication_waits_is_rejected_before_run_allocation() {
    let (directory, mut app, extension, key, release) = model_extension_waiting_for_auth().await;
    write(
        &extension,
        "work.tl",
        "fn main() { print(\"changed after authentication began\"); }\n",
    );
    release.notify_one();

    drive_until(&mut app, "stale extension source rejection", false, |app| {
        app.messages
            .iter()
            .any(|message| message.text.contains("Extension source changed"))
    })
    .await;

    let task = app.extensions.loaded["model-auth"]
        .instance
        .tasks
        .get(&key)
        .expect("started task remains inspectable");
    assert!(
        task.run_id.is_none(),
        "a source change during authentication must be rejected before run allocation"
    );
    assert!(app.extensions.running.is_empty());
    assert!(
        app.messages
            .iter()
            .any(|message| message.text.contains("Extension source changed")),
        "the rejected asynchronous result must explain why the old approval was not used"
    );
    assert!(
        !directory
            .path()
            .join(".teal/runtime/agent.sqlite3")
            .exists(),
        "a rejected stale source must not open RuntimeStore"
    );
    app.tasks.shutdown().await;
}

#[tokio::test]
async fn approval_rejects_artifact_changes_after_the_prompt_is_created() {
    let directory = tempfile::tempdir().expect("workspace");
    std::fs::write(directory.path().join("plan.md"), "first version\n").expect("artifact");
    let extension = directory.path().join("demo-extension");
    write_extension(
        &extension,
        r#"
import "bench/extension.tl" as extension;
fn main() { extension::request_approval("Review plan", "[\"plan.md\"]", "publish", ""); }
"#,
    );
    let mut app = app_for(directory.path(), &directory.path().join("state")).await;
    load_approved(&mut app, &extension, "demo").await;
    assert!(app.route_extension_command("advance", &[]));
    next_event(&mut app).await;

    let receipt = app.extensions.loaded["demo"]
        .instance
        .approvals
        .keys()
        .next()
        .expect("approval receipt")
        .clone();
    std::fs::write(directory.path().join("plan.md"), "changed after review\n")
        .expect("changed artifact");
    app.answer_extension_approval("demo", &receipt, true);

    assert_eq!(
        app.extensions.loaded["demo"].instance.approvals[&receipt].approved,
        None
    );
    app.tasks.shutdown().await;
}

#[tokio::test]
async fn failed_run_with_candidate_delivers_task_failed_without_advancing_the_stage() {
    let directory = tempfile::tempdir().expect("workspace");
    let extension = directory.path().join("demo-extension");
    write_extension(
        &extension,
        r#"
import "bench/extension.tl" as extension;
fn main() { extension::set("phase", "planning"); }
"#,
    );
    let mut app = app_for(directory.path(), &directory.path().join("state")).await;
    load_approved(&mut app, &extension, "demo").await;

    let store = Arc::new(RuntimeStore::open(directory.path()).expect("runtime store"));
    let prepared = prepare_execution(
        &persistent_program(),
        store.as_ref(),
        "demo:event-1:0",
        &"a".repeat(64),
        ExecutionBindings::default(),
    )
    .expect("prepared task");
    let claim = store.claim_run(&prepared.run_id).expect("run claim");
    claim.start_prepared().expect("started run");
    store
        .submit_prepared_result(&prepared.run_id, "candidate")
        .expect("candidate result");
    claim
        .set_run_state(&prepared.run_id, RunState::CompletedWithErrors)
        .expect("failed terminal state");

    let key = "demo:event-1:0";
    let loaded = app
        .extensions
        .loaded
        .get_mut("demo")
        .expect("loaded extension");
    loaded
        .instance
        .values
        .insert("phase".into(), "planning".into());
    loaded.instance.tasks.insert(
        key.into(),
        ExtensionTaskRef {
            callback: None,
            key: key.into(),
            task: "work".into(),
            input: "draft".into(),
            approval_ids: Vec::new(),
            state: BTreeMap::new(),
            run_id: Some(prepared.run_id),
            control: None,
            delivered: false,
        },
    );
    app.finish_extension_task("demo", key, Ok(()));
    next_event(&mut app).await;

    let instance = &app.extensions.loaded["demo"].instance;
    assert_eq!(instance.values.get("phase"), Some(&"planning".into()));
    assert_eq!(instance.values.get("outcome"), Some(&"task_failed".into()));
    assert!(instance.tasks[key].delivered);
    app.tasks.shutdown().await;
}

#[tokio::test]
async fn extension_cancel_preserves_unknown_work_until_it_is_resolved() {
    let directory = tempfile::tempdir().expect("workspace");
    let extension = directory.path().join("demo-extension");
    write_extension(
        &extension,
        r#"
import "bench/extension.tl" as extension;
fn main() { extension::set("phase", "planning"); }
"#,
    );
    let mut app = app_for(directory.path(), &directory.path().join("state")).await;
    load_approved(&mut app, &extension, "demo").await;

    let store = Arc::new(RuntimeStore::open(directory.path()).expect("runtime store"));
    let prepared = prepare_execution(
        &persistent_program(),
        store.as_ref(),
        "demo:event-unknown:0",
        &"b".repeat(64),
        ExecutionBindings::default(),
    )
    .expect("prepared task");
    let claim = store.claim_run(&prepared.run_id).expect("run claim");
    claim.start_prepared().expect("started run");
    let operation = store
        .prepare_operation(
            &prepared.run_id,
            ExternalOperationKind::Host,
            "unknown-extension-effect",
        )
        .expect("operation intent");
    drop(claim);
    drop(store);

    let reopened = Arc::new(RuntimeStore::open(directory.path()).expect("reopened store"));
    reopened
        .set_run_state(&prepared.run_id, RunState::NeedsAttention)
        .expect("unresolved effect requires attention");
    assert_eq!(
        reopened.run(&prepared.run_id).unwrap().unwrap().state,
        RunState::NeedsAttention
    );
    assert_eq!(
        reopened.operations(&prepared.run_id).unwrap()[0].state,
        ExternalOperationState::Prepared
    );

    let key = "demo:event-unknown:0";
    app.extensions
        .loaded
        .get_mut("demo")
        .expect("loaded extension")
        .instance
        .tasks
        .insert(
            key.into(),
            ExtensionTaskRef {
                callback: None,
                key: key.into(),
                task: "work".into(),
                input: "draft".into(),
                approval_ids: Vec::new(),
                state: BTreeMap::new(),
                run_id: Some(prepared.run_id.clone()),
                control: Some(ExtensionTaskControl::Cancel),
                delivered: false,
            },
        );

    app.execute_command_line("extension resume demo");
    assert_eq!(
        reopened.run(&prepared.run_id).unwrap().unwrap().state,
        RunState::NeedsAttention,
        "cancellation must not hide an unresolved external effect"
    );
    assert_eq!(
        reopened.operations(&prepared.run_id).unwrap()[0].state,
        ExternalOperationState::Prepared
    );
    assert!(matches!(
        app.extensions.loaded["demo"].instance.tasks[key].control,
        Some(ExtensionTaskControl::Cancel)
    ));

    reopened
        .finish_operation(&operation.id, ExternalOperationState::Abandoned, None)
        .expect("explicit resolution");
    app.execute_command_line("extension resume demo");
    assert_eq!(
        reopened.run(&prepared.run_id).unwrap().unwrap().state,
        RunState::Cancelled
    );
    assert!(
        app.extensions.loaded["demo"].instance.tasks[key]
            .control
            .is_none()
    );
    app.tasks.shutdown().await;
}

#[tokio::test]
async fn shipped_sdlc_extension_loads_the_complete_package_through_bench_preflight_hosts() {
    let directory = tempfile::tempdir().expect("workspace");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/showcases/bench/sdlc");
    let mut app = app_for(directory.path(), &directory.path().join("state")).await;
    load_approved(&mut app, &root, "sdlc").await;

    let package = &app.extensions.loaded["sdlc"].package;
    assert!(package.manifest.exclusive_workspace);
    assert_eq!(package.manifest.tasks.len(), 10);
    for task in [
        "initialize",
        "intent",
        "spec",
        "plan",
        "commit",
        "build",
        "verify",
        "deliver",
        "deploy",
        "diagnose",
    ] {
        assert!(
            package.entry("task", task).is_some(),
            "missing {task} entry"
        );
    }
    assert!(package.entry("command", "sdlc").is_some());
    assert!(package.entry("mode", "sdlc").is_some());
    assert!(package.entry("panel", "sdlc").is_some());
    app.tasks.shutdown().await;
}

#[tokio::test]
async fn shipped_sdlc_extension_executes_plan_approved_delivery_and_deployment() {
    let directory = tempfile::tempdir().expect("SDLC workspace");
    let workspace = directory.path();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/showcases/bench/sdlc");
    let state = workspace.join("bench-state");
    let mut app = app_for(workspace, &state).await;
    load_approved(&mut app, &root, "sdlc").await;
    std::fs::write(workspace.join("README.md"), "SDLC test workspace\n").expect("baseline file");
    std::fs::write(
        workspace.join("verify-feature.ps1"),
        r#"
$ErrorActionPreference = "Stop"
$source = Get-Content -Raw (Join-Path $PSScriptRoot "src/feature.tl")
if ($source -notmatch 'fn feature\(\)') { throw "feature declaration is missing" }
if ($source -notmatch 'planned implementation') { throw "planned behavior is missing" }
"#,
    )
    .expect("verification script");
    git(workspace, &["init", "--quiet"]);
    git(workspace, &["config", "user.name", "SDLC Test"]);
    git(
        workspace,
        &["config", "user.email", "sdlc-test@example.invalid"],
    );
    git(workspace, &["add", "README.md", "verify-feature.ps1"]);
    git(workspace, &["commit", "--quiet", "-m", "test: baseline"]);
    let requests = install_sdlc_model(&mut app, &state).await;
    let mut config = BTreeMap::from([("agent".into(), "sdlc-test".into())]);
    for name in ["build", "test", "lint", "eval", "deploy"] {
        config.insert(
            format!("command.{name}"),
            test_command("verify-feature.ps1"),
        );
    }
    app.extensions
        .loaded
        .get_mut("sdlc")
        .expect("loaded SDLC")
        .config
        .extend(config);

    assert!(app.route_extension_command(
        "sdlc",
        &["Implement the approved feature and preserve the contract.".into()]
    ));
    drive_until(&mut app, "final plan approval", true, |app| {
        app.extensions.loaded["sdlc"].instance.values.get("stage") == Some(&"plan".into())
            && matches!(
                app.pending_overlay,
                Some(PendingOverlay::Confirmation(
                    PendingConfirmation::ExtensionApproval { .. }
                ))
            )
    })
    .await;
    let pending = app.extensions.loaded["sdlc"]
        .instance
        .approvals
        .values()
        .filter(|approval| approval.approved.is_none())
        .collect::<Vec<_>>();
    assert_eq!(pending.len(), 1, "only the final plan should need approval");
    assert_eq!(pending[0].operation, "plan");

    accept_sdlc_approval(&mut app, "plan").await;
    drive_until(&mut app, "verified delivery", true, |app| {
        app.extensions.loaded["sdlc"].instance.values.get("stage") == Some(&"deploy".into())
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(workspace.join("src/feature.tl")).expect("Builder source"),
        "fn feature() { print(\"planned implementation\"); }\n"
    );
    assert!(
        git(workspace, &["log", "-1", "--format=%s"]).contains("feat: deliver verified change"),
        "delivery must use the verified commit operation"
    );
    assert!(workspace.join(".teal/runtime/extension-evidence").is_dir());

    accept_sdlc_approval(&mut app, "deploy").await;
    drive_until(&mut app, "deployment completion", true, |app| {
        app.extensions.loaded["sdlc"].instance.values.get("stage") == Some(&"maintain".into())
    })
    .await;
    let flow = app.extensions.loaded["sdlc"].instance.values["flow_id"].clone();
    assert!(
        workspace
            .join(format!("intent/{flow}/deployment.md"))
            .is_file()
    );

    {
        let requests = requests.lock().expect("model requests");
        assert_eq!(
            requests.len(),
            8,
            "all automatic SDLC model stages must run"
        );
        assert!(requests.iter().any(|request| {
            request
                .tools
                .iter()
                .any(|tool| tool.name == "workspace.write_file")
        }));
        assert!(
            requests
                .iter()
                .all(|request| request.model == "sdlc-test-model")
        );
    }
    app.tasks.shutdown().await;
}
