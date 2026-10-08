use std::path::Path;

use crate::presentation::AgentBlock;
use teal_workbench_core::{Document, TextEdit, Transaction};

use super::{App, AppOptions, AppPanel};

async fn app_for(workspace: &Path) -> App {
    let state = workspace.join("state");
    App::new_with_session_paths(
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
        Some(teal_workbench_services::SessionPaths {
            config: state.join("config"),
            state: state.clone(),
            cache: state.join("cache"),
        }),
    )
    .await
    .expect("isolated Bench")
}

#[tokio::test]
async fn agent_view_projects_requested_analysis_and_rejects_invalid_requests() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(
        workspace.path().join("graph.tl"),
        "fn helper(value: int) -> int { return value; } fn main() { let value: int = 1; let copy = value; helper(copy); }",
    )
    .expect("source");
    let mut app = app_for(workspace.path()).await;

    for view in ["ast", "types", "data", "effects"] {
        app.execute_command_line(&format!(":agent-view {view} graph.tl"));
    }
    let panel = app.agent_panel.expect("Agent panel");
    let blocks = app
        .panels
        .get(panel)
        .and_then(AppPanel::as_text)
        .expect("Agent text panel")
        .agent_blocks();
    for view in ["ast", "types", "data", "effects"] {
        assert!(blocks.iter().any(|block| matches!(
            block,
            AgentBlock::AnalysisGraph { source, view: actual, .. }
                if source == "graph.tl" && actual == view
        )));
    }
    assert!(blocks.iter().any(|block| matches!(
        block,
        AgentBlock::AnalysisGraph { view, nodes, edges, .. }
            if view == "ast" && !nodes.is_empty() && !edges.is_empty()
    )));

    app.execute_command_line(":agent-view unknown graph.tl");
    let blocks = app
        .panels
        .get(panel)
        .and_then(AppPanel::as_text)
        .expect("Agent text panel")
        .agent_blocks();
    assert!(matches!(
        blocks.last(),
        Some(AgentBlock::Diagnostic { summary, detail })
            if summary == "Unknown Teal analysis view" && detail.contains("ast, types, data or effects")
    ));
    app.tasks.shutdown().await;
}

#[tokio::test]
async fn agent_view_projects_imported_modules_from_the_compilation_graph() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(
        workspace.path().join("leaf.tl"),
        "pub fn answer() -> int { return 7; }",
    )
    .expect("dependency source");
    std::fs::write(
        workspace.path().join("root.tl"),
        "import \"leaf.tl\" as leaf; fn main() { print(leaf::answer()); }",
    )
    .expect("entry source");
    let mut app = app_for(workspace.path()).await;

    app.execute_command_line(":agent-view ast root.tl");
    let panel = app.agent_panel.expect("Agent panel");
    let blocks = app
        .panels
        .get(panel)
        .and_then(AppPanel::as_text)
        .expect("Agent text panel")
        .agent_blocks();
    let nodes = blocks
        .iter()
        .find_map(|block| match block {
            AgentBlock::AnalysisGraph {
                source,
                view,
                nodes,
                ..
            } if source == "root.tl" && view == "ast" => Some(nodes),
            _ => None,
        })
        .expect("AST graph block");
    assert!(nodes.iter().any(|node| node.contains("root.tl")));
    assert!(nodes.iter().any(|node| node.contains("leaf.tl")));
    app.tasks.shutdown().await;
}

#[tokio::test]
async fn agent_source_queries_share_unsaved_entry_and_import_buffers() {
    let workspace = tempfile::tempdir().expect("workspace");
    let dependency = workspace.path().join("draft.tl");
    let entry = workspace.path().join("main.tl");
    std::fs::write(
        &dependency,
        "pub fn disk_only() -> string { return \"disk\"; }",
    )
    .expect("dependency source");
    std::fs::write(
        &entry,
        "import \"draft.tl\" as draft; fn main() { draft::missing(); }",
    )
    .expect("entry source");
    let mut app = app_for(workspace.path()).await;

    let dependency_buffer = app
        .buffers
        .insert(Document::open(&dependency).expect("dependency document"));
    let dependency_len = dependency_buffer.text().len_chars();
    dependency_buffer
        .apply(
            Transaction::new(vec![TextEdit {
                range: 0..dependency_len,
                insert: "pub fn answer() -> int { return 42; }".into(),
            }])
            .unwrap(),
        )
        .unwrap();
    let entry_buffer = app
        .buffers
        .insert(Document::open(&entry).expect("entry document"));
    let entry_len = entry_buffer.text().len_chars();
    entry_buffer
        .apply(
            Transaction::new(vec![TextEdit {
                range: 0..entry_len,
                insert: "import \"draft.tl\" as draft; fn helper() -> int { return draft::answer(); } fn main() { print(helper()); }".into(),
            }])
            .unwrap(),
        )
        .unwrap();

    app.execute_command_line(":agent-view ast main.tl");
    app.execute_command_line(":agent-imports main.tl");
    app.execute_command_line(":agent-references main.tl");
    let panel = app.agent_panel.expect("Agent panel");
    let blocks = app
        .panels
        .get(panel)
        .and_then(AppPanel::as_text)
        .expect("Agent text panel")
        .agent_blocks();
    assert!(blocks.iter().any(|block| matches!(
        block,
        AgentBlock::AnalysisGraph { nodes, .. }
            if nodes.iter().any(|node| node.contains("draft.tl"))
    )));
    assert!(blocks.iter().any(|block| matches!(
        block,
        AgentBlock::ModuleImports { imports, .. }
            if imports.iter().any(|import| import == "draft.tl as draft")
    )));
    assert!(blocks.iter().any(|block| matches!(
        block,
        AgentBlock::References { items, .. } if !items.is_empty()
    )));
    assert!(!blocks.iter().any(|block| matches!(
        block,
        AgentBlock::Diagnostic { summary, .. } if summary == "Teal analysis failed"
    )));
    app.tasks.shutdown().await;
}
