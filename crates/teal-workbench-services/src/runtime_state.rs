//! 任务相关工作区资源的后台观察；观察结果不能代替工具授权。

use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use teal_agent::{
    ActionContext, ActionResourceAccess, AgentError, HostRuntimeState, PermissionProvider,
    ProviderFuture, ResourceVersion, RuntimeEnvironment, RuntimeEnvironmentChange, RuntimeProgress,
    RuntimeStateProvider, ToolCall, ToolOutput,
};

const MAX_PATHS: usize = 128;
const MAX_EVENTS: usize = 64;
const MAX_RUNS: usize = 16;

/// 只暴露打开文档的版本身份，不读取或转发文档正文。
pub trait BufferVersionProvider: Send + Sync {
    /// 返回指定路径当前打开缓冲区的版本；未打开时返回空。
    fn buffer_version(&self, path: &Path) -> Option<String>;
}

#[cfg(feature = "tools")]
impl BufferVersionProvider for crate::tools::WorkspaceTools {
    fn buffer_version(&self, path: &Path) -> Option<String> {
        self.runtime_buffer_version(path)
    }
}

#[cfg(feature = "tools")]
impl BufferVersionProvider for crate::tools::WorkspaceToolRuntime {
    fn buffer_version(&self, path: &Path) -> Option<String> {
        self.runtime_buffer_version(path)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Stamp {
    Missing,
    Present {
        len: u64,
        modified: Option<SystemTime>,
    },
    Unavailable,
}

#[derive(Default)]
struct RunWatch {
    last_seen: u64,
    paths: BTreeMap<PathBuf, Stamp>,
    buffers: BTreeMap<PathBuf, Option<String>>,
    changes: VecDeque<(u64, RuntimeEnvironmentChange)>,
    revision: u64,
    omitted: u32,
    phase: Option<String>,
}

struct Monitor {
    epoch: u64,
    next_use: u64,
    runs: BTreeMap<String, RunWatch>,
    policy_version: String,
}

/// 观察单个工作区内已访问的任务资源。
///
/// 构建时启动有界后台扫描；每次模型安全边界再次扫描，因此文件事件只是
/// 提醒模型重看环境，真正的权限和版本仍由工具执行前的原路径复核。
#[derive(Clone)]
pub struct WorkspaceRuntimeState {
    root: PathBuf,
    policy: Arc<dyn PermissionProvider>,
    inner: Arc<Mutex<Monitor>>,
    buffer_source: Arc<Mutex<Option<Arc<dyn BufferVersionProvider>>>>,
    tool_paths: Vec<PathBuf>,
    configuration_paths: Vec<PathBuf>,
    background_active: bool,
}

impl WorkspaceRuntimeState {
    /// 为规范工作区创建观察器；无 Tokio runtime 时仍可在快照边界工作。
    ///
    /// # Errors
    /// 工作区不存在或不是目录时返回错误。此函数不读取任务文件或授予权限。
    pub fn new(
        root: impl AsRef<Path>,
        policy: Arc<dyn PermissionProvider>,
    ) -> Result<Self, AgentError> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|error| AgentError::Protocol(error.to_string()))?;
        if !root.is_dir() {
            return Err(AgentError::Protocol(
                "runtime state root is not a directory".into(),
            ));
        }
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX))
            .unwrap_or_default();
        let inner = Arc::new(Mutex::new(Monitor {
            epoch,
            next_use: 0,
            runs: BTreeMap::new(),
            policy_version: policy.version().to_owned(),
        }));
        let buffer_source: Arc<Mutex<Option<Arc<dyn BufferVersionProvider>>>> = Arc::default();
        let background_active = if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let weak: Weak<Mutex<Monitor>> = Arc::downgrade(&inner);
            let policy = Arc::clone(&policy);
            let buffers = Arc::clone(&buffer_source);
            handle.spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(750));
                loop {
                    interval.tick().await;
                    let Some(inner) = weak.upgrade() else { break };
                    if let Ok(mut guard) = inner.lock() {
                        let source = buffers.lock().ok().and_then(|slot| slot.clone());
                        scan(&mut guard, policy.version(), source.as_deref());
                    }
                }
            });
            true
        } else {
            false
        };
        Ok(Self {
            root,
            policy,
            inner,
            buffer_source,
            tool_paths: Vec::new(),
            configuration_paths: Vec::new(),
            background_active,
        })
    }

    /// 添加启动前已由宿主选定的工具程序；只观察存在与版本，不读取内容。
    #[must_use]
    pub fn with_tool_paths(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.tool_paths = paths.into_iter().collect();
        self
    }

    /// 添加宿主已经解析的配置文件路径；只观察元数据，不读取秘密或配置正文。
    #[must_use]
    pub fn with_configuration_paths(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.configuration_paths = paths.into_iter().collect();
        self
    }

    /// 使用工作台共享的打开文档版本目录监测未落盘的用户编辑。
    #[must_use]
    pub fn with_buffer_source(self, source: Arc<dyn BufferVersionProvider>) -> Self {
        if let Ok(mut slot) = self.buffer_source.lock() {
            *slot = Some(source);
        }
        self
    }

    fn register(&self, watch: &mut RunWatch, paths: &[String]) {
        for path in paths {
            if let Some(path) = within_root(&self.root, path) {
                insert_path(watch, path.clone());
                let mut parent = if path.is_dir() {
                    path.as_path()
                } else {
                    path.parent().unwrap_or(&self.root)
                };
                while parent.starts_with(&self.root) {
                    insert_path(watch, parent.join("AGENTS.md"));
                    if parent == self.root {
                        break;
                    }
                    parent = parent.parent().unwrap_or(&self.root);
                }
            }
        }
        for file in [".teal/config.toml", ".teal/bench.toml", ".teal/model.toml"] {
            insert_path(watch, self.root.join(file));
        }
        for path in &self.tool_paths {
            insert_path(watch, path.clone());
        }
        for path in &self.configuration_paths {
            insert_path(watch, path.clone());
        }
        if let Ok(slot) = self.buffer_source.lock()
            && let Some(source) = slot.as_ref()
        {
            for path in watch.paths.keys() {
                watch
                    .buffers
                    .entry(path.clone())
                    .or_insert_with(|| source.buffer_version(path));
            }
        }
    }

    fn note_self_change(&self, context: &ActionContext, resources: &[ResourceVersion]) {
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let Some(watch) = guard.runs.get_mut(&context.run_id) else {
            return;
        };
        let buffer_source = self.buffer_source.lock().ok().and_then(|slot| slot.clone());
        for resource in resources {
            if resource.access == ActionResourceAccess::Write
                && let Some(input) = resource.resource.strip_prefix("file://")
                && let Some(path) = within_root(&self.root, input)
            {
                if watch.paths.len() >= MAX_PATHS && !watch.paths.contains_key(&path) {
                    watch.omitted = watch.omitted.saturating_add(1);
                    continue;
                }
                let stamp = stamp(&path, None);
                watch.paths.insert(path.clone(), stamp);
                if let Some(source) = buffer_source.as_deref() {
                    watch
                        .buffers
                        .insert(path.clone(), source.buffer_version(&path));
                }
                watch.revision = watch.revision.saturating_add(1);
                push_change(
                    watch,
                    RuntimeEnvironmentChange {
                        resource: path.to_string_lossy().into_owned(),
                        kind: "file".into(),
                        state: "changed".into(),
                        origin: "self".into(),
                    },
                );
            }
        }
    }
}

impl RuntimeStateProvider for WorkspaceRuntimeState {
    fn snapshot(
        &self,
        context: ActionContext,
        paths: Vec<String>,
        since: Option<(u64, u64)>,
    ) -> ProviderFuture<Result<HostRuntimeState, AgentError>> {
        let this = self.clone();
        Box::pin(async move {
            let mut guard = this
                .inner
                .lock()
                .map_err(|_| AgentError::Protocol("runtime monitor lock is unavailable".into()))?;
            let policy = this.policy.version().to_owned();
            let buffer_source = this.buffer_source.lock().ok().and_then(|slot| slot.clone());
            scan(&mut guard, &policy, buffer_source.as_deref());
            let epoch = guard.epoch;
            let new_run = !guard.runs.contains_key(&context.run_id);
            if new_run
                && guard.runs.len() >= MAX_RUNS
                && let Some(oldest) = guard
                    .runs
                    .iter()
                    .min_by_key(|(_, watch)| watch.last_seen)
                    .map(|(id, _)| id.clone())
            {
                guard.runs.remove(&oldest);
            }
            guard.next_use = guard.next_use.saturating_add(1);
            let last_seen = guard.next_use;
            let watch = guard.runs.entry(context.run_id).or_default();
            watch.last_seen = last_seen;
            this.register(watch, &paths);
            scan_one(watch, buffer_source.as_deref());
            let (mut health, changes, omitted) = match since {
                Some(_) if new_run => ("baseline_reset", Vec::new(), watch.omitted),
                Some((prior_epoch, _)) if prior_epoch != epoch => {
                    ("baseline_reset", Vec::new(), watch.omitted)
                }
                Some((_, revision)) => {
                    let changes = watch
                        .changes
                        .iter()
                        .filter(|(seq, _)| *seq > revision)
                        .map(|(_, change)| change.clone())
                        .collect::<Vec<_>>();
                    ("active", changes, watch.omitted)
                }
                None => ("active", Vec::new(), watch.omitted),
            };
            let phase = watch.phase.clone();
            let revision = watch.revision;
            if health != "baseline_reset"
                && (!this.background_active
                    || omitted > 0
                    || watch
                        .paths
                        .values()
                        .any(|stamp| matches!(stamp, Stamp::Unavailable)))
            {
                health = "degraded";
            }
            Ok(HostRuntimeState {
                phase,
                graph: None,
                environment: RuntimeEnvironment {
                    epoch,
                    revision,
                    health: health.into(),
                    changes,
                    omitted,
                },
            })
        })
    }

    fn project_tool(
        &self,
        context: &ActionContext,
        call: &ToolCall,
        output: &ToolOutput,
        resources: &[ResourceVersion],
    ) -> Option<RuntimeProgress> {
        if output.is_error {
            return None;
        }
        if resources
            .iter()
            .any(|resource| resource.access == ActionResourceAccess::Write)
        {
            self.note_self_change(context, resources);
        }
        if let Ok(mut guard) = self.inner.lock()
            && let Some(watch) = guard.runs.get_mut(&context.run_id)
        {
            watch.phase = match call.name.as_str() {
                "workspace.read_file" | "workspace.search" | "workspace.list" => {
                    Some("exploring".into())
                }
                "workspace.write_file" | "workspace.apply_patch" | "workspace.replace_file" => {
                    Some("authoring".into())
                }
                "teal.check" | "teal.test" | "teal.fmt_check" => Some("checking".into()),
                "teal.run" => Some("executing".into()),
                _ => watch.phase.clone(),
            };
        }
        #[cfg(feature = "tools")]
        if call.name == crate::agent_algorithms::PLAN_TOOL_NAME {
            let plan = crate::agent_algorithms::AgentPlan::from_arguments(&call.arguments).ok()?;
            return Some(RuntimeProgress {
                source_call_id: call.id.clone(),
                source_tool: call.name.clone(),
                generation: context.generation,
                revision: 0,
                source: "model_plan".into(),
                steps: plan
                    .steps()
                    .iter()
                    .enumerate()
                    .map(|(index, step)| teal_agent::RuntimePlanStep {
                        index: u8::try_from(index).unwrap_or(u8::MAX),
                        text: step.step.clone(),
                        status: step.status.name().to_owned(),
                    })
                    .collect(),
            });
        }
        None
    }
}

fn within_root(root: &Path, input: &str) -> Option<PathBuf> {
    let raw = Path::new(input);
    if raw
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return None;
    }
    let path = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        root.join(raw)
    };
    let ancestor = path.ancestors().find(|path| path.exists())?;
    if !ancestor.canonicalize().ok()?.starts_with(root) {
        return None;
    }
    Some(path)
}

fn insert_path(watch: &mut RunWatch, path: PathBuf) {
    if watch.paths.len() >= MAX_PATHS && !watch.paths.contains_key(&path) {
        watch.omitted = watch.omitted.saturating_add(1);
    } else {
        watch
            .paths
            .entry(path.clone())
            .or_insert_with(|| stamp(&path, None));
    }
}

fn stamp(path: &Path, before: Option<&Stamp>) -> Stamp {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Stamp::Unavailable,
        Ok(metadata) => {
            let len = metadata.len();
            let modified = metadata.modified().ok();
            if let Some(Stamp::Present {
                len: old_len,
                modified: old_modified,
            }) = before
                && len == *old_len
                && modified == *old_modified
            {
                return before.cloned().unwrap_or(Stamp::Unavailable);
            }
            Stamp::Present { len, modified }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Stamp::Missing,
        Err(_) => Stamp::Unavailable,
    }
}

fn scan(
    monitor: &mut Monitor,
    policy_version: &str,
    buffer_source: Option<&dyn BufferVersionProvider>,
) {
    if monitor.policy_version != policy_version {
        monitor.policy_version = policy_version.to_owned();
        for watch in monitor.runs.values_mut() {
            watch.revision = watch.revision.saturating_add(1);
            push_change(
                watch,
                RuntimeEnvironmentChange {
                    resource: "permission-policy".into(),
                    kind: "permission".into(),
                    state: "changed".into(),
                    origin: "external".into(),
                },
            );
        }
    }
    for watch in monitor.runs.values_mut() {
        scan_one(watch, buffer_source);
    }
}

fn scan_one(watch: &mut RunWatch, buffer_source: Option<&dyn BufferVersionProvider>) {
    let paths = watch
        .paths
        .iter()
        .map(|(path, before)| (path.clone(), before.clone()))
        .collect::<Vec<_>>();
    for (path, before) in paths {
        let after = stamp(&path, Some(&before));
        if after == before {
            continue;
        }
        watch.paths.insert(path.clone(), after.clone());
        watch.revision = watch.revision.saturating_add(1);
        let kind = if path.file_name().is_some_and(|name| name == "AGENTS.md") {
            "instructions"
        } else if path.file_name().is_some_and(|name| {
            matches!(
                name.to_string_lossy().as_ref(),
                "config.toml" | "bench.toml" | "model.toml"
            )
        }) {
            "permission"
        } else if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("teal-"))
        {
            "tool"
        } else {
            "file"
        };
        push_change(
            watch,
            RuntimeEnvironmentChange {
                resource: match kind {
                    "instructions" => "project-instructions".into(),
                    "permission" => "permission-policy".into(),
                    "tool" => format!(
                        "tool://{}",
                        path.file_name().map_or_else(
                            || "unknown".to_owned(),
                            |name| name.to_string_lossy().into_owned(),
                        )
                    ),
                    _ => path.to_string_lossy().into_owned(),
                },
                kind: kind.into(),
                state: match after {
                    Stamp::Missing => "removed",
                    Stamp::Unavailable => "unavailable",
                    Stamp::Present { .. } => "changed",
                }
                .into(),
                origin: "external".into(),
            },
        );
    }
    if let Some(source) = buffer_source {
        let paths = watch
            .buffers
            .iter()
            .map(|(path, version)| (path.clone(), version.clone()))
            .collect::<Vec<_>>();
        for (path, before) in paths {
            let after = source.buffer_version(&path);
            if before == after {
                continue;
            }
            watch.buffers.insert(path.clone(), after);
            watch.revision = watch.revision.saturating_add(1);
            push_change(
                watch,
                RuntimeEnvironmentChange {
                    resource: path.to_string_lossy().into_owned(),
                    kind: "file".into(),
                    state: "changed".into(),
                    origin: "external".into(),
                },
            );
        }
    }
}

fn push_change(watch: &mut RunWatch, change: RuntimeEnvironmentChange) {
    if let Some((revision, prior)) = watch.changes.back_mut()
        && prior.resource == change.resource
        && prior.kind == change.kind
        && prior.state == change.state
        && prior.origin == change.origin
    {
        *revision = watch.revision;
        return;
    }
    watch.changes.push_back((watch.revision, change));
    if watch.changes.len() > MAX_EVENTS {
        watch.changes.pop_front();
        watch.omitted = watch.omitted.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use teal_agent::AllowAllPermissions;

    #[derive(Default)]
    struct TestBuffers(Mutex<BTreeMap<PathBuf, String>>);

    impl BufferVersionProvider for TestBuffers {
        fn buffer_version(&self, path: &Path) -> Option<String> {
            self.0.lock().ok()?.get(path).cloned()
        }
    }

    fn context() -> ActionContext {
        ActionContext {
            run_id: "watch-test".into(),
            task_id: "task".into(),
            generation: 1,
            policy_version: String::new(),
        }
    }

    #[tokio::test]
    async fn observes_only_registered_task_files_and_marks_own_edits() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.tl");
        let other = directory.path().join("other.tl");
        std::fs::write(&first, "a").unwrap();
        std::fs::write(&other, "a").unwrap();
        let monitor =
            WorkspaceRuntimeState::new(directory.path(), Arc::new(AllowAllPermissions)).unwrap();
        let baseline = monitor
            .snapshot(context(), vec![first.to_string_lossy().into_owned()], None)
            .await
            .unwrap();
        std::fs::write(&other, "changed outside the watched set").unwrap();
        let same = monitor
            .snapshot(
                context(),
                Vec::new(),
                Some((baseline.environment.epoch, baseline.environment.revision)),
            )
            .await
            .unwrap();
        assert!(same.environment.changes.is_empty());

        std::fs::write(&first, "external change").unwrap();
        let changed = monitor
            .snapshot(
                context(),
                Vec::new(),
                Some((baseline.environment.epoch, baseline.environment.revision)),
            )
            .await
            .unwrap();
        assert!(
            changed.environment.changes.iter().any(
                |event| event.origin == "external" && event.resource == first.to_string_lossy()
            )
        );

        std::fs::write(&first, "self edit").unwrap();
        monitor.project_tool(
            &context(),
            &ToolCall {
                id: "write-1".into(),
                name: "workspace.write_file".into(),
                arguments: serde_json::json!({"path": first.to_string_lossy()}),
            },
            &ToolOutput::value(serde_json::Value::Null),
            &[ResourceVersion {
                access: ActionResourceAccess::Write,
                resource: format!("file://{}", first.to_string_lossy().replace('\\', "/")),
                version: "after".into(),
            }],
        );
        let own = monitor
            .snapshot(
                context(),
                Vec::new(),
                Some((changed.environment.epoch, changed.environment.revision)),
            )
            .await
            .unwrap();
        assert!(
            own.environment
                .changes
                .iter()
                .any(|event| event.origin == "self")
        );
        let restarted = monitor
            .snapshot(
                context(),
                Vec::new(),
                Some((baseline.environment.epoch.wrapping_add(1), 0)),
            )
            .await
            .unwrap();
        assert_eq!(restarted.environment.health, "baseline_reset");
    }

    #[tokio::test]
    async fn reports_policy_and_tool_changes_without_reading_file_contents() {
        let directory = tempfile::tempdir().unwrap();
        let tool = directory.path().join("teal-c.exe");
        let monitor = WorkspaceRuntimeState::new(directory.path(), Arc::new(AllowAllPermissions))
            .unwrap()
            .with_tool_paths([tool.clone()]);
        let baseline = monitor.snapshot(context(), Vec::new(), None).await.unwrap();
        std::fs::write(&tool, "binary").unwrap();
        {
            let mut guard = monitor.inner.lock().unwrap();
            guard.policy_version = "older policy".into();
        }
        let changed = monitor
            .snapshot(
                context(),
                Vec::new(),
                Some((baseline.environment.epoch, baseline.environment.revision)),
            )
            .await
            .unwrap();
        assert!(
            changed
                .environment
                .changes
                .iter()
                .any(|change| change.kind == "tool")
        );
        assert!(
            changed
                .environment
                .changes
                .iter()
                .any(|change| change.kind == "permission")
        );
    }

    #[tokio::test]
    async fn observes_open_buffer_revision_without_disk_write() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("edited.tl");
        std::fs::write(&path, "unchanged disk").unwrap();
        let buffers = Arc::new(TestBuffers::default());
        let monitor = WorkspaceRuntimeState::new(directory.path(), Arc::new(AllowAllPermissions))
            .unwrap()
            .with_buffer_source(buffers.clone());
        let baseline = monitor
            .snapshot(context(), vec![path.to_string_lossy().into_owned()], None)
            .await
            .unwrap();
        buffers
            .0
            .lock()
            .unwrap()
            .insert(path.clone(), "buffer:2".into());
        let changed = monitor
            .snapshot(
                context(),
                Vec::new(),
                Some((baseline.environment.epoch, baseline.environment.revision)),
            )
            .await
            .unwrap();
        assert!(
            changed
                .environment
                .changes
                .iter()
                .any(|event| { event.resource == path.to_string_lossy() && event.kind == "file" })
        );
    }

    #[tokio::test]
    async fn accepts_normalized_resource_paths_from_host_tool_bindings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.tl");
        std::fs::write(&path, "one").unwrap();
        let monitor =
            WorkspaceRuntimeState::new(directory.path(), Arc::new(AllowAllPermissions)).unwrap();
        let normalized = path
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let baseline = monitor
            .snapshot(context(), vec![normalized], None)
            .await
            .unwrap();
        std::fs::write(&path, "second version").unwrap();
        let changed = monitor
            .snapshot(
                context(),
                Vec::new(),
                Some((baseline.environment.epoch, baseline.environment.revision)),
            )
            .await
            .unwrap();
        assert!(
            changed
                .environment
                .changes
                .iter()
                .any(|change| change.kind == "file")
        );
    }
}
