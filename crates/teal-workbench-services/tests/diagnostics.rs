use std::{path::Path, process::Command};

use serde_json::json;
use teal_workbench_services::{DiagnosticSpan, diagnostic_context, parse_cargo_diagnostics};

fn git(workspace: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(workspace)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn compiler_message(message: serde_json::Value) -> String {
    json!({"reason":"compiler-message","message":message}).to_string()
}

#[test]
fn cargo_json_parser_preserves_windows_paths_and_related_spans_while_ignoring_noise() {
    let primary = json!({
        "file_name":"C:\\repo\\crate\\src\\lib.rs",
        "byte_start":12,
        "byte_end":18,
        "line_start":4,
        "column_start":3,
        "label":"primary",
        "is_primary":true
    });
    let related = json!({
        "file_name":"C:\\repo\\crate\\src\\other.rs",
        "byte_start":0,
        "byte_end":4,
        "line_start":1,
        "column_start":1,
        "label":"related",
        "is_primary":false
    });
    let child = json!({
        "message":"consider this related definition",
        "spans":[related]
    });
    let diagnostic = compiler_message(json!({
        "level":"error",
        "code":{"code":"E0123"},
        "message":"broken item",
        "spans":[primary],
        "children":[child]
    }));
    let output =
        format!("not json\n{{\"reason\":\"compiler-artifact\"}}\n{{bad json}}\n{diagnostic}\n");

    let parsed = parse_cargo_diagnostics(&output, 50);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].level, "error");
    assert_eq!(parsed[0].code.as_deref(), Some("E0123"));
    assert_eq!(
        parsed[0].primary.as_ref().map(|span| span.file.as_str()),
        Some("C:\\repo\\crate\\src\\lib.rs")
    );
    assert_eq!(parsed[0].related.len(), 1);
    assert_eq!(parsed[0].related[0].file, "C:\\repo\\crate\\src\\other.rs");
    assert_eq!(parsed[0].children, ["consider this related definition"]);
}

#[test]
fn cargo_json_parser_honors_the_diagnostic_limit() {
    let line = compiler_message(json!({
        "level":"warning",
        "message":"bounded",
        "spans":[],
        "children":[]
    }));
    let output = std::iter::repeat_n(line, 55).collect::<Vec<_>>().join("\n");
    assert_eq!(parse_cargo_diagnostics(&output, 50).len(), 50);
    assert!(parse_cargo_diagnostics(&output, 0).is_empty());
}

#[test]
fn diagnostic_context_prefers_shared_overlay_and_reports_git_or_uncommitted_provenance() {
    let workspace = tempfile::tempdir().expect("workspace");
    let source_dir = workspace.path().join("src");
    std::fs::create_dir_all(&source_dir).expect("source dir");
    let source = source_dir.join("lib.tl");
    let disk = (1..=10)
        .map(|line| format!("disk-{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&source, format!("{disk}\n")).expect("disk source");
    git(workspace.path(), &["init"]);
    git(
        workspace.path(),
        &["config", "user.email", "tests@example.invalid"],
    );
    git(workspace.path(), &["config", "user.name", "Teal tests"]);
    git(workspace.path(), &["add", "src/lib.tl"]);
    git(workspace.path(), &["commit", "-m", "diagnostic source"]);

    let span = DiagnosticSpan {
        file: "src/lib.tl".into(),
        byte_start: 0,
        byte_end: 1,
        line: 5,
        column: 2,
        label: None,
        primary: true,
    };
    let overlay = (1..=10)
        .map(|line| format!("overlay-{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let context = diagnostic_context(workspace.path(), &span, Some(&overlay)).expect("overlay");
    assert_eq!(context.path, "src/lib.tl");
    assert_eq!(context.context_start_line, 2);
    assert_eq!(context.context.len(), 7);
    assert_eq!(context.context[0], "overlay-2");
    assert_eq!(context.context[6], "overlay-8");
    assert!(context.commit.is_some());
    assert_eq!(context.commit_summary.as_deref(), Some("diagnostic source"));

    std::fs::write(
        &source,
        format!("{}\n", disk.replace("disk-5", "uncommitted")),
    )
    .expect("uncommitted source");
    let uncommitted = diagnostic_context(workspace.path(), &span, None).expect("disk context");
    assert_eq!(uncommitted.context[3], "uncommitted");
    assert!(uncommitted.commit.is_none());
}

#[test]
fn diagnostic_context_rejects_paths_outside_the_workspace() {
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside");
    let source = outside.path().join("outside.rs");
    std::fs::write(&source, "outside\n").expect("outside source");
    let span = DiagnosticSpan {
        file: source.to_string_lossy().into_owned(),
        byte_start: 0,
        byte_end: 1,
        line: 1,
        column: 1,
        label: None,
        primary: true,
    };
    assert!(
        diagnostic_context(workspace.path(), &span, None)
            .unwrap_err()
            .contains("outside the workspace")
    );
}
