//! Teal Agent 的界面入口、源码输入与工作区检查点回滚。

use similar::TextDiff;
use std::{
    io::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
};

use teal_workbench_services::{
    delivery_paths, execute_agent_commit, preflight_agent_commit, preflight_verified_agent_commit,
};

use super::{
    App, AppPanel, ConfirmOverlay, EditorPanel, MenuItem, MutationKind, MutationRequest, PanelKind,
    PendingConfirmation, PendingOverlay, PickerOverlay, RuntimeEvent, Severity, SplitAxis,
    TextPanel, mutation,
};
use crate::presentation::AgentBlock;

#[cfg(test)]
mod commit_tests {
    use super::*;
    use std::process::Command;

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

    fn ensure_process_audit() {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = teal_security::install_process_sink(Arc::new(TestAuditSink));
        });
    }

    fn git(root: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    }

    fn delivery(root: &Path, path: &str) -> teal_workbench_services::DeliveryReceipt {
        let bytes = std::fs::read(root.join(path)).unwrap();
        teal_workbench_services::DeliveryReceipt {
            summary: "verified delivery".into(),
            script: teal_workbench_services::DeliveryArtifact {
                path: path.into(),
                bytes: bytes.len() as u64,
                sha256: teal_workbench_services::digest(&bytes),
            },
            artifacts: Vec::new(),
            program_digest: teal_workbench_services::compile_digest_in_workspace(
                root,
                &root.join(path),
                "agentic",
            )
            .unwrap(),
            tier: "agentic".into(),
            evidence: Vec::new(),
            accepted_at_ms: 1,
        }
    }

    #[tokio::test]
    async fn commit_preflight_rejects_nonempty_index_and_path_mismatch() {
        ensure_process_audit();
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        git(dir.path(), &["config", "core.autocrlf", "false"]);
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-qm", "base"]);
        std::fs::write(dir.path().join("a.txt"), "changed").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        assert!(
            preflight_agent_commit(dir.path(), &["a.txt".into()])
                .await
                .unwrap_err()
                .contains("index is not empty")
        );
        git(dir.path(), &["reset", "-q"]);
        assert!(
            preflight_agent_commit(dir.path(), &["missing.txt".into()])
                .await
                .unwrap_err()
                .contains("worktree paths do not match")
        );
    }

    #[tokio::test]
    async fn commit_preflight_ignores_dynamic_bench_state_but_never_delivers_it() {
        ensure_process_audit();
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        std::fs::write(dir.path().join("delivery.tl"), "fn main() {}").unwrap();
        let evidence = dir.path().join(".teal/bench/agent-evidence/task-1.json");
        std::fs::create_dir_all(evidence.parent().unwrap()).unwrap();
        std::fs::write(&evidence, "{}").unwrap();

        preflight_agent_commit(dir.path(), &["delivery.tl".into()])
            .await
            .expect("dynamic Bench state must not block a verified delivery");

        assert!(
            preflight_agent_commit(
                dir.path(),
                &[
                    "delivery.tl".into(),
                    ".teal/bench/agent-evidence/task-1.json".into(),
                ],
            )
            .await
            .unwrap_err()
            .contains("worktree paths do not match")
        );
    }

    #[tokio::test]
    async fn execute_agent_commit_uses_explicit_paths_and_only_commit() {
        ensure_process_audit();
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        git(dir.path(), &["config", "core.autocrlf", "false"]);
        std::fs::write(dir.path().join("main.tl"), "fn main() {}\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "b").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-qm", "base"]);
        std::fs::write(
            dir.path().join("main.tl"),
            "fn main() { print(\"changed\"); }\n",
        )
        .unwrap();
        let delivery = delivery(dir.path(), "main.tl");
        let commit = execute_agent_commit(dir.path(), "only main", &delivery)
            .await
            .unwrap();
        assert!(!commit.is_empty());
        let changed = Command::new("git")
            .args(["show", "--format=", "--name-only", &commit])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let names = String::from_utf8_lossy(&changed.stdout);
        assert!(names.lines().any(|line| line == "main.tl"));
        assert!(!names.lines().any(|line| line == "b.txt"));
        let cached = Command::new("git")
            .args(["diff", "--cached", "--name-only"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(cached.status.success());
        assert!(cached.stdout.is_empty(), "Task paths must not stay staged");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "b"
        );
    }
}

#[path = "agent/editing.rs"]
mod editing;
#[path = "agent/preparation.rs"]
mod preparation;
#[path = "agent/queries.rs"]
mod queries;
