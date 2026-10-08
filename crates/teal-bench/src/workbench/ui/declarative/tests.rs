use super::*;
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;
fn node(id: &str, content: serde_json::Value) -> serde_json::Value {
    let mut node = json!({"id":id,"width":{"fill":1},"height":"auto"});
    node["content"] = content;
    node
}
fn spec(root: serde_json::Value) -> String {
    let mut spec = json!({"version":1});
    spec["root"] = root;
    spec.to_string()
}
#[test]
fn invalid_tree_is_rejected_without_replacing_last_valid_tree() {
    let valid = spec(node(
        "text",
        json!({"kind":"text","text":"\u{4e16}\u{754c}","style":0,"wrap":true}),
    ));
    let mut panel = DeclarativePanel::from_json(&valid).unwrap();
    let duplicate = spec(node(
        "root",
        json!({"kind":"column","gap":0,"children":[node("same",json!({"kind":"spacer","cells":1})),node("same",json!({"kind":"spacer","cells":1}))]}),
    ));
    assert!(panel.replace(&duplicate).is_err());
    assert!(panel.specification().contains("\u{4e16}\u{754c}"));
    assert!(DeclarativePanel::from_json(&valid.replace("\"version\":1", "\"version\":2")).is_err());
}
#[test]
fn input_and_button_emit_typed_events_and_state_round_trips() {
    let source = spec(node(
        "root",
        json!({"kind":"column","gap":0,"children":[
            node("name",json!({"kind":"input","value":"","placeholder":"Name"})),
            node("ok",json!({"kind":"button","label":"OK","enabled":true}))
        ]}),
    ));
    let mut panel = DeclarativePanel::from_json(&source).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(50, 12)).unwrap();
    terminal
        .draw(|frame| panel.render(frame, frame.area()))
        .unwrap();
    let key = |key| InputEvent {
        key,
        modifiers: teal_workbench_core::KeyModifiers::default(),
    };
    let event = panel.input(key(Key::Char('界'))).unwrap();
    assert_eq!(event["kind"], "change");
    assert_eq!(event["value"], "\u{754c}");
    panel.input(key(Key::Tab));
    assert_eq!(panel.input(key(Key::Enter)).unwrap()["kind"], "activate");
    let state = panel.state();
    let mut restored = DeclarativePanel::from_json(&source).unwrap();
    restored.restore_state(state.clone()).unwrap();
    assert_eq!(restored.state(), state);
}
