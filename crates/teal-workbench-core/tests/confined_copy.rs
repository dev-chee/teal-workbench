use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

use teal_workbench_core::{BufferStore, WorkspaceIntent, WorkspaceMutationCoordinator};

#[derive(Default)]
struct TestAuditSink(Mutex<Vec<teal_security::Event>>);

impl teal_security::AuditSink for TestAuditSink {
    fn append(
        &self,
        event: &teal_security::Event,
    ) -> Result<teal_security::Receipt, teal_security::AuditError> {
        let mut events = self.0.lock().unwrap();
        events.push(event.clone());
        Ok(teal_security::Receipt {
            sequence: events.len() as u64,
            hash: format!("{:064x}", events.len()),
        })
    }
}

fn coordinator(workspace: &Path) -> WorkspaceMutationCoordinator {
    WorkspaceMutationCoordinator::new(workspace.to_owned(), workspace.join(".teal/checkpoints"))
        .unwrap()
        .with_audit_context(teal_security::AuditContext::new(Arc::new(
            TestAuditSink::default(),
        )))
}

#[test]
fn missing_audit_sink_rejects_copy_before_creating_the_destination() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(workspace.join("source")).unwrap();
    fs::write(workspace.join("source/value.txt"), "inside").unwrap();
    let coordinator =
        WorkspaceMutationCoordinator::new(workspace.clone(), workspace.join(".teal/checkpoints"))
            .unwrap();

    let error = coordinator
        .execute(
            &BufferStore::default(),
            WorkspaceIntent::Copy {
                paths: vec![Path::new("source").to_owned()],
                destination: Path::new("destination").to_owned(),
            },
            true,
        )
        .unwrap_err();
    assert!(error.contains("audit sink is required"));
    assert!(!workspace.join("destination").exists());
}

#[test]
fn audited_coordinator_can_replace_the_same_file_twice_on_windows() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let path = workspace.join("document.txt");
    fs::write(&path, "before").unwrap();
    let coordinator = coordinator(&workspace);
    let buffers = BufferStore::default();

    coordinator
        .write(&buffers, &path, "private content", true)
        .unwrap();
    coordinator
        .write(&buffers, &path, "shared content", true)
        .unwrap();

    assert_eq!(fs::read_to_string(path).unwrap(), "shared content");
}

#[test]
fn confined_copy_preserves_a_legal_tree() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let source = workspace.join("source");
    let destination = workspace.join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::create_dir_all(&destination).unwrap();
    fs::write(source.join("nested/value.txt"), "inside").unwrap();
    directory_link(&source.join("nested"), &source.join("alias")).unwrap();

    coordinator(&workspace)
        .execute(
            &BufferStore::default(),
            WorkspaceIntent::Copy {
                paths: vec![Path::new("source").to_owned()],
                destination: Path::new("destination").to_owned(),
            },
            true,
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("source/nested/value.txt")).unwrap(),
        "inside"
    );
    assert_eq!(
        fs::read_to_string(destination.join("source/alias/value.txt")).unwrap(),
        "inside"
    );
}

#[test]
fn confined_copy_preserves_a_wide_directory() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let source = workspace.join("source");
    let destination = workspace.join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    for index in 0..512 {
        fs::write(
            source.join(format!("value-{index:04}.txt")),
            index.to_string(),
        )
        .unwrap();
    }

    coordinator(&workspace)
        .execute(
            &BufferStore::default(),
            WorkspaceIntent::Copy {
                paths: vec![Path::new("source").to_owned()],
                destination: Path::new("destination").to_owned(),
            },
            true,
        )
        .unwrap();

    let copied = destination.join("source");
    assert_eq!(fs::read_dir(&copied).unwrap().count(), 512);
    assert_eq!(
        fs::read_to_string(copied.join("value-0511.txt")).unwrap(),
        "511"
    );
}

#[test]
fn confined_copy_rejects_an_external_directory_link_before_reading_files() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let source = workspace.join("source");
    let destination = workspace.join("destination");
    let outside = fixture.path().join("outside");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.txt"), "outside-secret").unwrap();
    directory_link(&outside, &source.join("escape")).unwrap();

    let error = coordinator(&workspace)
        .execute(
            &BufferStore::default(),
            WorkspaceIntent::Copy {
                paths: vec![source],
                destination: destination.clone(),
            },
            true,
        )
        .expect_err("a confined traversal must reject an external directory link");
    assert!(error.contains("outside"), "{error}");
    assert!(!destination.join("source/escape/secret.txt").exists());
}

#[test]
fn confined_copy_rejects_a_recursive_directory_link() {
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    let source = workspace.join("source");
    let destination = workspace.join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    directory_link(&source, &source.join("cycle")).unwrap();

    let error = coordinator(&workspace)
        .execute(
            &BufferStore::default(),
            WorkspaceIntent::Copy {
                paths: vec![source],
                destination,
            },
            true,
        )
        .expect_err("a recursive directory link must be rejected");
    assert!(error.contains("Recursive directory link"), "{error}");
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
