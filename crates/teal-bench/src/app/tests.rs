use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crossterm::event::{
    KeyModifiers as CrosstermModifiers, MouseButton as CrosstermMouseButton,
    MouseEvent as CrosstermMouseEvent, MouseEventKind as CrosstermMouseEventKind,
};
use ratatui::{Terminal, backend::TestBackend};
use tempfile::tempdir;

use teal_workbench_core::{
    InputEvent, Key, KeyModifiers, KeymapPreset, LayoutTree, MouseButton, MouseEvent,
    MouseEventKind, Selection, SelectionSet, Window, WindowKind, WindowSet,
};
use teal_workbench_services::{
    ClipboardBackend, ClipboardError, PanelSession, SessionState, load_session,
};
use crate::workbench::ui::{ClipboardAction, PanelKind, Widget, render_frame};

use super::{
    App, AppOptions, MouseNormalizer, PendingConfirmation, PendingOverlay, SessionRestorePolicy,
    mutation_text,
};
use crate::presentation::{AppPanel, ExplorerPanel, TextPanel};
use teal_workbench_core::SplitAxis;
use teal_workbench_services::tools::MutationKind;

#[tokio::test]
async fn knowledge_command_reports_each_layer_and_refreshes_on_demand() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let _environment = TealHomeGuard::set(&home);
    let knowledge_root = home.join("knowledge");
    std::fs::create_dir_all(&knowledge_root).unwrap();
    std::fs::write(
        knowledge_root.join("note.md"),
        "# Bench note\n\nworkbench layer body\n",
    )
    .unwrap();
    let state = directory.path().join("state");
    let mut app = isolated_app(options(directory.path().to_path_buf()), &state)
        .await
        .expect("app should start");

    app.execute_command_line("knowledge");
    let lines = app
        .messages
        .iter()
        .filter(|message| message.source == "knowledge")
        .map(|message| message.text.clone())
        .collect::<Vec<_>>();
    assert!(
        lines.iter().any(|line| line.contains("official embedded")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("user layer: 1 files")),
        "{lines:?}"
    );
    assert!(
        !knowledge_root.join("cache").exists(),
        "displaying status must not write the derived cache"
    );

    app.execute_command_line("knowledge refresh user");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut refreshed = String::new();
    while Instant::now() < deadline {
        let Ok(event) =
            tokio::time::timeout(Duration::from_millis(100), app.runtime_rx.recv()).await
        else {
            continue;
        };
        let Some(event) = event else {
            break;
        };
        app.handle_runtime_event(event).await;
        let latest = app.messages.latest().expect("message").text.clone();
        if latest.starts_with("user:") {
            refreshed = latest;
            break;
        }
    }
    assert!(refreshed.starts_with("user:"), "{refreshed}");
    assert!(
        knowledge_root.join("cache/manifest.json").is_file(),
        "an explicit refresh must write the cache: {refreshed}"
    );
    app.tasks.shutdown().await;
}

#[tokio::test]
async fn test_command_opens_the_tests_window_and_commits_results() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path().join("state");
    let _environment = TealHomeGuard::set(&directory.path().join("home"));
    let script = directory.path().join("main.tl");
    std::fs::write(
        &script,
        "fn main() -> int { return 0; }\n@test\nfn passes() -> int { return 0; }\n",
    )
    .unwrap();
    let mut app = isolated_app(options(directory.path().to_path_buf()), &state)
        .await
        .expect("app should start");
    let script = script
        .canonicalize()
        .expect("the script path should resolve");

    app.execute_command_line("test main.tl");
    let panels = app.tests_panels.expect("the Tests window should be opened");
    assert_eq!(
        app.windows.get(panels.window).map(|window| window.kind()),
        Some(teal_workbench_core::WindowKind::Tests)
    );
    assert_eq!(app.tests_source.as_deref(), Some(script.as_path()));
    assert_eq!(app.windows.active_id(), panels.window);

    // 提交一次结果：列表按源码分组、选中第一个失败项，输出面板跟随选中行。
    app.finish_tests(
        script.clone(),
        Vec::new(),
        crate::app::tests_view::TestsOutcome {
            rows: vec![
                crate::workbench::ui::TestRow {
                    name: "passes".to_owned(),
                    source: "main.tl".to_owned(),
                    path: Some(script.clone()),
                    line: 3,
                    status: crate::workbench::ui::TestRowStatus::Passed,
                    message: String::new(),
                    output: "checked\n".to_owned(),
                },
                crate::workbench::ui::TestRow {
                    name: "fails".to_owned(),
                    source: "main.tl".to_owned(),
                    path: Some(script.clone()),
                    line: 6,
                    status: crate::workbench::ui::TestRowStatus::Failed,
                    message: "assert_equal_failed: 4 != 5".to_owned(),
                    output: String::new(),
                },
            ],
            summary: "1 passed; 1 failed; 2 selected".to_owned(),
            error: None,
        },
    );

    let widget = app
        .panels
        .get(panels.list)
        .and_then(crate::presentation::AppPanel::as_tests)
        .expect("the list panel should hold the Tests widget");
    assert_eq!(widget.selected(), 1, "the first failure is selected");
    assert_eq!(widget.rows().len(), 2);
    assert_eq!(widget.summary(), "1 passed; 1 failed; 2 selected");

    let output = app
        .panels
        .get(panels.output)
        .and_then(crate::presentation::AppPanel::as_text)
        .expect("the output panel should hold text");
    let text = match output.session_state() {
        teal_workbench_services::PanelSessionState::Text { content, .. } => content,
        other => panic!("expected a text panel session state, received {other:?}"),
    };
    assert!(text.contains("FAIL  fails"), "{text}");
    assert!(text.contains("assert_equal_failed: 4 != 5"), "{text}");
}

pub(super) fn options(path: std::path::PathBuf) -> AppOptions {
    // 文件路径作为显式起始文件；目录只用于确定工作区并落到 Agent 主窗口。
    let start_file = path.is_file().then(|| path.clone());
    AppOptions {
        start_file,
        path: Some(path),
        safe_mode: true,
        no_config: true,
        session: None,
        model_protocol_capture: None,
    }
}

pub(super) async fn isolated_app(
    options: AppOptions,
    state: &Path,
) -> Result<App, Box<dyn std::error::Error>> {
    App::new_with_session_paths(
        options,
        None,
        false,
        teal_workbench_services::agent_algorithms::ConnectivityMode::Online,
        Some(teal_workbench_services::SessionPaths {
            config: state.join("config"),
            state: state.to_owned(),
            cache: state.join("cache"),
        }),
    )
    .await
}

struct TealHomeGuard {
    previous: Option<std::ffi::OsString>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

struct TestAuditSink;

impl teal_security::AuditSink for TestAuditSink {
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

fn ensure_test_audit() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = teal_security::install_process_sink(Arc::new(TestAuditSink));
    });
}

impl TealHomeGuard {
    fn set(path: &std::path::Path) -> Self {
        let lock = super::test_support::environment_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var_os("TEAL_HOME");
        unsafe { std::env::set_var("TEAL_HOME", path) };
        Self {
            previous,
            _lock: lock,
        }
    }
}

impl Drop for TealHomeGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var("TEAL_HOME", value) },
            None => unsafe { std::env::remove_var("TEAL_HOME") },
        }
    }
}

fn key(key: Key) -> InputEvent {
    InputEvent {
        key,
        modifiers: KeyModifiers::default(),
    }
}

struct MemoryClipboard {
    value: Arc<Mutex<String>>,
    fail_write: bool,
}

impl ClipboardBackend for MemoryClipboard {
    fn read_text(&mut self) -> Result<String, ClipboardError> {
        self.value
            .lock()
            .map(|value| value.clone())
            .map_err(|error| ClipboardError::Unavailable(error.to_string()))
    }

    fn write_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        if self.fail_write {
            return Err(ClipboardError::Unavailable("busy".to_owned()));
        }
        *self
            .value
            .lock()
            .map_err(|error| ClipboardError::Unavailable(error.to_string()))? = text.to_owned();
        Ok(())
    }
}

fn editor_session(app: &App, path: &Path) -> SessionState {
    let panel = app.windows.focused();
    let mut session = SessionState::new(
        path.parent().expect("editor parent").to_path_buf(),
        WindowSet::new(Window::new(
            WindowKind::Editor,
            WindowKind::Editor.default_title(),
            LayoutTree::new(panel),
        )),
    );
    session.keymap = KeymapPreset::HelixFirst;
    session.panels.push(PanelSession {
        id: panel,
        state: teal_workbench_services::PanelSessionState::Editor {
            recovery_key: None,
            path: Some(path.to_owned()),
            selections: SelectionSet::default(),
            scroll: 0,
            column: 0,
        },
    });
    session
}

#[test]
fn session_restore_policy_accepts_only_documented_values() {
    assert_eq!(
        SessionRestorePolicy::parse("prompt"),
        Some(SessionRestorePolicy::Prompt)
    );
    assert_eq!(
        SessionRestorePolicy::parse(" ALWAYS "),
        Some(SessionRestorePolicy::Always)
    );
    assert_eq!(
        SessionRestorePolicy::parse("Never"),
        Some(SessionRestorePolicy::Never)
    );
    assert_eq!(SessionRestorePolicy::parse("sometimes"), None);
}

#[test]
fn unified_patch_requires_the_exact_digest_context_and_hunk_counts() {
    let current = "one\ntwo\nthree\nfour\nfive\n";
    let digest = teal_workbench_services::agent_algorithms::digest(current.as_bytes());
    let patch = "@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n@@ -4,2 +4,2 @@\n four\n-five\n+FIVE\n";
    let changed = mutation_text(
        current,
        MutationKind::ApplyPatch {
            expected_sha256: digest,
            patch: patch.into(),
        },
    )
    .expect("two exact hunks should apply");
    assert_eq!(changed, "one\nTWO\nthree\nfour\nFIVE\n");

    let stale = mutation_text(
        current,
        MutationKind::ApplyPatch {
            expected_sha256: "0".repeat(64),
            patch: patch.into(),
        },
    )
    .expect_err("stale input digest must fail");
    assert!(stale.contains("digest"));

    let context = mutation_text(
        current,
        MutationKind::ApplyPatch {
            expected_sha256: teal_workbench_services::agent_algorithms::digest(current.as_bytes()),
            patch: "@@ -1,2 +1,2 @@\n wrong\n-two\n+TWO\n".into(),
        },
    )
    .expect_err("mismatched context must fail");
    assert!(context.contains("context differs"));

    let counts = mutation_text(
        current,
        MutationKind::ApplyPatch {
            expected_sha256: teal_workbench_services::agent_algorithms::digest(current.as_bytes()),
            patch: "@@ -1,2 +1,1 @@\n one\n-two\n+TWO\n".into(),
        },
    )
    .expect_err("incorrect hunk counts must fail");
    assert!(counts.contains("line counts"));
}

#[test]
fn unified_patch_preserves_explicit_eof_newline_markers() {
    let apply = |current: &str, patch: &str| {
        mutation_text(
            current,
            MutationKind::ApplyPatch {
                expected_sha256: teal_workbench_services::agent_algorithms::digest(
                    current.as_bytes(),
                ),
                patch: patch.into(),
            },
        )
        .unwrap()
    };
    assert_eq!(
        apply(
            "old",
            "@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n"
        ),
        "new\n"
    );
    assert_eq!(
        apply(
            "old\n",
            "@@ -1 +1 @@\n-old\n+new\n\\ No newline at end of file\n"
        ),
        "new"
    );
    assert_eq!(
        apply("first\nlast\n", "@@ -1,2 +1 @@\n first\n-last\n"),
        "first\n"
    );
    let current = "old";
    assert!(
        mutation_text(
            current,
            MutationKind::ApplyPatch {
                expected_sha256: teal_workbench_services::agent_algorithms::digest(
                    current.as_bytes()
                ),
                patch: "@@ -1 +1 @@\n\\ No newline at end of file\n-old\n+new\n".into()
            }
        )
        .is_err()
    );
    assert_eq!(
        apply("first\nsecond\n", "@@ -1,0 +2 @@\n+inserted\n"),
        "first\ninserted\nsecond\n"
    );
    let current = "first\nsecond\n";
    assert!(
        mutation_text(
            current,
            MutationKind::ApplyPatch {
                expected_sha256: teal_workbench_services::agent_algorithms::digest(
                    current.as_bytes()
                ),
                patch: "@@ -1,0 +3 @@\n+inserted\n".into()
            }
        )
        .is_err()
    );
}

#[tokio::test]
async fn trusted_workspace_toml_config_keeps_the_builtin_default_without_logging_in() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let home = directory.path().join("home");
    let _environment = TealHomeGuard::set(&home);
    let paths = teal_config::ConfigPaths::discover(Some(directory.path()))
        .expect("fixture configuration paths should resolve");
    ensure_test_audit();
    teal_config::set_workspace_trust(&paths, teal_config::TrustLevel::Trusted)
        .expect("workspace trust should persist");
    std::fs::write(directory.path().join("coding.tl"), "task main() { }")
        .expect("source Agent application should be written");
    let workspace_config = directory.path().join(".teal/bench.toml");
    std::fs::create_dir_all(workspace_config.parent().expect("workspace config parent"))
        .expect("workspace config directory should exist");
    std::fs::write(
        &workspace_config,
        format!(
            r#"
            version = {}

            [agent_apps.coding]
            script = "coding.tl"
        "#,
            teal_config::CONFIG_VERSION
        ),
    )
    .expect("workspace config should be written");

    let app = isolated_app(
        AppOptions {
            start_file: None,
            path: Some(directory.path().to_path_buf()),
            safe_mode: false,
            no_config: false,
            session: None,
            model_protocol_capture: None,
        },
        state.path(),
    )
    .await
    .expect("trusted TOML configuration should initialize Bench");
    assert_eq!(app.active_agent.as_deref(), Some("default"));
    let (_, default) = app
        .agent_settings
        .get("default")
        .expect("built-in default agent");
    assert_eq!(default.provider, "default");
    assert_eq!(default.profile, None);
    assert_eq!(default.model, "default");
    assert!(app.agent_app_scripts.contains_key("coding"));
    assert!(
        app.agent_app_tx.is_none()
            && app.agent_app_services.is_none()
            && app.inline_completion.is_none(),
        "startup must not start the Teal Agent or inline model service"
    );
}

#[tokio::test]
async fn legacy_workspace_agent_config_is_rejected_without_logging_in() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let home = directory.path().join("home");
    let _environment = TealHomeGuard::set(&home);
    let paths = teal_config::ConfigPaths::discover(Some(directory.path()))
        .expect("fixture configuration paths should resolve");
    ensure_test_audit();
    teal_config::set_workspace_trust(&paths, teal_config::TrustLevel::Trusted)
        .expect("workspace trust should persist");
    let workspace_config = directory.path().join(".teal/bench.toml");
    std::fs::create_dir_all(workspace_config.parent().expect("workspace config parent"))
        .expect("workspace config directory should exist");
    std::fs::write(
        &workspace_config,
        format!(
            r#"
            version = {}

            [agents.coding]
            instructions = "Inspect source before editing."
        "#,
            teal_config::CONFIG_VERSION
        ),
    )
    .expect("workspace config should be written");

    let error = match isolated_app(
        AppOptions {
            start_file: None,
            path: Some(directory.path().to_path_buf()),
            safe_mode: false,
            no_config: false,
            session: None,
            model_protocol_capture: None,
        },
        state.path(),
    )
    .await
    {
        Ok(_) => panic!("legacy named Agent configuration must be rejected"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("agents"));
}

#[tokio::test]
async fn session_restore_prompt_defers_replacement_until_confirmation() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let path = directory.path().join("saved.rs");
    std::fs::write(&path, "fn restored() {}\n").expect("Write test data");
    let mut app = isolated_app(options(path.clone()), state.path())
        .await
        .expect("Application launch");

    let session = editor_session(&app, &path);
    app.begin_session_restore(session, SessionRestorePolicy::Prompt);

    assert!(!app.overlays.is_empty());
    assert!(matches!(
        &app.pending_overlay,
        Some(PendingOverlay::Confirmation(
            PendingConfirmation::RestoreSession(_)
        ))
    ));
    assert_eq!(
        app.panels
            .get(app.windows.focused())
            .expect("startup panel")
            .kind(),
        PanelKind::Editor
    );

    app.handle_input(key(Key::Char('y')));

    assert!(app.overlays.is_empty());
    assert_eq!(app.keymap, KeymapPreset::HelixFirst);
    assert!(
        app.panels
            .get(app.windows.focused())
            .and_then(|panel| panel.as_editor())
            .is_some()
    );
}

#[tokio::test]
async fn always_restore_policy_bypasses_the_confirmation_overlay() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let path = directory.path().join("saved.rs");
    std::fs::write(&path, "fn restored() {}\n").expect("Write test data");
    let mut app = isolated_app(options(directory.path().to_path_buf()), state.path())
        .await
        .expect("Application launch");

    let session = editor_session(&app, &path);
    app.begin_session_restore(session, SessionRestorePolicy::Always);

    assert!(app.overlays.is_empty());
    assert!(app.pending_overlay.is_none());
    assert!(
        app.panels
            .get(app.windows.focused())
            .and_then(|panel| panel.as_editor())
            .is_some()
    );
}

#[tokio::test]
async fn never_restore_policy_keeps_the_fresh_workspace_without_prompting() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let path = directory.path().join("saved.rs");
    std::fs::write(&path, "fn restored() {}\n").expect("Write test data");
    let mut app = isolated_app(options(path.clone()), state.path())
        .await
        .expect("Application launch");
    let session = editor_session(&app, &path);

    app.begin_session_restore(session, SessionRestorePolicy::Never);

    assert!(app.overlays.is_empty());
    assert!(app.pending_overlay.is_none());
    assert_eq!(
        app.panels
            .get(app.windows.focused())
            .expect("new workspace panel")
            .kind(),
        PanelKind::Editor
    );
}

#[tokio::test]
async fn declining_session_restore_keeps_the_fresh_workspace() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let path = directory.path().join("saved.rs");
    std::fs::write(&path, "fn restored() {}\n").expect("Write test data");
    let mut app = isolated_app(options(path.clone()), state.path())
        .await
        .expect("Application launch");
    let session = editor_session(&app, &path);
    app.begin_session_restore(session, SessionRestorePolicy::Prompt);

    app.handle_input(key(Key::Char('n')));

    assert!(app.overlays.is_empty());
    assert!(app.pending_overlay.is_none());
    assert_eq!(
        app.panels
            .get(app.windows.focused())
            .expect("new workspace panel")
            .kind(),
        PanelKind::Editor
    );
}

#[tokio::test]
async fn directory_start_opens_the_agent_primary_window() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let app = isolated_app(options(directory.path().to_path_buf()), state.path())
        .await
        .expect("Application launch");
    // 目录启动落到 Agent 主窗口；资源管理器按需单独打开。
    let primary = app.windows.primary();
    assert_eq!(
        app.windows.get(primary).map(|window| window.kind()),
        Some(WindowKind::Agent)
    );
    let focused = app
        .panels
        .get(app.windows.focused())
        .expect("focused panel");
    assert_eq!(focused.kind(), PanelKind::Agent);
}

#[tokio::test]
async fn colon_from_the_agent_session_uses_the_main_composer_and_executes_there() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let mut app = isolated_app(options(directory.path().to_path_buf()), state.path())
        .await
        .expect("Application launch");
    let session = app.agent_panel.expect("the Agent session panel");
    let _ = app.windows.focus(session);

    app.handle_input(key(Key::Char(':')));

    let composer = app.windows.focused();
    assert!(matches!(
        app.panels.get(composer),
        Some(AppPanel::AgentComposer(_))
    ));
    assert!(
        app.cmdline.is_none(),
        "the bottom command line stays closed"
    );
    assert_eq!(
        app.panels
            .get(composer)
            .and_then(AppPanel::as_composer)
            .map(|composer| composer.text()),
        Some(":"),
        "the colon is inserted into the main composer"
    );

    for character in "messages".chars() {
        app.handle_input(key(Key::Char(character)));
    }
    app.handle_input(key(Key::Enter));

    assert!(
        app.cmdline.is_none(),
        "the command never uses the bottom input"
    );
    assert!(
        app.panels.iter().any(|(_, panel)| {
            panel.kind() == PanelKind::Messages && panel.title() == "Messages"
        })
    );
}

#[tokio::test]
async fn explorer_commands_use_the_cursor_context_directory() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let folder = directory.path().join("folder");
    let destination = folder.join("dest");
    std::fs::create_dir_all(&destination).expect("directory test data");
    let mut app = isolated_app(options(directory.path().to_path_buf()), state.path())
        .await
        .expect("Application launch");
    // 资源管理器在独立窗口中打开；文件操作使用该窗口的目录上下文。
    app.add_panel(
        AppPanel::from(ExplorerPanel::new(directory.path(), true).expect("file explorer panel")),
        SplitAxis::Rows,
    );
    let panel_id = app.windows.focused();
    let explorer = app
        .panels
        .get_mut(panel_id)
        .and_then(|panel| panel.as_explorer_mut())
        .expect("file explorer panel");
    explorer.handle_input(key(Key::Escape));
    explorer.handle_input(key(Key::Char('j')));

    app.execute_command_line("file-new created.txt");
    app.settle_mutations().await;
    assert!(folder.join("created.txt").is_file());
    app.execute_command_line("copy dest");
    app.settle_mutations().await;
    assert!(destination.join("created.txt").is_file());
    app.execute_command_line("file-new moved.txt");
    app.settle_mutations().await;
    app.execute_command_line("move dest");
    app.settle_mutations().await;
    assert!(!folder.join("moved.txt").exists());
    assert!(destination.join("moved.txt").is_file());
}

#[tokio::test]
async fn explorer_session_restores_expansion_cursor_and_view_options() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let folder = directory.path().join("folder");
    std::fs::create_dir(&folder).expect("folder test data");
    std::fs::write(folder.join("nested.txt"), "nested").expect("file test data");
    let mut app = isolated_app(options(directory.path().to_path_buf()), state.path())
        .await
        .expect("Application launch");
    app.add_panel(
        AppPanel::from(ExplorerPanel::new(directory.path(), true).expect("file explorer panel")),
        SplitAxis::Rows,
    );
    app.session_path = directory.path().join("session.json");
    let panel_id = app.windows.focused();
    let explorer = app
        .panels
        .get_mut(panel_id)
        .and_then(|panel| panel.as_explorer_mut())
        .expect("file explorer panel");
    explorer.handle_input(key(Key::Escape));
    explorer.handle_input(key(Key::Char('j')));
    explorer.handle_input(key(Key::Char('l')));
    explorer.settle();
    explorer.handle_input(key(Key::Char('l')));
    explorer.handle_input(key(Key::Char('.')));
    explorer.settle();
    let expected = explorer.session_state();

    app.save_session().expect("save session");
    let saved = load_session(&app.session_path).expect("load session");
    assert_eq!(saved.version, 8);
    assert!(app.restore_session(saved));
    let restored = app
        .panels
        .get(app.windows.focused())
        .and_then(|panel| panel.as_explorer())
        .expect("restored file explorer");
    assert_eq!(restored.session_state(), expected);
}

#[tokio::test]
async fn file_start_edits_and_saves_through_the_modal_transaction_path() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let path = directory.path().join("main.rs");
    std::fs::write(&path, "fn main() {}\n").expect("Write test data");
    let mut app = isolated_app(options(path.clone()), state.path())
        .await
        .expect("Application launch");
    app.handle_input(key(Key::Char('i')));
    app.handle_input(key(Key::Char('/')));
    // 斜杠仅在普通模式下打开搜索；插入模式下作为文本输入。
    app.handle_input(key(Key::Escape));
    app.save_focused();
    app.settle_mutations().await;
    assert!(
        std::fs::read_to_string(path)
            .expect("saved files")
            .starts_with('/')
    );
    assert!(
        app.panels
            .get(app.windows.focused())
            .and_then(|panel| panel.as_editor())
            .is_some()
    );
}

#[tokio::test]
async fn agent_mutations_use_the_open_document_transaction_and_save_boundary() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let path = directory.path().join("main.rs");
    std::fs::write(&path, "old").expect("Write test data");
    let mut app = isolated_app(options(path.clone()), state.path())
        .await
        .expect("Application launch");
    app.apply_agent_mutation(
        &path,
        MutationKind::Replace {
            old: "old".to_owned(),
            new: "new".to_owned(),
        },
    )
    .expect("Agent modification");
    assert_eq!(
        std::fs::read_to_string(&path).expect("Saved changes"),
        "new"
    );
    let editor = app
        .panels
        .get(app.windows.focused())
        .and_then(|panel| panel.as_editor())
        .expect("focused editor");
    assert_eq!(editor.document().to_string(), "new");
    assert!(!editor.document().is_dirty());
}

#[test]
fn mouse_normalizer_counts_only_same_cell_button_within_half_second() {
    let raw = CrosstermMouseEvent {
        kind: CrosstermMouseEventKind::Down(CrosstermMouseButton::Left),
        column: 4,
        row: 2,
        modifiers: CrosstermModifiers::NONE,
    };
    let start = Instant::now();
    let mut normalizer = MouseNormalizer::default();
    assert_eq!(normalizer.normalize(raw, start).click_count, 1);
    assert_eq!(
        normalizer
            .normalize(raw, start + Duration::from_millis(300))
            .click_count,
        2
    );
    assert_eq!(
        normalizer
            .normalize(raw, start + Duration::from_millis(900))
            .click_count,
        1
    );
}

#[tokio::test]
async fn failed_clipboard_write_does_not_cut_and_successful_paste_normalizes_newlines() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let path = directory.path().join("main.rs");
    std::fs::write(&path, "old").expect("Write test data");
    let mut app = isolated_app(options(path), state.path())
        .await
        .expect("Application launch");
    let panel = app.windows.focused();
    let editor = app
        .panels
        .get_mut(panel)
        .and_then(|panel| panel.as_editor_mut())
        .expect("editor");
    editor
        .document_mut()
        .set_selections(SelectionSet::single(Selection { anchor: 0, head: 3 }));
    let value = Arc::new(Mutex::new(String::new()));
    app.clipboard = Box::new(MemoryClipboard {
        value: Arc::clone(&value),
        fail_write: true,
    });
    app.apply_panel_clipboard(panel, ClipboardAction::Cut);
    assert_eq!(
        app.panels
            .get(panel)
            .and_then(|panel| panel.as_editor())
            .expect("editor")
            .document()
            .to_string(),
        "old"
    );

    *value.lock().expect("clipboard lock") = "new\r\ntext".to_owned();
    app.clipboard = Box::new(MemoryClipboard {
        value,
        fail_write: false,
    });
    app.apply_panel_clipboard(panel, ClipboardAction::Paste);
    assert_eq!(
        app.panels
            .get(panel)
            .and_then(|panel| panel.as_editor())
            .expect("editor")
            .document()
            .to_string(),
        "new\ntext"
    );
}

#[tokio::test]
async fn click_focuses_panel_while_hovered_wheel_preserves_focus() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let mut app = isolated_app(options(directory.path().to_path_buf()), state.path())
        .await
        .expect("Application launch");
    let first = app.windows.focused();
    let second = app.add_panel(
        TextPanel::plain(PanelKind::Output, "Output", "one\ntwo\nthree"),
        teal_workbench_core::SplitAxis::Columns,
    );
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            app.frame_geometry = render_frame(
                frame,
                &app.windows,
                &mut app.panels,
                app.cmdline.as_ref(),
                &app.completion_menu,
                &app.messages,
            );
        })
        .expect("draw");
    let first_area = app
        .frame_geometry
        .panel(first)
        .expect("first geometry")
        .body;
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: first_area.x + 2,
        row: first_area.y + 2,
        modifiers: KeyModifiers::default(),
        click_count: 1,
    });
    assert_eq!(app.windows.focused(), first);
    let second_area = app
        .frame_geometry
        .panel(second)
        .expect("second geometry")
        .body;
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: second_area.x + 2,
        row: second_area.y + 2,
        modifiers: KeyModifiers::default(),
        click_count: 0,
    });
    assert_eq!(app.windows.focused(), first);
}

/// 构造一行测试结果；`status` 见 [`crate::workbench::ui::TestRowStatus`]。
fn tests_row(
    name: &str,
    script: &std::path::Path,
    status: crate::workbench::ui::TestRowStatus,
    message: &str,
) -> crate::workbench::ui::TestRow {
    crate::workbench::ui::TestRow {
        name: name.to_owned(),
        source: "main.tl".to_owned(),
        path: Some(script.to_path_buf()),
        line: 4,
        status,
        message: message.to_owned(),
        output: String::new(),
    }
}

/// 读取输出面板的当前纯文本。
fn panel_text(app: &App, panel: teal_workbench_core::PanelId) -> String {
    match app
        .panels
        .get(panel)
        .and_then(crate::presentation::AppPanel::as_text)
        .map(crate::presentation::TextPanel::session_state)
    {
        Some(teal_workbench_services::PanelSessionState::Text { content, .. }) => content,
        _ => String::new(),
    }
}

#[tokio::test]
async fn tests_selection_links_output_and_session_restore_rebinds_the_window() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path().join("state");
    let _environment = TealHomeGuard::set(&directory.path().join("home"));
    let script = directory.path().join("main.tl");
    std::fs::write(
        &script,
        "fn main() -> int { return 0; }\n@test\nfn passes() -> int { return 0; }\n",
    )
    .unwrap();
    let script = script.canonicalize().unwrap();
    let mut app = isolated_app(options(directory.path().to_path_buf()), &state)
        .await
        .expect("app should start");

    app.finish_tests(
        script.clone(),
        Vec::new(),
        crate::app::tests_view::TestsOutcome {
            rows: vec![
                tests_row(
                    "passes",
                    &script,
                    crate::workbench::ui::TestRowStatus::Passed,
                    "",
                ),
                tests_row(
                    "fails",
                    &script,
                    crate::workbench::ui::TestRowStatus::Failed,
                    "assert_equal_failed: 4 != 5",
                ),
            ],
            summary: "1 passed; 1 failed; 2 selected".to_owned(),
            error: None,
        },
    );
    let panels = app.tests_panels.expect("the Tests window should be opened");

    // 选中行变化必须刷新输出面板。
    app.execute_command_line("test-select 0");
    let first = panel_text(&app, panels.output);
    assert!(first.contains("ok  passes"), "{first}");
    app.execute_command_line("test-select 1");
    let second = panel_text(&app, panels.output);
    assert!(second.contains("FAIL  fails"), "{second}");
    assert!(second.contains("assert_equal_failed: 4 != 5"), "{second}");

    // 恢复会话后 Tests 窗口仍绑定列表与输出面板，并保留源码路径与选中行。
    app.save_session().expect("session should save");
    let session = teal_workbench_services::load_session(&app.session_path)
        .expect("saved session should load");
    let mut restored = isolated_app(options(directory.path().to_path_buf()), &state)
        .await
        .expect("second app should start");
    assert!(restored.restore_session(session), "session should restore");
    let restored_panels = restored
        .tests_panels
        .expect("the Tests window binding should be restored");
    assert_eq!(restored.tests_source.as_deref(), Some(script.as_path()));
    let widget = restored
        .panels
        .get(restored_panels.list)
        .and_then(crate::presentation::AppPanel::as_tests)
        .expect("the restored list panel should hold the Tests widget");
    assert_eq!(widget.rows().len(), 2);
    assert_eq!(widget.selected(), 1);
    assert_eq!(widget.summary(), "1 passed; 1 failed; 2 selected");
    restored.execute_command_line("test-select 1");
    assert!(
        panel_text(&restored, restored_panels.output).contains("assert_equal_failed"),
        "the restored output panel must follow the selection"
    );
}

#[tokio::test]
async fn concurrent_test_runs_are_rejected_before_starting_a_second_one() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path().join("state");
    let _environment = TealHomeGuard::set(&directory.path().join("home"));
    std::fs::write(
        directory.path().join("main.tl"),
        "fn main() -> int { return 0; }\n@test\nfn passes() -> int { return 0; }\n",
    )
    .unwrap();
    let mut app = isolated_app(options(directory.path().to_path_buf()), &state)
        .await
        .expect("app should start");

    app.tests_run_in_flight = true;
    app.execute_command_line("test main.tl");
    assert_eq!(
        app.messages.latest().expect("warning message").text,
        "A test run is already in progress"
    );
    assert!(app.tests_source.is_none(), "the second run must not start");
}

#[tokio::test]
async fn closing_the_debug_window_stops_the_session_without_reopening_panels() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path().join("state");
    let _environment = TealHomeGuard::set(&directory.path().join("home"));
    let mut app = isolated_app(options(directory.path().to_path_buf()), &state)
        .await
        .expect("app should start");

    // 构造一个绑定到 Debug 窗口的活动会话：关闭窗口必须终止工作线程并清除绑定。
    let (window, output) = app.bind_debug_session();
    app.debug_window = Some(window);
    app.debug_panel = Some(output);
    let (control, received) = std::sync::mpsc::channel();
    app.debug_control = Some(control);
    assert_eq!(app.windows.active_id(), window);
    assert_eq!(
        app.windows.get(window).map(|window| window.kind()),
        Some(teal_workbench_core::WindowKind::Debug)
    );

    app.close_active_window(true);
    assert!(
        matches!(
            received.try_recv(),
            Ok(crate::integration::debug::DebugCommand::Stop)
        ),
        "closing the window must ask the worker to stop"
    );
    assert!(app.debug_control.is_none());
    assert!(app.debug_window.is_none());
    assert!(app.debug_panel.is_none());

    // 迟到的终态事件不得重新创建 Debugging 面板。
    let panels_after_close = app.panels.iter().count();
    app.handle_debug_event(crate::integration::debug::DebugEvent::Finished(Err(
        "terminated".to_owned(),
    )));
    assert_eq!(app.panels.iter().count(), panels_after_close);
    assert!(app.debug_panel.is_none());
}
