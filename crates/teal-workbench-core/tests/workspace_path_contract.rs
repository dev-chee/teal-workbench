use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use teal_workbench_core::{BufferStore, WorkspaceMutationCoordinator, ensure_workspace_path};

struct TestAudit(AtomicU64);

impl teal_security::AuditSink for TestAudit {
    fn append(
        &self,
        _: &teal_security::Event,
    ) -> Result<teal_security::Receipt, teal_security::AuditError> {
        let sequence = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(teal_security::Receipt {
            sequence,
            hash: format!("{sequence:064x}"),
        })
    }
}

#[test]
fn shared_transaction_creates_text_without_overwriting_existing_target() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let coordinator =
        WorkspaceMutationCoordinator::new(workspace.clone(), workspace.join(".teal/checkpoints"))
            .unwrap()
            .with_audit_context(teal_security::AuditContext::new(Arc::new(TestAudit(
                AtomicU64::new(0),
            ))));
    let target = workspace.join(".teal/knowledge/agent-note.md");
    let buffers = BufferStore::default();
    coordinator
        .create_new_text(&buffers, &target, "first\n", true)
        .unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"first\n");
    assert!(
        coordinator
            .create_new_text(&buffers, &target, "second\n", true)
            .is_err()
    );
    assert_eq!(fs::read(&target).unwrap(), b"first\n");
}

#[test]
fn accepts_relative_paths_against_the_workspace_root() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    fs::create_dir(&workspace).expect("workspace directory");

    ensure_workspace_path(&workspace, Path::new("nested/output.txt"))
        .expect("relative paths are resolved from the workspace root");
}

#[test]
fn accepts_a_new_path_below_multiple_missing_workspace_ancestors() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    fs::create_dir(&workspace).expect("workspace directory");
    let target = workspace.join("nested").join("deeper").join("output.txt");

    ensure_workspace_path(&workspace, &target)
        .expect("the nearest existing ancestor establishes the confined boundary");
}

#[test]
fn rejects_paths_whose_existing_ancestor_is_outside_the_workspace() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let outside = directory.path().join("outside");
    fs::create_dir(&workspace).expect("workspace directory");
    fs::create_dir(&outside).expect("outside directory");

    let error = ensure_workspace_path(&workspace, &outside.join("nested/output.txt"))
        .expect_err("an outside ancestor must remain rejected");
    assert!(error.contains("outside"));
}
