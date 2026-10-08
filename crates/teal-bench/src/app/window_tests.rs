//! 窗口层行为：标签条、切换、关闭与会话往返。

use ratatui::{Terminal, backend::TestBackend};
use tempfile::tempdir;

use teal_workbench_core::{
    InputEvent, Key, KeyModifiers, LayoutTree, PanelId, PanelSize, Window, WindowKind, WindowSet,
};
use teal_workbench_services::load_session;

use super::App;
use super::tests::{isolated_app, options};
use crate::workbench::ui::{ComposerMode, PanelKind, Widget, render_frame};

fn draw(app: &mut App) {
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
}

#[tokio::test]
async fn windows_open_switch_and_close_without_losing_panels() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let mut app = isolated_app(options(directory.path().to_path_buf()), state.path())
        .await
        .expect("Application launch");
    let primary = app.windows.active_id();
    let primary_panel = app.windows.focused();

    draw(&mut app);
    assert!(
        app.frame_geometry.tab_strip.is_none(),
        "a single window must not spend a row on the tab strip"
    );

    app.open_window(WindowKind::Editor);
    assert_eq!(app.windows.len(), 2);
    let secondary = app.windows.active_id();
    assert_ne!(secondary, primary);
    let secondary_panel = app.windows.focused();
    assert_ne!(secondary_panel, primary_panel);

    draw(&mut app);
    assert!(
        app.frame_geometry.tab_strip.is_some(),
        "multiple windows must render the tab strip"
    );
    assert_eq!(app.frame_geometry.tabs.len(), 2);
    let (first_id, first_rect) = app.frame_geometry.tabs[0];
    assert_eq!(first_id, primary);
    app.select_tab(first_rect.x, first_rect.y);
    assert_eq!(app.windows.active_id(), primary);
    assert_eq!(app.windows.focused(), primary_panel);

    // 主窗口保留，关闭次要窗口后回到主窗口。
    assert!(app.windows.select(secondary));
    app.close_active_window(true);
    assert_eq!(app.windows.len(), 1);
    assert_eq!(app.windows.active_id(), primary);
    assert_eq!(app.windows.focused(), primary_panel);
    assert!(app.panels.get(secondary_panel).is_none());
    assert!(app.panels.get(primary_panel).is_some());
}

#[tokio::test]
async fn the_primary_window_cannot_be_closed() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let mut app = isolated_app(options(directory.path().to_path_buf()), state.path())
        .await
        .expect("Application launch");
    let primary = app.windows.active_id();
    app.close_active_window(true);
    assert_eq!(app.windows.len(), 1);
    assert_eq!(app.windows.active_id(), primary);
}

#[tokio::test]
async fn sessions_round_trip_multiple_windows() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let mut app = isolated_app(options(directory.path().to_path_buf()), state.path())
        .await
        .expect("Application launch");
    let primary = app.windows.active_id();
    app.open_window(WindowKind::Agent);
    let agent_window = app.windows.active_id();
    app.save_session().expect("save session");
    let saved = load_session(&app.session_path).expect("load session");
    assert_eq!(saved.version, 8);
    assert_eq!(saved.windows.len(), 2);
    assert_eq!(saved.windows.active_id(), agent_window);
    assert_eq!(saved.windows.iter().next().map(|(id, _)| id), Some(primary));
    assert_eq!(
        saved.windows.get(agent_window).map(|window| window.kind()),
        Some(WindowKind::Agent)
    );

    let mut restored = isolated_app(options(directory.path().to_path_buf()), state.path())
        .await
        .expect("Application launch");
    assert!(restored.restore_session(saved));
    assert_eq!(restored.windows.len(), 2);
    assert_eq!(
        restored.windows.active().kind(),
        WindowKind::Agent,
        "the active window must survive the round trip"
    );
}

#[tokio::test]
async fn command_completions_follow_the_active_window_scope() {
    use teal_workbench_core::Key;

    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let mut app = isolated_app(options(directory.path().to_path_buf()), state.path())
        .await
        .expect("Application launch");

    // Agent 主窗口只有一个输入入口：会话区的 `:` 把焦点交给主输入框，命令文本不会
    // 落入底部命令行。主输入框不展示全局命令补全，因此也不会泄漏 Debug 专属命令。
    let session = app.agent_panel.expect("the Agent session panel");
    let _ = app.windows.focus(session);
    app.handle_input(InputEvent {
        key: Key::Char(':'),
        modifiers: KeyModifiers::default(),
    });
    app.handle_input(InputEvent {
        key: Key::Char('b'),
        modifiers: KeyModifiers::default(),
    });
    let composer = app.windows.focused();
    assert_ne!(composer, session);
    assert_eq!(
        app.panels
            .get(composer)
            .and_then(|panel| panel.as_composer())
            .map(|composer| composer.text()),
        Some(":b")
    );
    assert!(app.cmdline.is_none());
    assert!(
        app.completion_menu.is_empty(),
        "{:?}",
        app.completion_menu.candidates()
    );

    // Debug 输出面板仍使用底部命令行；该范围内同一前缀包含 Debug 命令。
    app.completion_menu.clear();
    app.open_window(WindowKind::Debug);
    let console = app.windows.focused();
    let output = app
        .windows
        .active()
        .validate()
        .expect("valid Debug window")
        .into_iter()
        .find(|panel| *panel != console)
        .expect("the Debug window has an output panel");
    let _ = app.windows.focus(output);
    app.handle_input(InputEvent {
        key: Key::Char(':'),
        modifiers: KeyModifiers::default(),
    });
    app.handle_input(InputEvent {
        key: Key::Char('b'),
        modifiers: KeyModifiers::default(),
    });
    assert!(
        app.completion_menu
            .candidates()
            .iter()
            .any(|name| name == "break"),
        "{:?}",
        app.completion_menu.candidates()
    );

    // Rows 键移动选择,Enter 接受候选写回命令行首词。
    app.handle_input(InputEvent {
        key: Key::Down,
        modifiers: KeyModifiers::default(),
    });
    let selected = app.completion_menu.selected();
    assert!(selected > 0, "Down must move the completion selection");
    let expected = app
        .completion_menu
        .current()
        .expect("a selected candidate")
        .to_owned();
    app.handle_input(InputEvent {
        key: Key::Enter,
        modifiers: KeyModifiers::default(),
    });
    assert_eq!(
        app.cmdline.as_ref().map(|cmdline| cmdline.body()),
        Some(expected.as_str())
    );
    assert!(app.completion_menu.is_empty());
}

#[tokio::test]
async fn keyboard_resize_changes_a_floating_window_and_leaves_docked_windows_alone() {
    use teal_workbench_core::{Key, KeyModifiers};

    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let mut app = isolated_app(options(directory.path().to_path_buf()), state.path())
        .await
        .expect("Application launch");
    // 主窗口是停靠的：调整请求被拒绝且不改变布局。
    let primary = app.windows.primary();
    app.resize_active_window(teal_workbench_core::Direction::Right, true);
    assert!(
        app.windows
            .get(primary)
            .is_some_and(teal_workbench_core::Window::is_docked)
    );

    // 打开浮动窗口后，宽度与高度按固定步长增长。
    app.open_float_window(WindowKind::Editor);
    let floating = app.windows.active_id();
    let before = app
        .windows
        .get(floating)
        .and_then(|window| window.placement().rect())
        .expect("floating window rect");
    let prefix = |key| InputEvent {
        key,
        modifiers: KeyModifiers {
            control: true,
            ..KeyModifiers::default()
        },
    };
    app.handle_input(prefix(Key::Char('w')));
    app.handle_input(InputEvent {
        key: Key::Char('l'),
        modifiers: KeyModifiers::default(),
    });
    app.handle_input(prefix(Key::Char('w')));
    app.handle_input(InputEvent {
        key: Key::Char('j'),
        modifiers: KeyModifiers::default(),
    });
    let after = app
        .windows
        .get(floating)
        .and_then(|window| window.placement().rect())
        .expect("floating window rect");
    assert!(after.width > before.width, "Ctrl-w l must widen");
    assert!(after.height > before.height, "Ctrl-w j must grow taller");
    assert_eq!(after.x, before.x, "right/bottom anchors keep the origin");
    assert_eq!(after.y, before.y);
}

#[test]
fn window_set_orders_and_cycles_windows() {
    let mut set = WindowSet::new(Window::new(
        WindowKind::Editor,
        "one",
        LayoutTree::new(PanelId::default()),
    ));
    let primary = set.primary();
    let second = set.insert(Window::new(
        WindowKind::Debug,
        "two",
        LayoutTree::new(PanelId::default()),
    ));
    assert_eq!(set.order(), [primary, second]);
    assert_eq!(set.active_id(), second);
    assert!(set.select_next());
    assert_eq!(set.active_id(), primary);
    assert!(set.select_previous());
    assert_eq!(set.active_id(), second);
    assert_eq!(
        set.close(primary),
        Err(teal_workbench_core::WindowCloseError::Primary)
    );
    assert!(set.close(second).is_ok());
    assert_eq!(set.len(), 1);
}

#[tokio::test]
async fn agent_window_uses_the_session_and_prompt_template() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let mut app = isolated_app(options(directory.path().to_path_buf()), state.path())
        .await
        .expect("Application launch");
    app.open_window(WindowKind::Agent);
    let window = app.windows.active();
    let panels = window.validate().expect("the template must be valid");
    assert_eq!(
        panels.len(),
        2,
        "the Agent window has a session and a prompt"
    );
    // `validate()` 不保证顺序，因此按几何从上到下取面板。
    let area = teal_workbench_core::Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 40,
    };
    let mut ordered = app.windows.rectangles(area);
    ordered.sort_by_key(|(_, rect)| rect.y);
    let (session, prompt) = (ordered[0].0, ordered[1].0);
    assert_eq!(
        app.panels.get(session).map(Widget::kind),
        Some(PanelKind::Agent)
    );
    let composer = app
        .panels
        .get(prompt)
        .and_then(|panel| panel.as_composer())
        .expect("the prompt panel hosts a composer");
    assert_eq!(composer.mode(), ComposerMode::Agent);
    assert!(
        !composer.chips().is_empty(),
        "the composer must describe its agent context"
    );
    assert_eq!(window.layout().focused(), prompt);
    assert_eq!(
        app.windows.panel_size(session),
        Some(PanelSize::fill(1).with_min(6))
    );
    assert_eq!(
        app.windows.panel_size(prompt),
        Some(PanelSize::cells(3).with_min(3).with_max(10))
    );

    // 输入换行后槽位随内容增长，且不越过上下限。
    app.handle_input(InputEvent {
        key: Key::Char('h'),
        modifiers: KeyModifiers::default(),
    });
    app.handle_input(InputEvent {
        key: Key::Enter,
        modifiers: KeyModifiers {
            shift: true,
            ..KeyModifiers::default()
        },
    });
    app.handle_input(InputEvent {
        key: Key::Char('i'),
        modifiers: KeyModifiers::default(),
    });
    assert_eq!(
        app.windows.panel_size(prompt),
        Some(PanelSize::cells(4).with_min(3).with_max(10))
    );
}

#[tokio::test]
async fn debug_window_splits_source_inspect_and_console() {
    let directory = tempdir().expect("temporary directory");
    let state = tempdir().expect("isolated Bench state");
    let mut app = isolated_app(options(directory.path().to_path_buf()), state.path())
        .await
        .expect("Application launch");
    app.open_window(WindowKind::Debug);
    let panels = app
        .windows
        .active()
        .validate()
        .expect("the template must be valid");
    assert_eq!(panels.len(), 4);
    let area = teal_workbench_core::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 40,
    };
    let rectangles = app.windows.rectangles(area);
    let console = rectangles
        .iter()
        .find(|(panel, _)| {
            app.panels
                .get(*panel)
                .and_then(|panel| panel.as_debug_console())
                .is_some_and(|console| console.composer().mode() == ComposerMode::Command)
        })
        .map(|(panel, rect)| (*panel, *rect))
        .expect("the console panel hosts a command composer");
    // 控制台在所有工作区面板下方。
    assert!(
        rectangles
            .iter()
            .filter(|(panel, _)| *panel != console.0)
            .all(|(_, rect)| rect.y < console.1.y)
    );
    // Source 是左侧只读输出面板；Variables 与 CallStack 是结构化消息面板。
    let source = rectangles
        .iter()
        .filter(|(panel, _)| *panel != console.0)
        .min_by_key(|(_, rect)| (rect.x, rect.y))
        .map(|(panel, _)| *panel)
        .expect("a source panel");
    assert_eq!(
        app.panels.get(source).map(Widget::kind),
        Some(PanelKind::Output)
    );
    assert_eq!(
        app.panels.get(source).map(Widget::title),
        Some("Source".to_owned())
    );
    let messages = panels
        .iter()
        .filter(|panel| {
            app.panels
                .get(**panel)
                .is_some_and(|panel| panel.kind() == PanelKind::Messages)
        })
        .count();
    assert_eq!(messages, 2, "Variables and CallStack");
}
