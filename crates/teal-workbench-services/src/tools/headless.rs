//! 不依赖 Bench 界面的工作区工具宿主。

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use teal_workbench_core::{
    BufferStore, ChangeSetFile, ChangeSetSelection, WorkspaceIntent, WorkspaceMutationCoordinator,
};
use teal_workspace::Workspace;
use tokio::sync::mpsc;

use super::{ChangeSetRequest, MutationKind, MutationRequest, WorkspaceToolRuntime};

/// CLI 与嵌入宿主复用的工作区工具目录和版本化文件提交器。
///
/// 交互批准由调用方的 PermissionProvider 负责；此类型只核对文件版本和执行事务。
pub struct HeadlessWorkspaceTools {
    runtime: Arc<WorkspaceToolRuntime>,
    _mutation_worker: tokio::task::JoinHandle<()>,
    _changeset_worker: tokio::task::JoinHandle<()>,
}

impl HeadlessWorkspaceTools {
    /// 打开一个独立工作区，并安装受管工具所需的后台事务接收器。
    ///
    /// # Errors
    /// 工作区、版本日志、检查点或工具目录无法创建时返回错误。
    pub fn open(root: &Path) -> Result<Self, String> {
        let (mutations_tx, mut mutations_rx) = mpsc::channel::<MutationRequest>(32);
        let (changesets_tx, mut changesets_rx) = mpsc::channel::<ChangeSetRequest>(16);
        let root = root.canonicalize().map_err(|error| error.to_string())?;
        let checkpoints = root.join(".teal/runtime/headless-checkpoints");
        let runtime = Arc::new(
            WorkspaceToolRuntime::new(
                &root,
                checkpoints.clone(),
                mutations_tx,
                changesets_tx,
                teal_lsp_client::DiagnosticStore::default(),
            )
            .map_err(|error| error.to_string())?,
        );
        let workspace = Arc::new(Workspace::open(&root).map_err(|error| error.to_string())?);
        let coordinator = Arc::new(WorkspaceMutationCoordinator::new(root, checkpoints)?);
        let changeset_workspace = Arc::clone(&workspace);
        let changeset_worker = tokio::task::spawn_blocking(move || {
            while let Some(request) = changesets_rx.blocking_recv() {
                let result = apply_changeset(&changeset_workspace, request.change_set);
                let _ = request.response.send(result);
            }
        });
        let mutation_worker = tokio::task::spawn_blocking(move || {
            while let Some(request) = mutations_rx.blocking_recv() {
                let result = apply_mutation(&workspace, &coordinator, &request);
                let _ = request.response.send(result);
            }
        });
        Ok(Self {
            runtime,
            _mutation_worker: mutation_worker,
            _changeset_worker: changeset_worker,
        })
    }

    /// 返回与 Bench 使用相同实现和签名的可恢复模型工具目录。
    #[must_use]
    pub fn runtime(&self) -> Arc<WorkspaceToolRuntime> {
        Arc::clone(&self.runtime)
    }
}

fn apply_mutation(
    workspace: &Workspace,
    coordinator: &WorkspaceMutationCoordinator,
    request: &MutationRequest,
) -> Result<(), String> {
    if request.expected_buffer_version.is_some() {
        return Err("headless workspace has no open buffer version".into());
    }
    let buffers = BufferStore::default();
    match &request.kind {
        MutationKind::WriteNew(text) => {
            coordinator.create_new_text(&buffers, &request.path, text, true)
        }
        MutationKind::CreateDirectory => coordinator.execute(
            &buffers,
            WorkspaceIntent::Create {
                path: request.path.clone(),
                directory: true,
            },
            true,
        ),
        MutationKind::Move { destination } => coordinator.execute(
            &buffers,
            WorkspaceIntent::Move {
                paths: vec![request.path.clone()],
                destination: destination.clone(),
            },
            true,
        ),
        MutationKind::Delete => coordinator.execute(
            &buffers,
            WorkspaceIntent::Remove {
                paths: vec![request.path.clone()],
                permanent: false,
            },
            true,
        ),
        kind => {
            let saved = match workspace.read(&request.path) {
                Ok(snapshot) => Some(snapshot),
                Err(teal_workspace::WorkspaceError::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    None
                }
                Err(error) => return Err(error.to_string()),
            };
            let current = saved.as_ref().map_or("", |snapshot| snapshot.text.as_str());
            let digest = crate::digest(current.as_bytes());
            if request
                .expected_content_sha256
                .as_ref()
                .is_some_and(|value| value != &digest)
            {
                return Err("workspace content changed before the requested mutation".into());
            }
            let next = match kind {
                MutationKind::Write(text) => text.clone(),
                MutationKind::Replace { old, new } => {
                    if old.is_empty() || current.matches(old).count() != 1 {
                        return Err("replacement text must occur exactly once".into());
                    }
                    current.replacen(old, new, 1)
                }
                MutationKind::ApplyPatch {
                    expected_sha256,
                    patch,
                } => {
                    if !expected_sha256.is_empty() && expected_sha256 != &digest {
                        return Err("patch source digest differs from the current file".into());
                    }
                    let relative = request
                        .path
                        .strip_prefix(workspace.root())
                        .map_err(|_| "patch path is outside the workspace")?;
                    let file = ChangeSetFile::new(
                        relative.to_path_buf(),
                        digest.clone(),
                        digest,
                        patch.clone(),
                    )
                    .map_err(|error| error.to_string())?;
                    file.apply_all(current).map_err(|error| error.to_string())?
                }
                _ => unreachable!("resource operations were handled above"),
            };
            if let Some(saved) = saved {
                workspace
                    .replace(&saved, &next)
                    .map_err(|error| error.to_string())?;
                Ok(())
            } else if matches!(kind, MutationKind::Write(_)) {
                coordinator.write(&buffers, &request.path, &next, true)
            } else {
                Err("mutation target does not exist".into())
            }
        }
    }
}

fn apply_changeset(
    workspace: &Workspace,
    change_set: teal_workbench_core::ChangeSet,
) -> Result<String, String> {
    let mut originals = BTreeMap::<PathBuf, String>::new();
    let mut snapshots = BTreeMap::new();
    for file in &change_set.files {
        let snapshot = workspace
            .read(&file.path)
            .map_err(|error| error.to_string())?;
        if crate::digest(snapshot.text.as_bytes()) != file.original_sha256 {
            return Err("ChangeSet source changed before commit".into());
        }
        originals.insert(file.path.clone(), snapshot.text.clone());
        snapshots.insert(file.path.clone(), snapshot);
    }
    let next = change_set
        .apply_selected(&originals, &ChangeSetSelection::All)
        .map_err(|error| error.to_string())?;
    let mut applied = Vec::new();
    for (path, text) in next {
        let snapshot = &snapshots[&path];
        match workspace.replace(snapshot, &text) {
            Ok(change) => applied.push(change.id),
            Err(error) => {
                let rollback = applied
                    .iter()
                    .rev()
                    .filter_map(|id| workspace.undo(id).err())
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>();
                if !rollback.is_empty() {
                    return Err(format!(
                        "ChangeSet failed: {error}; rollback effects may be unknown: {}",
                        rollback.join("; ")
                    ));
                }
                return Err(error.to_string());
            }
        }
    }
    Ok(format!("Applied {} verified files", applied.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AuditSink;

    impl teal_security::AuditSink for AuditSink {
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

    #[test]
    fn headless_creation_uses_the_no_overwrite_transaction() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("nested/note.md");
        let workspace = Workspace::open(root.path()).unwrap();
        let coordinator = WorkspaceMutationCoordinator::new(
            root.path().to_owned(),
            root.path().join("checkpoints"),
        )
        .unwrap()
        .with_audit_context(teal_security::AuditContext::new(Arc::new(AuditSink)));
        let (response, _) = tokio::sync::oneshot::channel();
        let request = MutationRequest {
            observation: crate::tracing_context::Context::current(),
            path: path.clone(),
            kind: MutationKind::WriteNew("first".into()),
            expected_buffer_version: None,
            expected_content_sha256: None,
            response,
        };
        apply_mutation(&workspace, &coordinator, &request).unwrap();
        assert!(apply_mutation(&workspace, &coordinator, &request).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "first");
    }

    #[test]
    fn headless_mutation_rejects_a_stale_source_before_writing() {
        let _ = teal_security::install_process_sink(Arc::new(AuditSink));
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.tl");
        std::fs::write(&path, "before\n").unwrap();
        let workspace = Workspace::open(root.path()).unwrap();
        let coordinator = WorkspaceMutationCoordinator::new(
            root.path().to_path_buf(),
            root.path().join("checkpoints"),
        )
        .unwrap();
        let (response, _) = tokio::sync::oneshot::channel();
        let request = MutationRequest {
            observation: crate::tracing_context::Context::current(),
            path: path.clone(),
            kind: MutationKind::Write("after\n".into()),
            expected_buffer_version: None,
            expected_content_sha256: Some(crate::digest(b"older\n")),
            response,
        };
        assert!(
            apply_mutation(&workspace, &coordinator, &request)
                .unwrap_err()
                .contains("changed")
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "before\n");
    }

    #[tokio::test]
    async fn headless_catalog_exposes_persistent_toolchain_without_bench() {
        let _ = teal_security::install_process_sink(Arc::new(AuditSink));
        let root = tempfile::tempdir().unwrap();
        let headless = HeadlessWorkspaceTools::open(root.path()).unwrap();
        let names = headless
            .runtime()
            .persistent_tool_registry()
            .unwrap()
            .definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(names.contains("teal.check"));
        assert!(names.contains("teal.fmt_check"));
        assert!(names.contains("teal.test"));
        assert!(names.contains("teal.run"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn source_workspace_transaction_detects_conflict_and_commits_without_bench() {
        let _ = teal_security::install_process_sink(Arc::new(AuditSink));
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("note.txt");
        std::fs::write(&path, "before").unwrap();
        let headless = HeadlessWorkspaceTools::open(root.path()).unwrap();
        let store = Arc::new(teal_agent::RuntimeStore::open(root.path()).unwrap());
        let grant = headless.runtime().agent_grant();
        let services = teal_agent::RuntimeServices::new()
            .with_workspace_transactions(headless.runtime())
            .with_grant(Arc::clone(&grant))
            .with_permissions(Arc::new(teal_agent::AllowAllPermissions))
            .with_audit_context(teal_security::process_context());
        let make_runtime = |source: &str| {
            let compiled = teal::compile_source(
                teal::Source::new("workspace-transaction.tl", source),
                &teal::FsSourceLoader,
                teal::CompileOptions::default().with_tier(teal::RuntimeTier::Agentic),
            )
            .unwrap();
            let program =
                teal_agent::AgentProgram::from_shared_bytes(compiled.serialized_bytes().unwrap())
                    .unwrap();
            let run = store.create_run(&program).unwrap();
            let runtime = teal_agent::AgentRuntime::new(
                program,
                teal_model::TomlAuthManager::open(
                    root.path(),
                    teal_model::AuthProviderRegistry::new(),
                )
                .unwrap(),
                teal_model::ModelFactoryRegistry::new(),
            )
            .unwrap()
            .with_store(Arc::clone(&store), run.id.clone())
            .unwrap()
            .with_services(services.clone())
            .unwrap();
            (runtime, run)
        };
        let mut host = teal::NativeHostRuntime::builder()
            .audit_context(teal_security::process_context())
            .authorization(grant.clone())
            .console(false)
            .file_system(true)
            .build()
            .unwrap();
        let (runtime, run) = make_runtime(
            r#"
            import "std/agent.tl" as app;
            task application() -> none | Error {
                let before = app::workspace_snapshot("note.txt").await?;
                app::receive().await;
                app::workspace_replace(before, "after").await?;
            }

            task main() -> int {
                let result = application().await;
                match result { Error { .. } => { return 1; } _ => { return 0; } }
            }
"#,
        );
        let request = match runtime.run(&mut host).await {
            Err(teal_agent::AgentRuntimeError::NeedsInteraction(request)) => request,
            other => panic!("expected a persistent input wait, got {other:?}"),
        };
        std::fs::write(&path, "external").unwrap();
        store
            .answer_interaction(
                &run.id,
                &request.id,
                request.context.generation,
                teal_agent::UserAnswer::AgentInput {
                    fields: std::collections::BTreeMap::from([
                        ("kind".into(), "text".into()),
                        ("value".into(), "continue".into()),
                    ]),
                },
            )
            .unwrap();
        let outcome = runtime.resume(&mut host).await;
        assert_eq!(outcome.as_ref().unwrap().exit_code, 1);
        runtime.finish_persistent_execution(&outcome).unwrap();
        assert_eq!(
            store.run(&run.id).unwrap().unwrap().state,
            teal_agent::RunState::CompletedWithErrors
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
        drop(runtime);

        let (runtime, _) = make_runtime(
            r#"
            import "std/agent.tl" as app;
            task application() -> none | Error {
                let before = app::workspace_snapshot("note.txt").await?;
                let receipt = app::workspace_replace(before, "after").await?;
                app::emit("text", receipt.after_sha256);
            }

            task main() -> int {
                let result = application().await;
                match result { Error { .. } => { return 1; } _ => { return 0; } }
            }
"#,
        );
        runtime.run(&mut host).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after");
    }
}
