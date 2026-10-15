//! 统一协调工作区文本和资源修改。
//!
//! 打开文件通过共享缓冲区事务更新，关闭文件复用保留编码的原子写入器。
//! 检查点和操作次序不构成跨文件原子事务；后续失败可能留下先前已完成的资源变化。

use crate::anchored::DirectoryAnchor;
use crate::{BufferStore, Document, DocumentChange, TextEncoding, Transaction};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_CHECKPOINT: AtomicU64 = AtomicU64::new(1);

/// 用户或协议请求的资源操作；路径是否需要限制由进入协调器的策略决定。
#[derive(Clone, Debug)]
pub enum WorkspaceIntent {
    /// 新建空文件或目录；目标已存在时报错。
    Create {
        /// 目标路径。
        path: PathBuf,
        /// 为真时建立目录，为假时建立空文件并补齐父目录。
        directory: bool,
    },
    /// 重命名路径，并把已打开的缓冲区迁移到新位置。
    Rename {
        /// 原路径。
        old: PathBuf,
        /// 新路径；已存在时报错。
        new: PathBuf,
    },
    /// 把一组路径复制到目标目录，各自保留原文件名。
    Copy {
        /// 待复制的源路径。
        paths: Vec<PathBuf>,
        /// 接收副本的目录。
        destination: PathBuf,
    },
    /// 把一组路径移动到目标目录，各自保留原文件名。
    Move {
        /// 待移动的源路径。
        paths: Vec<PathBuf>,
        /// 接收路径的目录。
        destination: PathBuf,
    },
    /// 删除一组路径，并解除对应缓冲区的文件绑定。
    Remove {
        /// 待删除的路径。
        paths: Vec<PathBuf>,
        /// 为真时直接删除，为假时移入平台回收站。
        permanent: bool,
    },
}

mod targets;
use targets::Root;
pub use targets::{ResolvedEntry, ResolvedTarget};

/// 共享文件修改规则；串行执行和工作线程副本仍由调用方安排。
#[derive(Clone)]
pub struct WorkspaceMutationCoordinator {
    audit: teal_security::AuditContext,
    root: Root,
    checkpoints: PathBuf,
}

impl WorkspaceMutationCoordinator {
    /// 固定工作区身份与绝对检查点目录；相对检查点目录也以工作区为基准。
    ///
    /// # Errors
    /// 工作区不存在、不是目录或无法取得文件身份时失败。
    pub fn new(workspace: PathBuf, checkpoints: PathBuf) -> Result<Self, String> {
        let root = Root::new(&workspace)?;
        let checkpoints = std::path::absolute(if checkpoints.is_absolute() {
            checkpoints
        } else {
            root.input().join(checkpoints)
        })
        .map_err(|error| error.to_string())?;
        Ok(Self {
            audit: teal_security::process_context(),
            root,
            checkpoints,
        })
    }

    /// 为文件事务指定宿主提供的持久审计接收器。
    #[must_use]
    pub fn with_audit_context(mut self, audit: teal_security::AuditContext) -> Self {
        self.audit = audit;
        self
    }

    fn audited<T>(&self, operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let span = tracing::info_span!("teal.workspace.mutate");
        let _entered = span.enter();
        let event = teal_security::Event::new(
            teal_security::Action::WorkspaceMutation,
            teal_security::Phase::Intent,
            teal_security::Outcome::Pending,
            teal_security::new_correlation_id(),
        )
        .map_err(|error| error.to_string())?
        .with_actor(teal_security::Actor::Host)
        .with_workspace(self.root.input());
        let audit = self.audit.begin(event).map_err(|error| {
            format!("Audit unavailable; workspace mutation was not started: {error}")
        })?;
        let result = operation();
        audit
            .finish(if result.is_ok() {
                teal_security::Outcome::Succeeded
            } else {
                teal_security::Outcome::Failed
            })
            .map_err(|error| {
                format!("Audit result unavailable; workspace effect may be unknown: {error}")
            })?;
        result
    }

    /// 解析必须存在的读取目标；最终链接被解析。
    ///
    /// # Errors
    /// 缺失、链接循环、越界或无法取得身份时失败。
    pub fn resolve_read(&self, path: &Path, confined: bool) -> Result<ResolvedTarget, String> {
        ResolvedTarget::new(&self.root, path, confined, true)
    }

    /// 解析可包含缺失后缀的写入目标；执行阶段必须使用返回对象。
    ///
    /// # Errors
    /// 工作区变化、链接循环、越界或无法取得身份时失败。
    pub fn resolve_write(&self, path: &Path, confined: bool) -> Result<ResolvedTarget, String> {
        ResolvedTarget::new(&self.root, path, confined, false)
    }

    /// 解析目录项；允许缺失的重命名目标，不跟随最后一个链接。
    ///
    /// # Errors
    /// 父目录越界、工作区变化或无法取得目录项身份时失败。
    pub fn resolve_entry(&self, path: &Path, confined: bool) -> Result<ResolvedEntry, String> {
        ResolvedEntry::new(&self.root, path, confined, false)
    }

    fn parent(&self, target: &ResolvedTarget) -> Result<ResolvedTarget, String> {
        target.require_workspace(&self.root)?;
        target.revalidate()?;
        let parent = target
            .path()
            .parent()
            .ok_or("Missing target parent directory")?;
        let parent = self.resolve_write(parent, target.confined())?;
        parent.revalidate()?;
        DirectoryAnchor::ensure(parent.path()).map_err(|error| error.to_string())?;
        target.revalidate()?;
        parent.materialized()
    }

    /// 在父目录证明仍有效时取得目录锚点；后续叶操作必须使用该锚点。
    fn anchor_parent(
        &self,
        target: &ResolvedTarget,
    ) -> Result<(ResolvedTarget, DirectoryAnchor), String> {
        let parent = self.parent(target)?;
        let anchor = DirectoryAnchor::open(parent.path()).map_err(|error| error.to_string())?;
        parent.revalidate()?;
        Ok((parent, anchor))
    }

    fn target_name(target: &ResolvedTarget) -> Result<&std::ffi::OsStr, String> {
        target
            .path()
            .file_name()
            .ok_or_else(|| "Missing target file name".to_owned())
    }

    fn entry_name(entry: &ResolvedEntry) -> Result<&std::ffi::OsStr, String> {
        entry
            .path()
            .file_name()
            .ok_or_else(|| "Missing directory entry name".to_owned())
    }

    fn backup_target(&self) -> Result<ResolvedTarget, String> {
        let directory = self.resolve_write(&self.checkpoints, false)?;
        directory.revalidate()?;
        DirectoryAnchor::ensure(directory.path()).map_err(|error| error.to_string())?;
        let id = NEXT_CHECKPOINT.fetch_add(1, Ordering::Relaxed);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        self.resolve_write(&directory.path().join(format!("{stamp}-{id}.bak")), false)
    }

    fn checkpoint(&self, target: &ResolvedTarget) -> Result<(), String> {
        target.require_workspace(&self.root)?;
        target.revalidate()?;
        if target.exists() && !target.is_dir() {
            let (_, source_anchor) = self.anchor_parent(target)?;
            let source_name = Self::target_name(target)?;
            let contents = source_anchor
                .read(source_name)
                .map_err(|error| error.to_string())?;
            let backup = self.backup_target()?;
            let (_, backup_anchor) = self.anchor_parent(&backup)?;
            let backup_name = Self::target_name(&backup)?;
            backup_anchor
                .create_new(backup_name, &contents)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// 为文件集合建立检查点；尚不存在的目标保留明确的不存在标记。
    ///
    /// # Errors
    /// 目录、越界路径或读取/备份失败时返回错误；检查点不是跨文件事务。
    pub fn group_checkpoint(&self, paths: &[PathBuf]) -> Result<String, String> {
        self.audited(|| self.group_checkpoint_inner(paths))
    }

    fn group_checkpoint_inner(&self, paths: &[PathBuf]) -> Result<String, String> {
        if paths.is_empty() {
            return Err("A group checkpoint needs at least one path".into());
        }
        let mut entries = Vec::new();
        for path in paths {
            let target = self.resolve_write(path, true)?;
            if target.is_dir() {
                return Err("Directory checkpoints are unsupported".into());
            }
            let backup = self.backup_target()?;
            if target.exists() {
                let (_, source_anchor) = self.anchor_parent(&target)?;
                let contents = source_anchor
                    .read(Self::target_name(&target)?)
                    .map_err(|error| error.to_string())?;
                let (_, backup_anchor) = self.anchor_parent(&backup)?;
                backup_anchor
                    .create_new(Self::target_name(&backup)?, &contents)
                    .map_err(|error| error.to_string())?;
            } else {
                let (_, backup_anchor) = self.anchor_parent(&backup)?;
                backup_anchor
                    .create_new(Self::target_name(&backup)?, b"")
                    .map_err(|error| error.to_string())?;
            }
            entries.push(serde_json::json!({"backup":backup.path(),"path":target.path(),"existed":target.exists()}));
        }
        Ok(serde_json::json!({"backup":entries[0]["backup"],"path":entries[0]["path"],"group":entries}).to_string())
    }

    /// 删除操作新建的文件；缺失时幂等，最终链接只删除目录项。
    ///
    /// # Errors
    /// 普通目录、越界或身份变化时失败，不递归删除一个无法证明是新建的目录。
    pub fn remove_created(&self, buffers: &BufferStore, path: &Path) -> Result<(), String> {
        self.audited(|| self.remove_created_inner(buffers, path))
    }

    fn remove_created_inner(&self, buffers: &BufferStore, path: &Path) -> Result<(), String> {
        let entry = self.resolve_entry(path, true)?;
        if entry.is_dir() && !entry.is_link() {
            return Err("Created-file rollback cannot remove a directory".into());
        }
        if !entry.exists() {
            return Ok(());
        }
        self.remove_entry(buffers, &entry, true)
    }

    /// 保存完整文本。内存事务已应用但磁盘保存失败时，仍保留未保存的内存修改。
    ///
    /// # Errors
    /// 路径、身份、编码、文档版本或保存失败时返回错误。
    pub fn write(
        &self,
        buffers: &BufferStore,
        path: &Path,
        text: &str,
        confined: bool,
    ) -> Result<(), String> {
        self.audited(|| self.write_inner(buffers, path, text, confined))
    }

    /// 经共享工作区事务原子创建一个仍不存在的文本文件。
    ///
    /// 临时文件在同目录落盘，再用无覆盖提交；外部进程在最终核对后抢先创建目标时也会失败。
    ///
    /// # Errors
    /// 路径越界、目标或缓冲区已存在、审计或文件系统写入失败时返回错误。
    pub fn create_new_text(
        &self,
        buffers: &BufferStore,
        path: &Path,
        text: &str,
        confined: bool,
    ) -> Result<(), String> {
        self.create_new_resolved(buffers, self.resolve_write(path, confined)?, text)
    }

    /// 消费授权前解析并固定的目标，原子创建仍不存在的文本文件。
    ///
    /// 调用方以 `target.path()` 授权，并跨等待期持有该对象；本方法不重新解释路径。
    ///
    /// # Errors
    /// 工作区或祖先身份变化、目标已经存在、缓冲区冲突、审计或写入失败时返回错误。
    pub fn create_new_resolved(
        &self,
        buffers: &BufferStore,
        target: ResolvedTarget,
        text: &str,
    ) -> Result<(), String> {
        self.audited(|| {
            target.require_workspace(&self.root)?;
            target.revalidate()?;
            if target.is_dir() || target.exists() || buffers.find(target.path()).is_some() {
                return Err("Workspace creation target already exists".into());
            }
            let (_, parent_anchor) = self.anchor_parent(&target)?;
            self.checkpoint(&target)?;
            target.revalidate()?;
            parent_anchor
                .create_new(Self::target_name(&target)?, text.as_bytes())
                .map_err(|error| error.to_string())?;
            Ok(())
        })
    }

    fn write_inner(
        &self,
        buffers: &BufferStore,
        path: &Path,
        text: &str,
        confined: bool,
    ) -> Result<(), String> {
        self.write_resolved(buffers, self.resolve_write(path, confined)?, text)
    }

    fn write_resolved(
        &self,
        buffers: &BufferStore,
        target: ResolvedTarget,
        text: &str,
    ) -> Result<(), String> {
        target.require_workspace(&self.root)?;
        if target.is_dir() {
            return Err("Text modification points to a directory".into());
        }
        let (parent_guard, parent_anchor) = self.anchor_parent(&target)?;
        self.checkpoint(&target)?;
        target.revalidate()?;
        if let Some(buffer) = buffers.find(target.path()) {
            if buffer.is_read_only() {
                return Err("Unable to replace an open binary buffer".into());
            }
            let delta = DocumentChange::between(
                &buffer.to_string(),
                text,
                buffer.revision(),
                buffer.revision() + 1,
            );
            buffer
                .apply(delta.transaction)
                .map_err(|error| error.to_string())?;
            target.revalidate()?;
            // Windows 必须释放叶身份句柄；父目录锚点仍保持整个保存过程的祖先身份。
            parent_guard.revalidate()?;
            let name = Self::target_name(&target)?.to_os_string();
            drop(target);
            buffer
                .save_to_anchor(&parent_anchor, &name)
                .map_err(|error| error.to_string())
        } else {
            let mut document = if target.exists() {
                Document::open_from_anchor(&parent_anchor, Self::target_name(&target)?)
                    .map_err(|error| error.to_string())?
            } else {
                Document::from_text("")
            };
            if document.encoding() == TextEncoding::Binary {
                return Err("Text modification points to binary file".into());
            }
            let delta = DocumentChange::between(
                &document.to_string(),
                text,
                document.revision(),
                document.revision() + 1,
            );
            document
                .apply(delta.transaction)
                .map_err(|error| error.to_string())?;
            target.revalidate()?;
            parent_guard.revalidate()?;
            let name = Self::target_name(&target)?.to_os_string();
            drop(target);
            document
                .save_to_anchor(&parent_anchor, &name)
                .map_err(|error| error.to_string())
        }
    }

    /// 从受控检查点恢复文件；目标始终保持工作区约束。
    ///
    /// # Errors
    /// 备份越出检查点目录、目标越界或身份/文档检查失败时返回错误。
    pub fn restore(&self, buffers: &BufferStore, backup: &Path, path: &Path) -> Result<(), String> {
        self.audited(|| self.restore_inner(buffers, backup, path))
    }

    fn restore_inner(
        &self,
        buffers: &BufferStore,
        backup: &Path,
        path: &Path,
    ) -> Result<(), String> {
        let target = self.resolve_write(path, true)?;
        let backup = self.resolve_read(backup, false)?;
        let checkpoints = self.resolve_read(&self.checkpoints, false)?;
        if !backup.path().starts_with(checkpoints.path()) {
            return Err("Checkpoint is outside the checkpoint directory".into());
        }
        checkpoints.revalidate()?;
        self.copy_file(buffers, &backup, target)
    }

    /// 对打开文件应用内存事务，对关闭文件加载后原子保存；坐标必须来自相同旧文档。
    ///
    /// # Errors
    /// 路径、身份、事务或保存失败时返回错误。
    pub fn apply_transaction(
        &self,
        buffers: &BufferStore,
        path: &Path,
        transaction: Transaction,
        confined: bool,
    ) -> Result<(), String> {
        self.audited(|| self.apply_transaction_inner(buffers, path, transaction, confined))
    }

    fn apply_transaction_inner(
        &self,
        buffers: &BufferStore,
        path: &Path,
        transaction: Transaction,
        confined: bool,
    ) -> Result<(), String> {
        let target = self.resolve_read(path, confined)?;
        let (parent_guard, parent_anchor) = self.anchor_parent(&target)?;
        self.checkpoint(&target)?;
        target.revalidate()?;
        if let Some(buffer) = buffers.find(target.path()) {
            buffer.apply(transaction).map_err(|error| error.to_string())
        } else {
            let mut document =
                Document::open_from_anchor(&parent_anchor, Self::target_name(&target)?)
                    .map_err(|error| error.to_string())?;
            document
                .apply(transaction)
                .map_err(|error| error.to_string())?;
            target.revalidate()?;
            parent_guard.revalidate()?;
            let name = Self::target_name(&target)?.to_os_string();
            drop(target);
            document
                .save_to_anchor(&parent_anchor, &name)
                .map_err(|error| error.to_string())
        }
    }

    /// 顺序执行资源操作并维护缓冲区绑定；多个目标不构成跨文件原子事务。
    ///
    /// # Errors
    /// 越界、身份改变、循环、已存在的重命名目标或文件系统错误时失败。
    pub fn execute(
        &self,
        buffers: &BufferStore,
        intent: WorkspaceIntent,
        confined: bool,
    ) -> Result<(), String> {
        self.audited(|| self.execute_inner(buffers, intent, confined))
    }

    fn execute_inner(
        &self,
        buffers: &BufferStore,
        intent: WorkspaceIntent,
        confined: bool,
    ) -> Result<(), String> {
        match intent {
            WorkspaceIntent::Create { path, directory } => {
                let target = self.resolve_write(&path, confined)?;
                if target.exists() {
                    return Err("Target already exists".into());
                }
                let (_, parent_anchor) = self.anchor_parent(&target)?;
                target.revalidate()?;
                if directory {
                    parent_anchor
                        .create_dir(Self::target_name(&target)?)
                        .map_err(|error| error.to_string())?;
                } else {
                    parent_anchor
                        .create_new(Self::target_name(&target)?, b"")
                        .map_err(|error| error.to_string())?;
                }
            }
            WorkspaceIntent::Rename { old, new } => self.rename_entry(
                buffers,
                &self.resolve_entry(&old, confined)?,
                &self.resolve_entry(&new, confined)?,
            )?,
            WorkspaceIntent::Remove { paths, permanent } => {
                for path in paths {
                    self.remove_entry(buffers, &self.resolve_entry(&path, confined)?, permanent)?;
                }
            }
            WorkspaceIntent::Move { paths, destination } => {
                let destination = self.resolve_write(&destination, confined)?;
                for old in paths {
                    destination.revalidate()?;
                    let old = self.resolve_entry(&old, confined)?;
                    let new = destination
                        .path()
                        .join(old.original().file_name().ok_or("Missing file name")?);
                    self.rename_entry(buffers, &old, &self.resolve_entry(&new, confined)?)?;
                }
            }
            WorkspaceIntent::Copy { paths, destination } => {
                let destination = self.resolve_write(&destination, confined)?;
                // 预检逐项释放句柄；多资源请求不冻结整组文件内容或身份。
                for path in &paths {
                    let source = self.resolve_read(path, confined)?;
                    let target = destination
                        .path()
                        .join(source.original().file_name().ok_or("Missing file name")?);
                    if source.is_dir() && target.starts_with(source.path()) {
                        return Err("Unable to copy directory into itself".into());
                    }
                }
                destination.revalidate()?;
                DirectoryAnchor::ensure(destination.path()).map_err(|error| error.to_string())?;
                let destination = destination.materialized()?;
                for path in paths {
                    destination.revalidate()?;
                    let source = self.resolve_read(&path, confined)?;
                    let target = destination
                        .path()
                        .join(source.original().file_name().ok_or("Missing file name")?);
                    self.copy_tree(
                        buffers,
                        source,
                        self.resolve_write(&target, confined)?,
                        confined,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn rename_entry(
        &self,
        buffers: &BufferStore,
        old: &ResolvedEntry,
        new: &ResolvedEntry,
    ) -> Result<(), String> {
        old.require_workspace(&self.root)?;
        new.require_workspace(&self.root)?;
        if !old.exists() {
            return Err("Rename source does not exist".into());
        }
        if new.exists() {
            return Err("Rename target already exists".into());
        }
        old.revalidate()?;
        new.revalidate()?;
        let old_parent = old
            .path()
            .parent()
            .ok_or("Rename source parent is missing")?;
        let new_parent = new
            .path()
            .parent()
            .ok_or("Rename target parent is missing")?;
        let old_anchor = DirectoryAnchor::open(old_parent).map_err(|error| error.to_string())?;
        let new_anchor = DirectoryAnchor::open(new_parent).map_err(|error| error.to_string())?;
        old.revalidate()?;
        new.revalidate()?;
        let old_name = Self::entry_name(old)?.to_os_string();
        let new_name = Self::entry_name(new)?.to_os_string();
        let affected = self.affected(buffers, old);
        old.revalidate()?;
        new.revalidate()?;
        old_anchor
            .rename_to(&old_name, &new_anchor, &new_name)
            .map_err(|error| error.to_string())?;
        for (buffer, suffix, previous) in affected {
            let base = if old.is_link() {
                targets::normalize(new.original().to_owned())
            } else {
                new.path().to_owned()
            };
            let next = if suffix.as_os_str().is_empty() {
                base
            } else {
                base.join(suffix)
            };
            // 相对链接移入另一个目录后可能指向另一文件；已有文档仍绑定原目标。
            let next = if old.is_link() && targets::resolve(&next).ok().as_ref() != Some(&previous)
            {
                previous
            } else {
                next
            };
            buffer.relocate(Some(next));
        }
        Ok(())
    }

    fn remove_entry(
        &self,
        buffers: &BufferStore,
        entry: &ResolvedEntry,
        permanent: bool,
    ) -> Result<(), String> {
        entry.require_workspace(&self.root)?;
        if !entry.exists() {
            return Err("Removal source does not exist".into());
        }
        entry.revalidate()?;
        let affected = self.affected(buffers, entry);
        if !entry.is_link() && !entry.is_dir() {
            self.checkpoint(&self.resolve_read(entry.path(), false)?)?;
        }
        entry.revalidate()?;
        if permanent {
            let parent = entry
                .path()
                .parent()
                .ok_or("Removal source parent is missing")?;
            let parent_anchor = DirectoryAnchor::open(parent).map_err(|error| error.to_string())?;
            let name = Self::entry_name(entry)?.to_os_string();
            if entry.is_dir() && !entry.is_link() {
                // 递归删除仍由标准库逐项执行；父目录锚点至少固定了顶层目录项。
                fs::remove_dir_all(entry.path())
            } else {
                parent_anchor.remove(&name, entry.is_dir())
            }
            .map_err(|error| error.to_string())?;
        } else {
            // 平台回收站 API 只接受完整路径，无法继承已验证父目录句柄；拒绝旧路径副作用，
            // 避免祖先替换后把工作区外的对象移入回收站。
            return Err(
                "Non-permanent deletion is unavailable for anchored workspace entries".into(),
            );
        }
        for (buffer, _, previous) in affected {
            buffer.relocate(entry.is_link().then_some(previous));
        }
        Ok(())
    }

    /// 链接操作按逻辑路径找视图；普通文件/目录操作按实际目标找视图。
    fn affected(
        &self,
        buffers: &BufferStore,
        entry: &ResolvedEntry,
    ) -> Vec<(crate::BufferHandle, PathBuf, PathBuf)> {
        buffers
            .iter()
            .filter_map(|buffer| {
                let input = buffer.path()?;
                let input = std::path::absolute(if input.is_absolute() {
                    input
                } else {
                    self.root.input().join(input)
                })
                .ok()?;
                let actual = targets::resolve(&input).ok()?;
                let suffix = if entry.is_link() {
                    input.ancestors().find_map(|ancestor| {
                        (ancestor.file_name() == entry.path().file_name()
                            && targets::resolve(ancestor.parent()?).ok()?.as_path()
                                == entry.path().parent()?)
                        .then(|| input.strip_prefix(ancestor).ok().map(Path::to_owned))
                        .flatten()
                    })?
                } else {
                    actual.strip_prefix(entry.path()).ok()?.to_path_buf()
                };
                Some((buffer.clone(), suffix, actual))
            })
            .collect()
    }

    fn copy_tree(
        &self,
        buffers: &BufferStore,
        source: ResolvedTarget,
        target: ResolvedTarget,
        confined: bool,
    ) -> Result<(), String> {
        source.require_workspace(&self.root)?;
        target.require_workspace(&self.root)?;
        enum Step {
            Root(Box<(ResolvedTarget, ResolvedTarget)>),
            Enter(PathBuf, PathBuf),
            Leave,
        }
        let mut pending = vec![Step::Root(Box::new((source, target)))];
        let mut ancestors: Vec<(ResolvedTarget, ResolvedTarget)> = Vec::new();
        while let Some(step) = pending.pop() {
            let (source, target) = match step {
                Step::Leave => {
                    ancestors.pop();
                    continue;
                }
                Step::Root(pair) => *pair,
                Step::Enter(source, target) => (
                    self.resolve_read(&source, confined)?,
                    self.resolve_write(&target, confined)?,
                ),
            };
            for (source, target) in &ancestors {
                source.revalidate()?;
                target.revalidate()?;
            }
            source.revalidate()?;
            target.revalidate()?;
            if !source.is_dir() {
                self.copy_file(buffers, &source, target)?;
                continue;
            }
            if ancestors
                .iter()
                .any(|(ancestor, _)| source.same_object(ancestor))
            {
                return Err("Recursive directory link found while copying".into());
            }
            if target.path().starts_with(source.path()) {
                return Err("Unable to copy directory into itself".into());
            }
            let (_, parent_anchor) = self.anchor_parent(&target)?;
            source.revalidate()?;
            target.revalidate()?;
            parent_anchor
                .create_dir(Self::target_name(&target)?)
                .map_err(|error| error.to_string())?;
            source.revalidate()?;
            let output = target.materialized()?;
            ancestors.push((source.clone(), output.clone()));
            pending.push(Step::Leave);
            let source_anchor =
                DirectoryAnchor::open(source.path()).map_err(|error| error.to_string())?;
            for name in source_anchor.entries().map_err(|error| error.to_string())? {
                output.revalidate()?;
                // 排队只保留名字；真正进入该项时再构造证明，句柄数量随深度而非目录宽度增长。
                pending.push(Step::Enter(
                    source.path().join(&name),
                    output.path().join(name),
                ));
            }
        }
        Ok(())
    }

    fn copy_file(
        &self,
        buffers: &BufferStore,
        source: &ResolvedTarget,
        target: ResolvedTarget,
    ) -> Result<(), String> {
        source.require_workspace(&self.root)?;
        target.require_workspace(&self.root)?;
        source.revalidate()?;
        target.revalidate()?;
        if source.is_dir() || target.is_dir() {
            return Err("File copy requires file targets".into());
        }
        if source.same_object(&target) {
            return Err("Source and target refer to the same file".into());
        }
        let (parent_guard, parent_anchor) = self.anchor_parent(&target)?;
        let (_, source_anchor) = self.anchor_parent(source)?;
        if buffers.find(target.path()).is_some() {
            source.revalidate()?;
            let document = Document::open_from_anchor(&source_anchor, Self::target_name(source)?)
                .map_err(|error| error.to_string())?;
            if document.encoding() == TextEncoding::Binary {
                return Err("Binary copy cannot overwrite an already open text buffer".into());
            }
            return self.write_resolved(buffers, target, &document.to_string());
        }
        self.checkpoint(&target)?;
        source.revalidate()?;
        target.revalidate()?;
        let contents = source_anchor
            .read(Self::target_name(source)?)
            .map_err(|error| error.to_string())?;
        target.revalidate()?;
        parent_guard.revalidate()?;
        let name = Self::target_name(&target)?.to_os_string();
        drop(target);
        parent_anchor
            .atomic_replace(&name, &contents)
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

/// 只检查路径范围；实际文件操作应使用协调器返回的已解析目标。
///
/// # Errors
/// 工作区不可用、路径越界、循环或身份读取失败时返回错误。
pub fn ensure_workspace_path(workspace: &Path, path: &Path) -> Result<(), String> {
    ResolvedTarget::new(&Root::new(workspace)?, path, true, false)?.revalidate()
}
