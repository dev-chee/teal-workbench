use crate::{
    presentation::{AgentBlock, TextPanel},
    workbench::ui::PanelKind,
};

#[test]
fn diagnostic_context_block_keeps_provenance_symbol_context_and_related_spans() {
    let block = AgentBlock::DiagnosticContext {
        source: "src/lib.tl".into(),
        line: 5,
        column: 3,
        symbol: Some("entry".into()),
        commit: Some("0123456789abcdef".into()),
        commit_summary: Some("diagnostic source".into()),
        context: vec!["line-2".into(), "line-3".into(), "line-4".into()],
        related: vec!["src/other.tl:8:1".into()],
    };
    let mut panel = TextPanel::plain(PanelKind::Agent, "Agent", "");
    panel.push_agent_block(block.clone());

    assert_eq!(panel.agent_blocks(), &[block]);
    let session = panel.session_state();
    let text = match session {
        teal_workbench_services::PanelSessionState::Text { content, .. } => content,
        _ => panic!("Agent panel must persist as text"),
    };
    assert!(text.contains("[Diagnostic context] src/lib.tl:5:3"));
    assert!(text.contains("- symbol: entry"));
    assert!(text.contains("- commit: 0123456789ab diagnostic source"));
    assert!(text.contains("line-2"));
    assert!(text.contains("- related: src/other.tl:8:1"));
}
