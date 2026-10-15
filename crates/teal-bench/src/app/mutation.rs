//! 串行文件作业队列及共享缓冲区的保留和归还。
//!
//! 工作线程只操作文档副本，返回后先恢复原句柄和变化队列，再通知调用方完成。
//! 此处的保留覆盖已登记缓冲区；文件打开与退出排空遵循 `teal-bench` README 的恢复边界。

use super::{
    App, AppPanel, Arc, Duration, HashMap, JsonRpcId, LspClient, MutationKind, PanelId, Path,
    PathBuf, RuntimeEvent, Severity, WorkspaceIntent, WorkspaceMutationCoordinator,
    WorkspaceOperation, ensure_workspace_path, mutation_text, oneshot, parse_workspace_edit,
};
use std::collections::VecDeque;
use teal_workbench_core::{
    BufferHandle, BufferId, Document, DocumentChange, TextEdit, Transaction,
};

/// 工作线程可执行的操作描述，不持有只能在界面线程使用的缓冲区句柄。
#[derive(Clone)]
pub(crate) enum Work {
    /// 文件资源操作；布尔值决定是否强制限制在工作区内。
    Resource(WorkspaceIntent, bool),
    /// 智能体的完整替换或精确文本替换。
    Agent(PathBuf, MutationKind, Option<String>),
    /// 带审查时文本摘要的智能体变更。
    AgentChecked(PathBuf, MutationKind, Option<String>, Option<String>),
    /// 已完成用户分级审查的多文件 ChangeSet。
    AgentChangeSet(
        teal_workbench_core::ChangeSet,
        teal_workbench_core::ChangeSetSelection,
    ),
    /// 从指定检查点恢复到目标路径；`None` 表示该操作新建了目标，回滚即删除。
    Restore(Option<PathBuf>, PathBuf),
    /// 保存共享缓冲区，可选路径表示另存为。
    Save(BufferId, Option<PathBuf>),
    /// 稍后针对捕获的版本解析和应用 LSP 工作区编辑。
    Lsp(serde_json::Value),
}

/// 返回写入前后文本中发生变化的行区间（以写入后文本的行号为准，0 基）。
fn changed_line_ranges(before: &str, after: &str) -> Vec<std::ops::Range<usize>> {
    let diff = similar::TextDiff::from_lines(before, after);
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    let mut line = 0_usize;
    let mut push = |line: usize| {
        if let Some(last) = ranges.last_mut() {
            if last.start <= line && line < last.end {
                return;
            }
            if last.end == line {
                last.end = line + 1;
                return;
            }
        }
        ranges.push(line..line + 1);
    };
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Equal => line += 1,
            similar::ChangeTag::Insert => {
                push(line);
                line += 1;
            }
            similar::ChangeTag::Delete => push(line),
        }
    }
    ranges
}
/// 尚未执行的操作及完成通知；队列拥有通知发送端，不能提前回复成功。
struct Job {
    observation: crate::runtime_bridge::Context,
    work: Work,
    panel: Option<PanelId>,
    response: Option<oneshot::Sender<Result<(), String>>>,
    lsp: Option<(Arc<LspClient>, JsonRpcId)>,
    /// LSP 编辑入队时的版本快照，开始执行前必须重新核对。
    versions: Vec<(BufferId, u64)>,
}

/// 留在应用线程的活动作业及原始文档，用于在任意完成路径释放保留状态。
struct Active {
    job: Job,
    originals: Vec<(BufferHandle, Document)>,
}

#[derive(Default)]
pub(crate) struct State {
    queue: VecDeque<Job>,
    active: Option<Active>,
}

impl State {
    pub(super) fn is_full(&self) -> bool {
        self.queue.len() >= 64
    }

    pub(super) fn panel_busy(&self, panel: PanelId) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.job.panel == Some(panel))
            || self.queue.iter().any(|job| job.panel == Some(panel))
    }

    pub(super) fn is_idle(&self) -> bool {
        self.active.is_none() && self.queue.is_empty()
    }
}

/// 工作线程的逻辑结果和实际文档状态；操作失败不代表此前没有产生变化。
pub(crate) struct Completed {
    result: Result<(), String>,
    documents: Vec<(BufferId, Document, Vec<DocumentChange>)>,
}

impl App {
    pub(super) fn queue_work(
        &mut self,
        work: Work,
        panel: Option<PanelId>,
        response: Option<oneshot::Sender<Result<(), String>>>,
        lsp: Option<(Arc<LspClient>, JsonRpcId)>,
    ) {
        if self.io.is_full() {
            if let Some(response) = response {
                let _ = response.send(Err("File system queue is full".into()));
            }
            if let Some((client, id)) = lsp {
                self.tasks.spawn(async move {
                    let _ = client
                        .respond(
                            id,
                            serde_json::json!({"applied":false,"failureReason":"File system queue is full"}),
                        )
                        .await;
                });
            }
            self.message(Severity::Warning, "workspace", "File system queue is full");
            return;
        }

        // LSP 编辑引用进入队列时的缓冲区修订。
        let versions = if matches!(work, Work::Lsp(_)) {
            self.buffers
                .iter()
                .map(|buffer| (buffer.id(), buffer.revision()))
                .collect()
        } else {
            Vec::new()
        };

        for path in self.work_paths(&work) {
            let generation = self.path_generations.entry(concrete(&path)).or_default();
            *generation = generation.saturating_add(1);
        }
        self.io.queue.push_back(Job {
            observation: crate::runtime_bridge::Context::with_span(tracing::info_span!(
                "teal.bench.file.mutate",
                operation.id = teal_workbench_services::observability::operation_id(),
                operation.result = tracing::field::Empty,
                otel.status_code = tracing::field::Empty
            )),
            work,
            panel,
            response,
            lsp,
            versions,
        });
    }

    pub(super) fn pump_mutations(&mut self) {
        if self.io.active.is_some() {
            return;
        }
        let Some(job) = self.io.queue.pop_front() else {
            return;
        };

        if job.versions.iter().any(|(id, revision)| {
            self.buffers
                .get(*id)
                .is_none_or(|buffer| buffer.revision() != *revision)
        }) {
            let error = "Buffer changed during LSP edit queuing".to_owned();
            if let Some(response) = job.response {
                let _ = response.send(Err(error.clone()));
            }
            if let Some((client, id)) = job.lsp {
                self.tasks.spawn(async move {
                    let _ = client
                        .respond(
                            id,
                            serde_json::json!({"applied":false,"failureReason":"Buffer revision expired"}),
                        )
                        .await;
                });
            }
            self.message(Severity::Warning, "lsp", error);
            return;
        }

        // 将文档副本移交工作线程前，先保留当前已登记的缓冲区。
        self.process_buffer_changes();
        let handles = self
            .buffers
            .iter()
            .filter(|b| affected(&job.work, b, &self.workspace))
            .cloned()
            .collect::<Vec<_>>();
        let mut originals = Vec::new();
        for handle in handles {
            match handle.begin_io() {
                Ok(document) => originals.push((handle, document)),
                Err(error) => {
                    self.message(Severity::Error, "workspace", error.to_string());

                    // 后续缓冲区无法保留时，释放先前的保留状态。
                    for (handle, document) in originals {
                        handle.finish_io(document, Vec::new());
                    }

                    if let Some(response) = job.response {
                        let _ = response.send(Err(error.to_string()));
                    }
                    if let Some((client, id)) = job.lsp {
                        self.tasks.spawn(async move {
                            let _ = client
                                .respond(
                                    id,
                                    serde_json::json!({"applied":false,"failureReason":error.to_string()}),
                                )
                                .await;
                        });
                    }
                    return;
                }
            }
        }

        // 保留原句柄和副本作为失败归还路径；只有可跨线程的数据进入工作线程。
        // 归还原副本只恢复内存状态，不能据此声称磁盘操作已经回滚。
        let snapshots = originals
            .iter()
            .map(|(h, d)| (h.id(), d.clone()))
            .collect::<Vec<_>>();
        let work = job.work.clone();
        let coordinator = self.mutations.clone();
        let root = self.workspace.clone();
        let output = self.runtime_tx.clone();
        let observation = job.observation.clone();
        self.io.active = Some(Active { job, originals });

        self.tasks.spawn(observation.instrument(async move {
            let outcome = crate::runtime_bridge::spawn_blocking(move || {
                let mut buffers = teal_workbench_core::BufferStore::default();
                let handles = snapshots
                    .into_iter()
                    .map(|(id, document)| (id, buffers.insert(document)))
                    .collect::<Vec<_>>();
                let event = teal_security::Event::new(
                    teal_security::Action::WorkspaceJob,
                    teal_security::Phase::Intent,
                    teal_security::Outcome::Pending,
                    teal_security::new_correlation_id(),
                )
                .map(|event| {
                    event
                        .with_actor(teal_security::Actor::Host)
                        .with_workspace(&root)
                });
                let result = match event.map_err(|error| error.to_string()).and_then(|event| {
                    teal_security::process_context()
                        .begin(event)
                        .map_err(|error| error.to_string())
                }) {
                    Ok(attempt) => {
                        let result = execute(&coordinator, &buffers, &handles, &root, work);
                        let outcome = if result.is_ok() {
                            teal_security::Outcome::Succeeded
                        } else {
                            teal_security::Outcome::Failed
                        };
                        match attempt.finish(outcome) {
                            Ok(_) => result,
                            Err(error) => Err(format!(
                                "Audit result unavailable; workspace effect may be unknown: {error}"
                            )),
                        }
                    }
                    Err(error) => Err(format!(
                        "Audit unavailable; workspace mutation was not started: {error}"
                    )),
                };
                Completed {
                    result,
                    documents: handles
                        .into_iter()
                        .map(|(id, buffer)| (id, buffer.snapshot_document(), buffer.take_changes()))
                        .collect(),
                }
            })
            .await
            .map_err(|e| e.to_string());
            let _ = output.send(RuntimeEvent::WorkspaceFinished(outcome)).await;
        }));
    }

    pub(super) fn finish_mutation(&mut self, outcome: Result<Completed, String>) {
        let Some(active) = self.io.active.take() else {
            return;
        };
        // 写入前保留路径与原文本，用于写入后标记编辑器改动行。
        let written_originals = active
            .originals
            .iter()
            .map(|(_, document)| {
                (
                    document.path().map(Path::to_path_buf),
                    document.text().to_string(),
                )
            })
            .collect::<Vec<_>>();
        let changed_paths = self.work_paths(&active.job.work);
        let (result, mut documents) = match outcome {
            Ok(c) => (c.result, c.documents),
            Err(e) => (Err(e), Vec::new()),
        };
        teal_workbench_services::observability::outcome(&result);

        // 恢复每个已保留的句柄，包括失败结果中缺失的原始文档。
        for (buffer, original) in active.originals {
            let old_path = original.path().map(Path::to_owned);
            if let Some(index) = documents.iter().position(|(id, _, _)| *id == buffer.id()) {
                let (_, document, changes) = documents.swap_remove(index);
                buffer.finish_io(document, changes);
            } else {
                buffer.finish_io(original, Vec::new());
            }
            if old_path != buffer.path() {
                self.buffer_resource_changed(&buffer, old_path.as_deref());
            }
        }

        // 写入成功后标记编辑器中受影响的改动行；保存或重载会清除标记。
        if result.is_ok() {
            for (path, before) in written_originals {
                let path = match path {
                    Some(path) => path,
                    None => continue,
                };
                let after = std::fs::read_to_string(&path).unwrap_or_default();
                self.mark_editor_changes(&path, &before, &after);
            }
        }

        self.process_buffer_changes();
        if let Some(panel) = active.job.panel {
            if let Work::Resource(intent, _) = &active.job.work
                && let Some(explorer) = self
                    .panels
                    .get_mut(panel)
                    .and_then(AppPanel::as_explorer_mut)
            {
                explorer.refresh_after(intent);
            }
            if matches!(active.job.work, Work::Save(..)) && result.is_ok() {
                if let Some(editor) = self.panels.get_mut(panel).and_then(AppPanel::as_editor_mut) {
                    editor.clear_changed_lines();
                }
                self.reset_journal(panel);
                if result.is_ok() {
                    self.fire_hook(
                        "buffer.saved",
                        serde_json::json!({"panel":format!("{panel:?}")}),
                    );
                }
            }
        }

        // 返回的文档状态及变化到达界面后，才能确认完成。
        if let Some(response) = active.job.response {
            let _ = response.send(result.clone());
        }
        if let Some((client, id)) = active.job.lsp {
            let reply = match &result {
                Ok(()) => serde_json::json!({"applied":true}),
                Err(error) => serde_json::json!({"applied":false,"failureReason":error}),
            };
            self.tasks.spawn(async move {
                let _ = client.respond(id, reply).await;
            });
        }
        if let Err(error) = &result {
            self.message(Severity::Error, "workspace", error);
        }
        for path in changed_paths {
            let generation = self.path_generations.entry(concrete(&path)).or_default();
            *generation = generation.saturating_add(1);
        }
        if result.is_ok() {
            let changed = self.work_paths(&active.job.work);
            self.refresh_workspace_knowledge_after_write(&changed);
        }
        self.resume_deferred_opens();
    }

    /// 用写入前后的文本差异标记已打开编辑器的改动行。
    fn mark_editor_changes(&mut self, path: &Path, before: &str, after: &str) {
        let ranges = changed_line_ranges(before, after);
        let target = concrete(path);
        let ids = self.panels.iter().map(|(id, _)| id).collect::<Vec<_>>();
        for id in ids {
            let Some(panel) = self.panels.get_mut(id) else {
                continue;
            };
            let Some(editor) = panel.as_editor_mut() else {
                continue;
            };
            if editor
                .document()
                .path()
                .map(|path| concrete(path.as_path()))
                != Some(target.clone())
            {
                continue;
            }
            if ranges.is_empty() {
                editor.clear_changed_lines();
            } else {
                editor.mark_changed_lines(ranges.clone());
            }
        }
    }
    pub(super) fn mutation_path_busy(&self, path: &Path) -> bool {
        let path = concrete(path);
        self.io
            .active
            .iter()
            .map(|active| &active.job.work)
            .chain(self.io.queue.iter().map(|job| &job.work))
            .flat_map(|work| self.work_paths(work))
            .map(|candidate| concrete(&candidate))
            .any(|candidate| path.starts_with(&candidate) || candidate.starts_with(&path))
    }

    fn work_paths(&self, work: &Work) -> Vec<PathBuf> {
        match work {
            Work::Agent(path, _, _)
            | Work::AgentChecked(path, _, _, _)
            | Work::Restore(_, path) => {
                vec![path.clone()]
            }
            Work::AgentChangeSet(change_set, selection) => {
                selected_change_roots(&self.workspace, change_set, selection)
            }
            Work::Save(id, target) => target
                .clone()
                .or_else(|| self.buffers.get(*id).and_then(|buffer| buffer.path()))
                .into_iter()
                .collect(),
            Work::Resource(intent, _) => match intent {
                WorkspaceIntent::Create { path, .. } => vec![path.clone()],
                WorkspaceIntent::Rename { old, new } => vec![old.clone(), new.clone()],
                WorkspaceIntent::Copy { paths, destination }
                | WorkspaceIntent::Move { paths, destination } => paths
                    .iter()
                    .cloned()
                    .chain(std::iter::once(destination.clone()))
                    .collect(),
                WorkspaceIntent::Remove { paths, .. } => paths.clone(),
            },
            Work::Lsp(_) => self
                .buffers
                .iter()
                .filter_map(|buffer| buffer.path())
                .collect(),
        }
    }

    pub(super) async fn stop_mutations(&mut self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut warned = false;
        loop {
            self.pump_mutations();
            if self.io.active.is_none() {
                if self.io.queue.is_empty() {
                    self.resume_deferred_opens();
                    break;
                }
                continue;
            }
            if warned {
                match self.runtime_rx.recv().await {
                    Some(event) => self.handle_runtime_event(event).await,
                    None => tokio::time::sleep(Duration::from_millis(100)).await,
                }
                continue;
            }
            tokio::select! {
                event = self.runtime_rx.recv() => {
                    if let Some(event) = event { self.handle_runtime_event(event).await; }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    warned = true;
                    self.message(Severity::Warning, "workspace", "File system operations are still running on exit; waiting for safe completion");
                }
            }
        }
    }

    #[cfg(test)]
    pub(super) async fn settle_mutations(&mut self) {
        loop {
            self.pump_mutations();
            if self.io.active.is_none() {
                if self.io.queue.is_empty() {
                    let ids = self.panels.iter().map(|(id, _)| id).collect::<Vec<_>>();
                    for id in ids {
                        if let Some(panel) =
                            self.panels.get_mut(id).and_then(AppPanel::as_explorer_mut)
                        {
                            panel.settle();
                        }
                    }
                    break;
                }
                continue;
            }
            let event = tokio::time::timeout(Duration::from_secs(10), self.runtime_rx.recv())
                .await
                .unwrap()
                .unwrap();
            self.handle_runtime_event(event).await;
        }
    }
}

fn concrete(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

fn affected(work: &Work, buffer: &BufferHandle, root: &Path) -> bool {
    if let Work::Save(id, _) = work {
        return *id == buffer.id();
    }
    let Some(path) = buffer.path() else {
        return false;
    };
    let path = concrete(&path);
    let under = |target: &Path| path.starts_with(concrete(target));
    match work {
        Work::Agent(target, _, _)
        | Work::AgentChecked(target, _, _, _)
        | Work::Restore(_, target) => under(target),
        Work::AgentChangeSet(change_set, selection) => selected_change_paths(change_set, selection)
            .into_iter()
            .any(|change| under(&rooted_path(root, change))),
        Work::Resource(intent, _) => match intent {
            WorkspaceIntent::Create { path, .. } => under(path),
            WorkspaceIntent::Rename { old, new } => under(old) || under(new),
            WorkspaceIntent::Copy { destination, .. } => under(destination),
            WorkspaceIntent::Move { paths, destination } => {
                under(destination) || paths.iter().any(|p| under(p))
            }
            WorkspaceIntent::Remove { paths, .. } => paths.iter().any(|p| under(p)),
        },
        Work::Lsp(_) => true,
        Work::Save(..) => false,
    }
}

fn execute(
    coordinator: &WorkspaceMutationCoordinator,
    buffers: &teal_workbench_core::BufferStore,
    handles: &[(BufferId, BufferHandle)],
    root: &Path,
    work: Work,
) -> Result<(), String> {
    match work {
        Work::Resource(intent, confined) => coordinator.execute(buffers, intent, confined),
        Work::Agent(path, kind, expected_buffer_version) => execute_agent(
            coordinator,
            buffers,
            root,
            path,
            kind,
            expected_buffer_version,
            None,
        ),
        Work::AgentChecked(path, kind, expected_buffer_version, expected_content_sha256) => {
            execute_agent(
                coordinator,
                buffers,
                root,
                path,
                kind,
                expected_buffer_version,
                expected_content_sha256,
            )
        }
        Work::AgentChangeSet(change_set, selection) => {
            execute_change_set(coordinator, buffers, root, &change_set, &selection)
        }
        Work::Restore(backup, path) => match backup {
            Some(backup) => coordinator.restore(buffers, &backup, &path),
            None => coordinator.remove_created(buffers, &path),
        },
        Work::Save(id, path) => {
            let buffer = &handles
                .iter()
                .find(|(candidate, _)| *candidate == id)
                .ok_or("The buffer no longer exists")?
                .1;
            match path {
                Some(path) => buffer.save_to(path),
                None => buffer.save(),
            }
            .map_err(|e| e.to_string())
        }
        Work::Lsp(edit) => apply_lsp(coordinator, buffers, root, &edit),
    }
}

fn rooted_path(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    }
}

fn execute_change_set(
    coordinator: &WorkspaceMutationCoordinator,
    buffers: &teal_workbench_core::BufferStore,
    root: &Path,
    change_set: &teal_workbench_core::ChangeSet,
    selection: &teal_workbench_core::ChangeSetSelection,
) -> Result<(), String> {
    let selected_paths = selected_change_paths(change_set, selection)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let mut originals = std::collections::BTreeMap::new();
    let mut existed = std::collections::BTreeMap::new();
    for relative in &selected_paths {
        let path = rooted_path(root, relative);
        ensure_workspace_path(root, &path)?;
        let present = path.exists();
        let text = if let Some(buffer) = buffers.find(&path) {
            if buffer.is_read_only() {
                return Err("ChangeSet points to an open binary buffer".into());
            }
            buffer.to_string()
        } else if present {
            Document::open(&path)
                .map_err(|error| error.to_string())?
                .to_string()
        } else {
            String::new()
        };
        originals.insert(relative.clone(), text);
        existed.insert(relative.clone(), present);
    }
    let replacements = change_set
        .apply_selected(&originals, selection)
        .map_err(|error| error.to_string())?;

    let mut applied = Vec::new();
    for (relative, replacement) in replacements {
        let path = rooted_path(root, &relative);
        applied.push(relative.clone());
        if let Err(error) = coordinator.write(buffers, &path, &replacement, true) {
            let mut rollback_errors = Vec::new();
            for applied_path in applied.into_iter().rev() {
                let absolute = rooted_path(root, &applied_path);
                let before = &originals[&applied_path];
                let restored = if existed[&applied_path] {
                    coordinator.write(buffers, &absolute, before, true)
                } else if absolute.exists() {
                    coordinator
                        .write(buffers, &absolute, before, true)
                        .and_then(|()| {
                            coordinator.execute(
                                buffers,
                                WorkspaceIntent::Remove {
                                    paths: vec![absolute.clone()],
                                    permanent: true,
                                },
                                true,
                            )
                        })
                } else {
                    Ok(())
                };
                if let Err(rollback) = restored {
                    rollback_errors.push(format!("{}: {rollback}", applied_path.display()));
                }
            }
            return if rollback_errors.is_empty() {
                Err(error)
            } else {
                Err(format!(
                    "{error}; ChangeSet rollback failed: {}",
                    rollback_errors.join("; ")
                ))
            };
        }
    }
    Ok(())
}

fn selected_change_paths<'a>(
    change_set: &'a teal_workbench_core::ChangeSet,
    selection: &'a teal_workbench_core::ChangeSetSelection,
) -> Vec<&'a PathBuf> {
    match selection {
        teal_workbench_core::ChangeSetSelection::All => {
            change_set.files.iter().map(|file| &file.path).collect()
        }
        teal_workbench_core::ChangeSetSelection::File(path)
        | teal_workbench_core::ChangeSetSelection::Hunk { path, .. } => vec![path],
    }
}

/// 选择实际会写入的工作区绝对路径，供检查点和作业队列使用同一集合。
pub(super) fn selected_change_roots(
    root: &Path,
    change_set: &teal_workbench_core::ChangeSet,
    selection: &teal_workbench_core::ChangeSetSelection,
) -> Vec<PathBuf> {
    selected_change_paths(change_set, selection)
        .into_iter()
        .map(|path| rooted_path(root, path))
        .collect()
}

fn execute_agent(
    coordinator: &WorkspaceMutationCoordinator,
    buffers: &teal_workbench_core::BufferStore,
    root: &Path,
    path: PathBuf,
    kind: MutationKind,
    expected_buffer_version: Option<String>,
    expected_content_sha256: Option<String>,
) -> Result<(), String> {
    ensure_workspace_path(root, &path)?;
    let actual_buffer_version = buffers.find(&path).map(|buffer| {
        format!(
            "{}:{}",
            buffer.revision(),
            teal_workbench_services::agent_algorithms::digest(buffer.to_string().as_bytes())
        )
    });
    if actual_buffer_version != expected_buffer_version {
        return Err("Open buffer changed before Agent mutation execution".into());
    }
    match kind {
        MutationKind::CreateDirectory => {
            return coordinator.execute(
                buffers,
                WorkspaceIntent::Create {
                    path,
                    directory: true,
                },
                true,
            );
        }
        MutationKind::Move { destination } => {
            return coordinator.execute(
                buffers,
                WorkspaceIntent::Move {
                    paths: vec![path],
                    destination,
                },
                true,
            );
        }
        MutationKind::Delete => {
            return coordinator.execute(
                buffers,
                WorkspaceIntent::Remove {
                    paths: vec![path],
                    permanent: false,
                },
                true,
            );
        }
        MutationKind::WriteNew { text, target } => {
            if target.path() != path {
                return Err("workspace creation path differs from its prepared target".into());
            }
            return coordinator.create_new_resolved(buffers, *target, &text);
        }
        MutationKind::Write(_) | MutationKind::Replace { .. } | MutationKind::ApplyPatch { .. } => {
        }
    }
    let current = if let Some(buffer) = buffers.find(&path) {
        buffer.to_string()
    } else if path.exists() {
        Document::open(&path)
            .map_err(|e| e.to_string())?
            .to_string()
    } else {
        String::new()
    };
    if let Some(expected) = expected_content_sha256
        && teal_workbench_services::agent_algorithms::digest(current.as_bytes()) != expected
    {
        return Err("Agent ChangeSet is stale; the target changed during review".into());
    }
    coordinator.write(buffers, &path, &mutation_text(&current, kind)?, true)
}

fn apply_lsp(
    coordinator: &WorkspaceMutationCoordinator,
    buffers: &teal_workbench_core::BufferStore,
    root: &Path,
    edit: &serde_json::Value,
) -> Result<(), String> {
    let mut documents = buffers
        .iter()
        .filter_map(|buffer| {
            Some((
                url::Url::from_file_path(buffer.path()?).ok()?.to_string(),
                buffer.to_string(),
            ))
        })
        .collect::<HashMap<_, _>>();
    let mut uris = Vec::new();
    if let Some(changes) = edit.get("changes").and_then(serde_json::Value::as_object) {
        uris.extend(changes.keys().cloned());
    }
    if let Some(changes) = edit
        .get("documentChanges")
        .and_then(serde_json::Value::as_array)
    {
        for change in changes {
            for key in ["uri", "oldUri", "newUri"] {
                if let Some(uri) = change.get(key).and_then(serde_json::Value::as_str) {
                    uris.push(uri.to_owned());
                }
            }
            if let Some(uri) = change
                .pointer("/textDocument/uri")
                .and_then(serde_json::Value::as_str)
            {
                uris.push(uri.to_owned());
            }
        }
    }
    for uri in uris {
        let path = edit_path(&uri)?;
        ensure_workspace_path(root, &path)?;
        if let std::collections::hash_map::Entry::Vacant(entry) = documents.entry(uri)
            && path.is_file()
        {
            entry.insert(Document::open(path).map_err(|e| e.to_string())?.to_string());
        }
    }
    let mut operations = Vec::new();
    if let Some(changes) = edit.get("changes") {
        operations.extend(
            parse_workspace_edit(&serde_json::json!({"changes":changes}), &documents)
                .map_err(|e| e.to_string())?,
        );
    }
    if let Some(changes) = edit
        .get("documentChanges")
        .and_then(serde_json::Value::as_array)
    {
        for change in changes {
            let step =
                parse_workspace_edit(&serde_json::json!({"documentChanges":[change]}), &documents)
                    .map_err(|e| e.to_string())?;
            for operation in &step {
                match operation {
                    WorkspaceOperation::Edit(edit) => {
                        let path = edit_path(&edit.uri)?;
                        if let Some(version) = edit.version
                            && buffers.find(&path).is_none_or(|b| {
                                i32::try_from(b.revision()).unwrap_or(i32::MAX) != version
                            })
                        {
                            return Err("LSP edit points to unavailable document version".into());
                        }
                        let mut preview = Document::from_text(
                            documents.get(&edit.uri).ok_or("Missing edit snapshot")?,
                        );
                        let transaction = Transaction::new(
                            edit.edits
                                .iter()
                                .cloned()
                                .map(|edit| TextEdit {
                                    range: edit.range,
                                    insert: edit.insert,
                                })
                                .collect(),
                        )
                        .map_err(|error| error.to_string())?;
                        preview.apply(transaction).map_err(|e| e.to_string())?;
                        documents.insert(edit.uri.clone(), preview.to_string());
                    }
                    WorkspaceOperation::Create(path) => {
                        documents.insert(file_uri(path)?, String::new());
                    }
                    WorkspaceOperation::Rename { old, new } => {
                        let snapshot = documents.clone();
                        for (uri, text) in snapshot {
                            let path = edit_path(&uri)?;
                            if let Ok(suffix) = path.strip_prefix(old) {
                                documents.remove(&uri);
                                documents.insert(file_uri(&new.join(suffix))?, text);
                            }
                        }
                    }
                    WorkspaceOperation::Delete(path) => {
                        documents
                            .retain(|uri, _| edit_path(uri).is_ok_and(|p| !p.starts_with(path)));
                    }
                }
            }
            operations.extend(step);
        }
    }
    for operation in operations {
        match operation {
            WorkspaceOperation::Edit(edit) => {
                let transaction = Transaction::new(
                    edit.edits
                        .into_iter()
                        .map(|edit| TextEdit {
                            range: edit.range,
                            insert: edit.insert,
                        })
                        .collect(),
                )
                .map_err(|error| error.to_string())?;
                coordinator.apply_transaction(
                    buffers,
                    &edit_path(&edit.uri)?,
                    transaction,
                    true,
                )?;
            }
            WorkspaceOperation::Create(path) => coordinator.execute(
                buffers,
                WorkspaceIntent::Create {
                    path,
                    directory: false,
                },
                true,
            )?,
            WorkspaceOperation::Rename { old, new } => {
                coordinator.execute(buffers, WorkspaceIntent::Rename { old, new }, true)?;
            }
            WorkspaceOperation::Delete(path) => coordinator.execute(
                buffers,
                WorkspaceIntent::Remove {
                    paths: vec![path],
                    permanent: false,
                },
                true,
            )?,
        }
    }
    Ok(())
}

fn edit_path(uri: &str) -> Result<PathBuf, String> {
    url::Url::parse(uri)
        .map_err(|e| e.to_string())?
        .to_file_path()
        .map_err(|()| "Invalid edit URI".into())
}

fn file_uri(path: &Path) -> Result<String, String> {
    url::Url::from_file_path(path)
        .map(|url| url.to_string())
        .map_err(|()| "Invalid file path".into())
}

#[cfg(test)]
mod changed_line_tests {
    use super::changed_line_ranges;

    #[test]
    fn identical_text_has_no_changed_lines() {
        assert!(changed_line_ranges("one\ntwo\n", "one\ntwo\n").is_empty());
    }

    #[test]
    fn inserts_updates_and_deletes_map_to_new_line_indices() {
        // 在第二行插入：新文本的第 2 行（0 基 1）被标记。
        assert_eq!(
            changed_line_ranges("one\ntwo\n", "one\ninserted\ntwo\n"),
            vec![1..2]
        );
        // 连续插入合并为一个区间。
        assert_eq!(
            changed_line_ranges("one\nfour\n", "one\ntwo\nthree\nfour\n"),
            vec![1..3]
        );
        // 更新一行只标记该行。
        assert_eq!(
            changed_line_ranges("one\ntwo\nthree\n", "one\nTWO\nthree\n"),
            vec![1..2]
        );
        // 删除一行标记删除点所在的新行。
        assert_eq!(
            changed_line_ranges("one\ntwo\nthree\n", "one\nthree\n"),
            vec![1..2]
        );
    }
}

#[cfg(test)]
mod creation_tests {
    use super::*;
    use std::sync::Arc;

    struct Audit;
    impl teal_security::AuditSink for Audit {
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
    fn prepared_creation_preserves_receipt_bytes_and_rejects_reuse() {
        let root = tempfile::tempdir().unwrap();
        let coordinator =
            WorkspaceMutationCoordinator::new(root.path().into(), root.path().join("checkpoints"))
                .unwrap()
                .with_audit_context(teal_security::AuditContext::new(Arc::new(Audit)));
        let path = root.path().join("nested/note.md");
        let text = "first\r\nsecond\rthird";
        let kind = MutationKind::WriteNew {
            text: text.into(),
            target: Box::new(coordinator.resolve_write(&path, true).unwrap()),
        };
        let buffers = teal_workbench_core::BufferStore::default();
        execute_agent(
            &coordinator,
            &buffers,
            root.path(),
            path.clone(),
            kind.clone(),
            None,
            None,
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), text.as_bytes());
        assert!(
            execute_agent(&coordinator, &buffers, root.path(), path, kind, None, None).is_err()
        );
    }
}
