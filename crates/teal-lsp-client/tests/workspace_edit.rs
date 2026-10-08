//! 公开工作区编辑 API 的协议中立回归测试。

use std::collections::HashMap;

use serde_json::json;
use teal_lsp_client::{WorkspaceEditError, WorkspaceOperation, parse_workspace_edit};

#[test]
fn parses_utf16_edits_without_a_workbench_transaction_dependency() {
    let uri = "file:///C:/work/main.rs";
    let documents = HashMap::from([(uri.to_owned(), "a\u{1f642}b\n".to_owned())]);
    let edit = json!({"documentChanges":[
        {"textDocument":{"uri":uri,"version":3},"edits":[{"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":3}},"newText":"boundary"}]},
        {"kind":"create","uri":"file:///C:/work/new.rs"}
    ]});

    let operations = parse_workspace_edit(&edit, &documents).expect("effective workspace edit");
    let WorkspaceOperation::Edit(document) = &operations[0] else {
        panic!("the first operation must edit the document");
    };
    assert_eq!(document.version, Some(3));
    assert_eq!(document.edits.len(), 1);
    assert_eq!(document.edits[0].range, 1..2);
    assert_eq!(document.edits[0].insert, "boundary");
    assert!(matches!(operations[1], WorkspaceOperation::Create(_)));
}

#[test]
fn rejects_unknown_snapshots_utf16_surrogate_splits_and_overlapping_edits() {
    let uri = "file:///C:/work/main.rs";
    let documents = HashMap::from([(uri.to_owned(), "a\u{1f642}b".to_owned())]);

    let missing = json!({"changes":{"file:///C:/work/missing.rs":[]}});
    assert!(matches!(
        parse_workspace_edit(&missing, &documents),
        Err(WorkspaceEditError::MissingDocument(_))
    ));

    let surrogate_split = json!({
        "changes": HashMap::from([(uri, json!([{
            "range":{"start":{"line":0,"character":2},"end":{"line":0,"character":3}},
            "newText":"x"
        }]))])
    });
    assert!(matches!(
        parse_workspace_edit(&surrogate_split, &documents),
        Err(WorkspaceEditError::Malformed(_))
    ));

    let overlap = json!({
        "changes": HashMap::from([(uri, json!([
            {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"newText":"x"},
            {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"newText":"y"}
        ]))])
    });
    assert!(matches!(
        parse_workspace_edit(&overlap, &documents),
        Err(WorkspaceEditError::Overlap)
    ));
}
