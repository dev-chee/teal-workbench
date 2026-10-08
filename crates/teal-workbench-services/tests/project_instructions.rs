#![cfg(feature = "tools")]

use std::{path::Path, sync::Arc};

use teal_agent::{ActionContext, AllowAllPermissions, DenyAllPermissions, InstructionsProvider as _};
use teal_workbench_services::ProjectInstructions;

fn context() -> ActionContext {
    ActionContext {
        run_id: "run".into(),
        task_id: "task".into(),
        generation: 1,
        policy_version: "policy/v1".into(),
    }
}

#[tokio::test]
async fn nested_project_instructions_are_ordered_and_refresh_after_a_file_change() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("workspace");
    let nested = root.join("src/module");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(root.join("AGENTS.md"), "Root instructions").unwrap();
    std::fs::write(root.join("src/AGENTS.md"), "Source instructions").unwrap();
    let target = nested.join("main.tl");
    std::fs::write(&target, "fn main() {}").unwrap();
    let provider = ProjectInstructions::new(&root, true, Arc::new(AllowAllPermissions)).unwrap();

    let first = provider
        .snapshot(context(), vec![target.to_string_lossy().into_owned()])
        .await
        .unwrap();
    assert_eq!(first.fragments.len(), 2);
    assert_eq!(first.fragments[0].text, "Root instructions");
    assert_eq!(first.fragments[1].text, "Source instructions");
    assert_eq!(
        Path::new(&first.fragments[0].scope).canonicalize().unwrap(),
        root.canonicalize().unwrap()
    );
    assert_eq!(
        Path::new(&first.fragments[1].scope).canonicalize().unwrap(),
        root.join("src").canonicalize().unwrap()
    );
    first.validate().unwrap();

    std::fs::write(root.join("src/AGENTS.md"), "Changed source instructions").unwrap();
    let changed = provider
        .snapshot(context(), vec![target.to_string_lossy().into_owned()])
        .await
        .unwrap();
    assert_ne!(changed.digest, first.digest);
    assert_eq!(changed.fragments[1].text, "Changed source instructions");
}

#[tokio::test]
async fn untrusted_or_denied_project_instructions_expose_no_file_content() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("AGENTS.md"), "private-directive").unwrap();

    let untrusted = ProjectInstructions::new(&root, false, Arc::new(AllowAllPermissions)).unwrap();
    let snapshot = untrusted.snapshot(context(), Vec::new()).await.unwrap();
    assert!(snapshot.fragments.is_empty());

    let denied = ProjectInstructions::new(&root, true, Arc::new(DenyAllPermissions)).unwrap();
    let error = denied.snapshot(context(), Vec::new()).await.unwrap_err();
    assert!(!error.to_string().contains("private-directive"));
}

#[tokio::test]
async fn project_instruction_targets_and_symlinks_cannot_escape_the_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("workspace");
    let nested = root.join("nested");
    let outside = directory.path().join("outside");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("AGENTS.md"), "External instructions").unwrap();
    if let Err(error) = create_file_symlink(&outside.join("AGENTS.md"), &nested.join("AGENTS.md")) {
        if error.raw_os_error() == Some(1314) {
            return;
        }
        panic!("create instruction symlink: {error}");
    }
    let provider = ProjectInstructions::new(&root, true, Arc::new(AllowAllPermissions)).unwrap();

    assert!(
        provider
            .snapshot(context(), vec![outside.to_string_lossy().into_owned()])
            .await
            .is_err()
    );
    let error = provider
        .snapshot(context(), vec![nested.to_string_lossy().into_owned()])
        .await
        .unwrap_err();
    assert!(error.to_string().contains("linked"));
}

#[cfg(unix)]
fn create_file_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_file_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}
