use std::{fs, path::Path, sync::Arc};

use teal_workbench_core::{BufferStore, Document, WorkspaceIntent, WorkspaceMutationCoordinator};

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
fn removing_an_internal_directory_link_keeps_target_buffer_bound() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let target = workspace.join("target");
    let link = workspace.join("link");
    let target_file = target.join("value.txt");
    fs::create_dir_all(&target).unwrap();
    fs::write(&target_file, "inside").unwrap();
    directory_link(&target, &link).unwrap();

    let mut buffers = BufferStore::default();
    let buffer = buffers.insert(Document::open(&target_file).unwrap());
    let coordinator = coordinator(&workspace);
    let entry = coordinator
        .resolve_entry(&link, true)
        .expect("the link entry should resolve without following its target");
    assert!(entry.is_link());
    assert!(entry.is_dir());
    entry
        .revalidate()
        .expect("the unchanged link entry should remain valid");
    coordinator
        .execute(
            &buffers,
            WorkspaceIntent::Remove {
                paths: vec![link.clone()],
                permanent: true,
            },
            true,
        )
        .unwrap();

    assert!(!link.exists());
    assert_eq!(fs::read_to_string(&target_file).unwrap(), "inside");
    assert_eq!(buffer.path().as_deref(), Some(target_file.as_path()));
}

#[test]
fn renaming_an_internal_directory_link_preserves_the_alias_buffer_path() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let target = workspace.join("target");
    let old_link = workspace.join("old-link");
    let new_link = workspace.join("new-link");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("value.txt"), "inside").unwrap();
    directory_link(&target, &old_link).unwrap();

    let mut buffers = BufferStore::default();
    let buffer = buffers.insert(Document::open(old_link.join("value.txt")).unwrap());
    coordinator(&workspace)
        .execute(
            &buffers,
            WorkspaceIntent::Rename {
                old: old_link.clone(),
                new: new_link.clone(),
            },
            true,
        )
        .unwrap();

    assert!(!old_link.exists());
    assert_eq!(
        fs::read_to_string(target.join("value.txt")).unwrap(),
        "inside"
    );
    assert_eq!(
        buffer.path().as_deref(),
        Some(new_link.join("value.txt").as_path())
    );
}

#[test]
fn renaming_an_internal_directory_link_keeps_target_buffer_bound() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let target = workspace.join("target");
    let old_link = workspace.join("old-link");
    let new_link = workspace.join("new-link");
    let target_file = target.join("value.txt");
    fs::create_dir_all(&target).unwrap();
    fs::write(&target_file, "inside").unwrap();
    directory_link(&target, &old_link).unwrap();

    let mut buffers = BufferStore::default();
    let buffer = buffers.insert(Document::open(&target_file).unwrap());
    coordinator(&workspace)
        .execute(
            &buffers,
            WorkspaceIntent::Rename {
                old: old_link.clone(),
                new: new_link.clone(),
            },
            true,
        )
        .unwrap();

    assert!(!old_link.exists());
    assert_eq!(fs::read_to_string(&target_file).unwrap(), "inside");
    assert_eq!(buffer.path().as_deref(), Some(target_file.as_path()));
}

#[test]
fn confined_remove_deletes_only_an_external_directory_link_entry() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let outside = fixture.path().join("outside");
    let link = workspace.join("external-link");
    let outside_file = outside.join("value.txt");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(&outside_file, "outside").unwrap();
    directory_link(&outside, &link).unwrap();

    coordinator(&workspace)
        .execute(
            &BufferStore::default(),
            WorkspaceIntent::Remove {
                paths: vec![link.clone()],
                permanent: true,
            },
            true,
        )
        .unwrap();

    assert!(!link.exists());
    assert_eq!(fs::read_to_string(outside_file).unwrap(), "outside");
}

#[test]
fn confined_targets_reject_an_external_link_but_unconfined_read_can_follow_it() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let outside = fixture.path().join("outside");
    let link = workspace.join("external-link");
    let outside_file = outside.join("value.txt");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(&outside_file, "outside").unwrap();
    directory_link(&outside, &link).unwrap();

    let coordinator = coordinator(&workspace);
    let read_error = coordinator
        .resolve_read(Path::new("external-link/value.txt"), true)
        .expect_err("a confined read must reject an external link");
    assert!(read_error.contains("outside"), "{read_error}");
    let write_error = coordinator
        .resolve_write(Path::new("external-link/new.txt"), true)
        .expect_err("a confined write must reject an external link");
    assert!(write_error.contains("outside"), "{write_error}");

    let read = coordinator
        .resolve_read(Path::new("external-link/value.txt"), false)
        .unwrap();
    assert_eq!(read.path(), outside_file);
    read.revalidate().unwrap();
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
