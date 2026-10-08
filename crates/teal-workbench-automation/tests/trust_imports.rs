//! 用户配置入口的完整导入闭包信任回归。

use std::{fs, path::Path};

use teal_workbench_automation::{ScriptKind, TrustDecision, TrustStore, WorkbenchAutomationRuntime};

fn runtime(root: &Path) -> WorkbenchAutomationRuntime {
    WorkbenchAutomationRuntime::new(root.join("automation-state.json")).expect("automation runtime")
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

#[test]
fn user_config_and_imports_within_its_directory_are_trusted() {
    let fixture = tempfile::tempdir().unwrap();
    let user = fixture.path().join("user-config");
    let entry = user.join("config.tl");
    write(
        &user.join("helper.tl"),
        "pub fn value() -> int { return 1; }",
    );
    write(
        &entry,
        "import \"helper.tl\" as helper; fn main() { helper::value(); }",
    );
    let runtime = runtime(fixture.path());
    let trust = TrustStore::load(fixture.path().join("trust.json")).unwrap();
    let prepared = runtime.prepare(&entry, ScriptKind::UserConfig).unwrap();

    assert_eq!(
        runtime.trust_decision(&prepared, &trust),
        TrustDecision::Trusted
    );
}

#[test]
fn user_config_external_import_requires_snapshot_approval_and_reapproval_on_change() {
    let fixture = tempfile::tempdir().unwrap();
    let user = fixture.path().join("user-config");
    let outside = fixture.path().join("outside");
    let entry = user.join("config.tl");
    let helper = outside.join("helper.tl");
    write(&helper, "pub fn value() -> int { return 1; }");
    write(
        &entry,
        "import \"../outside/helper.tl\" as helper; fn main() { helper::value(); }",
    );
    let runtime = runtime(fixture.path());
    let mut trust = TrustStore::load(fixture.path().join("trust.json")).unwrap();
    let first = runtime.prepare(&entry, ScriptKind::UserConfig).unwrap();

    match runtime.trust_decision(&first, &trust) {
        TrustDecision::ApprovalRequired { path, capabilities } => {
            assert_eq!(path, entry.canonicalize().unwrap());
            assert_eq!(capabilities, *first.capabilities());
        }
        TrustDecision::Trusted => panic!("an external user import must require approval"),
    }
    trust
        .approve(
            first.path(),
            first.trust_contents(),
            first.capabilities().clone(),
        )
        .unwrap();
    assert_eq!(
        runtime.trust_decision(&first, &trust),
        TrustDecision::Trusted
    );

    write(&helper, "pub fn value() -> int { return 2; }");
    let changed = runtime.prepare(&entry, ScriptKind::UserConfig).unwrap();
    assert_ne!(first.trust_contents(), changed.trust_contents());
    assert!(matches!(
        runtime.trust_decision(&changed, &trust),
        TrustDecision::ApprovalRequired { .. }
    ));
}

#[test]
fn user_config_external_import_capability_change_requires_reapproval() {
    let fixture = tempfile::tempdir().unwrap();
    let user = fixture.path().join("user-config");
    let outside = fixture.path().join("outside");
    let entry = user.join("config.tl");
    write(
        &outside.join("helper.tl"),
        "pub fn value() -> int { return 1; }",
    );
    write(
        &entry,
        "import \"../outside/helper.tl\" as helper; fn main() { helper::value(); }",
    );
    let runtime = runtime(fixture.path());
    let mut trust = TrustStore::load(fixture.path().join("trust.json")).unwrap();
    let first = runtime.prepare(&entry, ScriptKind::UserConfig).unwrap();
    trust
        .approve(
            first.path(),
            first.trust_contents(),
            first.capabilities().clone(),
        )
        .unwrap();

    write(
        &entry,
        r#"import "../outside/helper.tl" as helper;
host "std.bench.config" as config { fn state_set(arg0: string, arg1: string); }
fn main() { helper::value(); config::state_set("phase", "changed"); }"#,
    );
    let changed = runtime.prepare(&entry, ScriptKind::UserConfig).unwrap();
    assert_ne!(first.capabilities(), changed.capabilities());
    assert!(matches!(
        runtime.trust_decision(&changed, &trust),
        TrustDecision::ApprovalRequired { .. }
    ));
}

#[test]
fn builtin_grammar_remains_trusted_with_an_external_user_import() {
    let fixture = tempfile::tempdir().unwrap();
    let builtin = fixture.path().join("builtin");
    let outside = fixture.path().join("outside");
    let entry = builtin.join("grammar.tl");
    write(
        &outside.join("helper.tl"),
        "pub fn value() -> int { return 1; }",
    );
    write(
        &entry,
        "import \"../outside/helper.tl\" as helper; fn main() { helper::value(); }",
    );
    let runtime = runtime(fixture.path());
    let trust = TrustStore::load(fixture.path().join("trust.json")).unwrap();
    let prepared = runtime.prepare(&entry, ScriptKind::BuiltinGrammar).unwrap();

    assert_eq!(
        runtime.trust_decision(&prepared, &trust),
        TrustDecision::Trusted
    );
}

#[test]
fn user_config_symlink_import_outside_its_directory_requires_approval() {
    let fixture = tempfile::tempdir().unwrap();
    let user = fixture.path().join("user-config");
    let outside = fixture.path().join("outside");
    let entry = user.join("config.tl");
    let helper = outside.join("helper.tl");
    write(&helper, "pub fn value() -> int { return 1; }");
    fs::create_dir_all(&user).unwrap();
    let link = user.join("linked-helper.tl");
    if let Err(error) = file_link(&helper, &link) {
        #[cfg(windows)]
        {
            eprintln!("skipping file-link regression without Windows symlink permission: {error}");
            return;
        }
        #[cfg(unix)]
        panic!("file link should be creatable: {error}");
    }
    write(
        &entry,
        "import \"linked-helper.tl\" as helper; fn main() { helper::value(); }",
    );
    let runtime = runtime(fixture.path());
    let trust = TrustStore::load(fixture.path().join("trust.json")).unwrap();
    let prepared = runtime.prepare(&entry, ScriptKind::UserConfig).unwrap();

    assert!(matches!(
        runtime.trust_decision(&prepared, &trust),
        TrustDecision::ApprovalRequired { .. }
    ));
}

#[cfg(unix)]
fn file_link(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn file_link(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}
