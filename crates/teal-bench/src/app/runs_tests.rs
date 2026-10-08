use teal_agent::{ActionContext, UserQuestion};
use teal_workbench_services::{PanelSession as _, PanelSessionState};
use teal_workbench_services::tools::{ApprovalPrompt, ChannelApproval};
use tokio::sync::mpsc;

use super::*;

struct NoImports;

impl teal_compiler::SourceLoader for NoImports {
    fn load(
        &self,
        _: &std::path::Path,
        import: &str,
    ) -> Result<teal_compiler::LoadedSource, teal_compiler::SourceLoadError> {
        Err(teal_compiler::SourceLoadError {
            message: format!("unexpected import `{import}`"),
        })
    }
}

fn program() -> teal_agent::AgentProgram {
    let image = teal_compiler::compile(
        teal_compiler::Source::new("runs-test.tl", "fn main() {}"),
        &NoImports,
        teal_compiler::CompileOptions::default()
            .with_runtime_profile(teal_compiler::RuntimeProfile::agent()),
    )
    .unwrap()
    .program;
    teal_agent::AgentProgram::from_bytes(image.to_bytes().unwrap()).unwrap()
}

fn output_texts(app: &App) -> Vec<String> {
    app.panels
        .iter()
        .filter_map(|(_, panel)| panel.as_text())
        .filter_map(|panel| match panel.session_state() {
            PanelSessionState::Text { content, .. } => Some(content),
            _ => None,
        })
        .collect()
}

async fn execute_runs(
    app: &mut App,
    approvals: &mut mpsc::Receiver<ApprovalPrompt>,
    arguments: &[&str],
    allow: bool,
) {
    app.runs_command(
        &arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect::<Vec<_>>(),
    );
    let prompt = tokio::time::timeout(std::time::Duration::from_secs(5), approvals.recv())
        .await
        .expect("runs command should request authorization")
        .expect("approval channel should stay open");
    prompt
        .response
        .send(if allow {
            teal_agent::ApprovalResponse::Allow(teal_agent::ApprovalScope::Once)
        } else {
            teal_agent::ApprovalResponse::Deny
        })
        .expect("approval response should be received");
    let event = tokio::time::timeout(std::time::Duration::from_secs(5), app.runtime_rx.recv())
        .await
        .expect("runs command should produce a result")
        .expect("runtime event channel should stay open");
    app.handle_runtime_event(event).await;
}

#[tokio::test]
async fn runs_list_and_show_open_read_only_output_panels() {
    let workspace = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut app = super::tests::isolated_app(
        super::tests::options(workspace.path().to_path_buf()),
        state.path(),
    )
    .await
    .unwrap();
    let (approval_tx, mut approvals) = mpsc::channel(4);
    app.agent_permissions = std::sync::Arc::new(ChannelApproval::new(approval_tx));
    let store = teal_agent::RuntimeStore::open(&app.workspace).unwrap();
    let run = store.create_run(&program()).unwrap();

    execute_runs(&mut app, &mut approvals, &["list"], true).await;
    assert!(
        output_texts(&app)
            .iter()
            .any(|text| text.contains("# Persistent runs") && text.contains(&run.id))
    );
    execute_runs(&mut app, &mut approvals, &["show", &run.id], true).await;
    assert!(
        output_texts(&app)
            .iter()
            .any(|text| text.contains(&format!("# Run `{}`", run.id)))
    );
    assert_eq!(
        store.run(&run.id).unwrap().unwrap().state,
        teal_agent::RunState::Running
    );
    app.tasks.shutdown().await;
}

#[tokio::test]
async fn runs_answer_routes_to_the_durable_question_overlay() {
    let workspace = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut app = super::tests::isolated_app(
        super::tests::options(workspace.path().to_path_buf()),
        state.path(),
    )
    .await
    .unwrap();
    let store = teal_agent::RuntimeStore::open(&app.workspace).unwrap();
    let run = store.create_run(&program()).unwrap();
    let request = store
        .request_interaction(
            &ActionContext {
                run_id: run.id.clone(),
                task_id: "task".into(),
                generation: 1,
                policy_version: "test".into(),
            },
            "source",
            UserQuestion::Text {
                prompt: "Continue?".into(),
            },
        )
        .unwrap();

    app.runs_command(&["answer".into(), run.id.clone(), request.id.clone()]);
    assert!(matches!(
        app.pending_overlay,
        Some(PendingOverlay::DurableQuestion { .. })
    ));
    assert_eq!(
        store.interactions(&run.id).unwrap()[0].status,
        teal_agent::InteractionStatus::Pending
    );
    app.tasks.shutdown().await;
}

#[tokio::test]
async fn runs_profile_routes_to_the_asynchronous_management_provider() {
    let workspace = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut app = super::tests::isolated_app(
        super::tests::options(workspace.path().to_path_buf()),
        state.path(),
    )
    .await
    .unwrap();
    let (approval_tx, mut approvals) = mpsc::channel(2);
    app.agent_permissions = std::sync::Arc::new(ChannelApproval::new(approval_tx));
    let store = teal_agent::RuntimeStore::open(&app.workspace).unwrap();
    let run = store.create_run(&program()).unwrap();

    execute_runs(
        &mut app,
        &mut approvals,
        &["profile", &run.id, "../outside.pb.gz"],
        false,
    )
    .await;
    assert!(app.messages.iter().any(|message| {
        message.source == "background" && message.text.contains("not authorized")
    }));
    assert!(
        !workspace
            .path()
            .parent()
            .unwrap()
            .join("outside.pb.gz")
            .exists()
    );
    app.tasks.shutdown().await;
}
