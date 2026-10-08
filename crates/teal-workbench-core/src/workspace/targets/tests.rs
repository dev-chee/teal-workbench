use super::super::WorkspaceMutationCoordinator;
use super::{ResolvedEntry, Root, file_key, identity_file};
use crate::BufferStore;
use std::{fs, path::Path, sync::Arc};

struct TestAuditSink;

impl teal_security::AuditSink for TestAuditSink {
    fn append(
        &self,
        _: &teal_security::Event,
    ) -> Result<teal_security::Receipt, teal_security::AuditError> {
        Ok(teal_security::Receipt {
            sequence: 1,
            hash: "0".repeat(64),
        })
    }
}

fn coordinator(workspace: &Path) -> WorkspaceMutationCoordinator {
    WorkspaceMutationCoordinator::new(workspace.to_owned(), workspace.join(".teal/checkpoints"))
        .unwrap()
        .with_audit_context(teal_security::AuditContext::new(Arc::new(TestAuditSink)))
}

#[test]
fn directory_link_identity_can_be_opened_without_following_the_target() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let target = workspace.join("target");
    let link = workspace.join("link");
    fs::create_dir_all(&target).unwrap();
    directory_link(&target, &link).unwrap();

    let file = identity_file(&link, true).expect("the link entry should open for identity");
    file_key(&file).expect("the link entry should expose a stable file identity");

    let root = Root::new(&workspace).expect("the workspace root should resolve");
    root.verify().expect("the workspace root should revalidate");
    let entry = ResolvedEntry::new(&root, &link, true, true)
        .expect("the link should resolve as a directory entry");
    assert!(entry.is_link());
    assert!(entry.is_dir());
}

#[test]
fn resolved_write_follows_an_internal_link_with_a_missing_suffix() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let target = workspace.join("target");
    let link = workspace.join("link");
    fs::create_dir_all(&target).unwrap();
    directory_link(&target, &link).unwrap();

    let coordinator = coordinator(&workspace);
    let resolved = coordinator
        .resolve_write(Path::new("link/missing/value.txt"), true)
        .unwrap();
    assert_eq!(resolved.path(), target.join("missing/value.txt"));
    assert!(!resolved.exists());

    coordinator
        .write_resolved(&BufferStore::default(), resolved, "created")
        .unwrap();
    assert_eq!(
        fs::read_to_string(target.join("missing/value.txt")).unwrap(),
        "created"
    );
}

#[test]
fn resolved_write_rejects_a_replaced_file() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let path = workspace.join("value.txt");
    let displaced = workspace.join("displaced.txt");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(&path, "original").unwrap();

    let coordinator = coordinator(&workspace);
    let resolved = coordinator.resolve_write(&path, true).unwrap();
    fs::rename(&path, &displaced).unwrap();
    fs::write(&path, "replacement").unwrap();

    let error = coordinator
        .write_resolved(&BufferStore::default(), resolved, "unexpected")
        .expect_err("a resolved target must reject a replacement file");
    assert!(error.contains("changed"), "{error}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "replacement");
    assert_eq!(fs::read_to_string(&displaced).unwrap(), "original");
}

#[test]
fn coordinator_rejects_a_target_resolved_by_another_workspace() {
    let fixture = tempfile::tempdir().unwrap();
    let first_workspace = fixture.path().join("first-workspace");
    let second_workspace = fixture.path().join("second-workspace");
    let path = first_workspace.join("value.txt");
    fs::create_dir_all(&first_workspace).unwrap();
    fs::create_dir_all(&second_workspace).unwrap();
    fs::write(&path, "original").unwrap();

    let resolved = coordinator(&first_workspace)
        .resolve_write(&path, true)
        .unwrap();
    let error = coordinator(&second_workspace)
        .write_resolved(&BufferStore::default(), resolved, "unexpected")
        .expect_err("a coordinator must reject another workspace's proof");

    assert!(error.contains("workspace"), "{error}");
    assert_eq!(fs::read_to_string(path).unwrap(), "original");
}

#[test]
fn resolved_entry_rejects_a_replaced_directory_link() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let first = workspace.join("first");
    let second = workspace.join("second");
    let link = workspace.join("link");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    fs::write(first.join("value.txt"), "first").unwrap();
    fs::write(second.join("value.txt"), "second").unwrap();
    directory_link(&first, &link).unwrap();

    let coordinator = coordinator(&workspace);
    let resolved = coordinator.resolve_entry(&link, true).unwrap();
    remove_directory_link(&link).unwrap();
    directory_link(&second, &link).unwrap();

    let error = coordinator
        .remove_entry(&BufferStore::default(), &resolved, true)
        .expect_err("a resolved entry must reject a replacement link");
    assert!(error.contains("changed"), "{error}");
    assert!(link.exists());
    assert_eq!(
        fs::read_to_string(first.join("value.txt")).unwrap(),
        "first"
    );
    assert_eq!(
        fs::read_to_string(second.join("value.txt")).unwrap(),
        "second"
    );
}

#[test]
fn dangling_internal_link_write_recreates_only_its_target() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let target = workspace.join("target");
    let link = workspace.join("link");
    fs::create_dir_all(&target).unwrap();
    directory_link(&target, &link).unwrap();
    fs::remove_dir(&target).unwrap();

    let coordinator = coordinator(&workspace);
    coordinator
        .write(
            &BufferStore::default(),
            Path::new("link/nested/value.txt"),
            "created",
            true,
        )
        .unwrap();

    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(
        fs::read_to_string(target.join("nested/value.txt")).unwrap(),
        "created"
    );
}

#[cfg(unix)]
fn directory_link(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn directory_link(target: &Path, link: &Path) -> std::io::Result<()> {
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "New-Item -ItemType Junction -Path $env:TEAL_TEST_LINK -Target $env:TEAL_TEST_TARGET | Out-Null",
        ])
        .env("TEAL_TEST_LINK", link)
        .env("TEAL_TEST_TARGET", target)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))
    }
}

#[cfg(unix)]
fn remove_directory_link(link: &Path) -> std::io::Result<()> {
    fs::remove_file(link)
}

#[cfg(windows)]
fn remove_directory_link(link: &Path) -> std::io::Result<()> {
    fs::remove_dir(link)
}
