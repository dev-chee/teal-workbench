//! 共享文档、视图选择区与后台文件操作的交接。
//!
//! `BufferHandle` 使用单线程共享所有权，不能直接移交后台线程；后台任务接收完整文档副本。
//! 普通克隆保留同一视图选择区，`fork_view` 才创建独立选择区。

use crate::{
    Document, LineEnding, SaveError, SelectionSet, TextEdit, TextEncoding, Transaction,
    TransactionError,
};
use ropey::Rope;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ops::Range,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_BUFFER: AtomicU64 = AtomicU64::new(1);

/// 独立于面板标识的进程内稳定缓冲区标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BufferId(u64);

/// 一次提交的文档修订及最小 UTF-8 差异。
#[derive(Clone, Debug)]
pub struct DocumentChange {
    /// 用于协议坐标转换和恢复的旧文本快照。
    pub old_text: String,

    /// 已提交的文本快照。
    pub new_text: String,

    /// 前一文档修订。
    pub old_revision: u64,

    /// 已提交的文档修订。
    pub revision: u64,

    /// 旧快照中被替换的 UTF-8 字节范围。
    pub old_byte_range: Range<usize>,

    /// 替换的 UTF-8 文本。
    pub inserted: String,

    /// 以旧 Unicode 字符坐标表示的恢复事务。
    pub transaction: Transaction,
}

impl DocumentChange {
    /// 构建两个快照之间保持 Unicode 边界的最小差异。
    ///
    /// 共同前缀和后缀均按字符边界计算，但 `old_byte_range` 使用字节，`transaction` 使用字符。
    /// 传入修订号只作为元数据记录；本函数不访问缓冲区，也不验证修订是否连续。
    pub fn between(old: &str, new: &str, old_revision: u64, revision: u64) -> Self {
        let prefix = old
            .chars()
            .zip(new.chars())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        // 只比较尚未被公共前缀消费的部分，避免完全相等或短文本时前后缀交叠。
        let suffix = old[prefix..]
            .chars()
            .rev()
            .zip(new[prefix..].chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        let old_end = old.len() - suffix;
        let inserted = new[prefix..new.len() - suffix].to_owned();
        let transaction = Transaction::new(vec![TextEdit {
            range: old[..prefix].chars().count()..old[..old_end].chars().count(),
            insert: inserted.clone(),
        }])
        .expect("Individual increments do not overlap");
        Self {
            old_text: old.to_owned(),
            new_text: new.to_owned(),
            old_revision,
            revision,
            old_byte_range: prefix..old_end,
            inserted,
            transaction,
        }
    }
}

struct Buffer {
    /// 文本、编码与撤销历史的唯一共享状态。
    document: Document,
    /// 由应用一次性消费并分发给日志及协议的提交记录。
    changes: Vec<DocumentChange>,
    /// 文档副本正在后台处理时阻止新的文本事务。
    busy: bool,
    /// 弱引用避免缓冲区反向保留已经关闭的视图。
    views: Vec<Weak<RefCell<SelectionSet>>>,
}

/// 共享的单事件循环缓冲区句柄；快照归调用方所有，不泄露借用。
///
/// 普通克隆共享文本、历史和同一份选择区；创建拆分视图应调用 [`Self::fork_view`]。
/// 句柄内部使用 `Rc<RefCell<_>>`，不能跨线程移动；后台 I/O 使用 [`Self::begin_io`] 返回的文档副本。
#[derive(Clone)]
pub struct BufferHandle {
    id: BufferId,
    inner: Rc<RefCell<Buffer>>,
    selections: Rc<RefCell<SelectionSet>>,
}

impl BufferHandle {
    /// 创建独立缓冲区句柄。
    pub fn new(document: Document) -> Self {
        let selections = Rc::new(RefCell::new(document.selections().clone()));
        Self {
            id: BufferId(NEXT_BUFFER.fetch_add(1, Ordering::Relaxed)),
            inner: Rc::new(RefCell::new(Buffer {
                document,
                changes: Vec::new(),
                busy: false,
                views: vec![Rc::downgrade(&selections)],
            })),
            selections,
        }
    }

    /// 返回稳定缓冲区标识。
    pub const fn id(&self) -> BufferId {
        self.id
    }

    /// 返回共享存储的轻量 Rope 快照。
    pub fn text(&self) -> Rope {
        self.inner.borrow().document.text().clone()
    }

    /// 返回拥有所有权的后备路径。
    pub fn path(&self) -> Option<PathBuf> {
        self.inner.borrow().document.path().map(Path::to_path_buf)
    }

    /// 返回当前选择区。
    pub fn selections(&self) -> SelectionSet {
        self.selections.borrow().clone()
    }

    /// 替换规范化选择区。
    pub fn set_selections(&self, mut selections: SelectionSet) {
        selections.clamp(self.inner.borrow().document.text().len_chars());
        *self.selections.borrow_mut() = selections;
    }

    /// 共享文本和历史，同时为新视图创建独立选择区。
    ///
    /// 新视图以当前选择区为起点，此后移动光标不影响原视图。编辑仍会更新共享文本，
    /// 并把全部存活视图的选择端点限制到新文档范围；这里不会按编辑距离平移其他视图的光标。
    #[must_use]
    pub fn fork_view(&self) -> Self {
        let selections = Rc::new(RefCell::new(self.selections()));
        self.inner
            .borrow_mut()
            .views
            .push(Rc::downgrade(&selections));
        Self {
            id: self.id,
            inner: self.inner.clone(),
            selections,
        }
    }

    /// 返回当前修订。
    pub fn revision(&self) -> u64 {
        self.inner.borrow().document.revision()
    }

    /// 返回文档是否有未保存修改。
    pub fn is_dirty(&self) -> bool {
        self.inner.borrow().document.is_dirty()
    }

    /// 返回是否禁止编辑。
    pub fn is_read_only(&self) -> bool {
        self.inner.borrow().document.is_read_only()
    }

    /// 返回保留的编码。
    pub fn encoding(&self) -> TextEncoding {
        self.inner.borrow().document.encoding()
    }

    /// 返回保留的行尾。
    pub fn line_ending(&self) -> LineEnding {
        self.inner.borrow().document.line_ending()
    }

    /// 应用事务并将提交的变化入队。
    ///
    /// 后台 I/O 保留期间返回 [`TransactionError::BufferBusy`]。插入文本先归一化换行，
    /// 只有文档修订实际改变时才记录变化；恢复日志使用原始已提交事务，语法与 LSP 使用字节差异。
    pub fn apply(&self, transaction: Transaction) -> Result<(), TransactionError> {
        let mut buffer = self.inner.borrow_mut();
        if buffer.busy {
            return Err(TransactionError::BufferBusy);
        }

        let transaction = transaction.normalized()?;
        let old = buffer.document.to_string();
        let revision = buffer.document.revision();

        buffer.document.apply(transaction.clone())?;
        buffer.clamp_views();

        // 保留精确的已提交事务，供恢复与同步使用。
        if revision != buffer.document.revision() {
            let mut change = DocumentChange::between(
                &old,
                &buffer.document.to_string(),
                revision,
                buffer.document.revision(),
            );
            change.transaction = transaction;
            buffer.changes.push(change);
        }

        Ok(())
    }

    /// 撤销修订并将最小变化入队。
    pub fn undo(&self) -> Result<bool, TransactionError> {
        self.history(false, 0)
    }

    /// 重做分支并将最小变化入队。
    pub fn redo(&self, branch: usize) -> Result<bool, TransactionError> {
        self.history(true, branch)
    }

    fn history(&self, redo: bool, branch: usize) -> Result<bool, TransactionError> {
        let mut buffer = self.inner.borrow_mut();
        if buffer.busy {
            return Err(TransactionError::BufferBusy);
        }
        let old = buffer.document.to_string();
        let revision = buffer.document.revision();
        let changed = if redo {
            buffer.document.redo(branch)?
        } else {
            buffer.document.undo()?
        };
        buffer.clamp_views();
        if changed {
            let change = DocumentChange::between(
                &old,
                &buffer.document.to_string(),
                revision,
                buffer.document.revision(),
            );
            buffer.changes.push(change);
        }
        Ok(changed)
    }

    /// 一次性排空已提交变化供应用消费者处理。
    ///
    /// 这是共享缓冲区级别的消费操作，任一视图调用后其他视图就不会再次取得相同变化。
    /// 应用应统一分发给恢复日志、LSP 和其他消费者，不能让各视图分别假定自己拥有完整队列。
    pub fn take_changes(&self) -> Vec<DocumentChange> {
        std::mem::take(&mut self.inner.borrow_mut().changes)
    }

    /// 原子保存当前文档。
    pub fn save(&self) -> Result<(), SaveError> {
        self.inner.borrow_mut().document.save()
    }

    /// 原子保存到新路径。
    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<(), SaveError> {
        self.inner.borrow_mut().document.save_to(path)
    }

    /// 在协调的资源操作后更新后备标识，保留编辑。
    pub fn relocate(&self, path: Option<PathBuf>) {
        self.inner.borrow_mut().document.relocate(path);
    }

    /// 返回后台文件 I/O 是否已保留此缓冲区。
    pub fn is_busy(&self) -> bool {
        self.inner.borrow().busy
    }

    /// 保留编辑并返回供阻塞工作线程使用的文档快照。
    ///
    /// 已被保留时返回 [`TransactionError::BufferBusy`]。成功后，应用事务及撤销/重做暂时被拒绝，
    /// 但视图选择区仍可变化。调用方必须在成功、失败和取消路径都调用 [`Self::finish_io`] 归还文档。
    pub fn begin_io(&self) -> Result<Document, TransactionError> {
        let mut buffer = self.inner.borrow_mut();
        if buffer.busy {
            return Err(TransactionError::BufferBusy);
        }

        buffer.busy = true;
        let mut document = buffer.document.clone();
        document.set_selections(self.selections());
        Ok(document)
    }

    /// 提交工作线程文档并释放保留，同时保留当前选择区。
    ///
    /// 文档副本中的文本、编码、历史和修订替换共享状态；视图使用等待期间的当前选择区并重新限界。
    /// 本函数不验证结果归属和修订，也不回滚磁盘；调用方必须先完成作业身份检查，失败时可归还原副本。
    pub fn finish_io(&self, document: Document, changes: Vec<DocumentChange>) {
        let mut buffer = self.inner.borrow_mut();

        // 视图保留当前光标，仅按返回文档的范围限制光标位置。
        buffer.document = document;
        buffer.clamp_views();
        buffer.changes.extend(changes);
        buffer.busy = false;
    }

    /// 复制完整文档以执行协调持久化。
    pub fn snapshot_document(&self) -> Document {
        let mut document = self.inner.borrow().document.clone();
        document.set_selections(self.selections());
        document
    }
}

impl Buffer {
    fn clamp_views(&mut self) {
        let len = self.document.text().len_chars();
        // 顺便移除已析构视图；仅限制端点，不把其他视图的选择区当作本次编辑来源。
        self.views.retain(|view| {
            if let Some(view) = view.upgrade() {
                view.borrow_mut().clamp(len);
                true
            } else {
                false
            }
        });
    }
}

impl std::fmt::Display for BufferHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.borrow().document.fmt(f)
    }
}

/// 工作区拥有的文档句柄；每个具体路径对应一个缓冲区。
#[derive(Default)]
pub struct BufferStore {
    buffers: BTreeMap<BufferId, BufferHandle>,
}

impl BufferStore {
    /// 插入文档或复用已有的命名缓冲区。
    ///
    /// 规范路径已存在时返回原句柄，并丢弃传入文档，不覆盖已有未保存内容。
    /// 未命名文档总是获得新的身份；规范化失败时按保留路径比较。
    pub fn insert(&mut self, document: Document) -> BufferHandle {
        if let Some(path) = document.path()
            && let Some(existing) = self.find(path)
        {
            return existing;
        }
        let handle = BufferHandle::new(document);
        self.buffers.insert(handle.id(), handle.clone());
        handle
    }

    /// 接管独立编辑器缓冲区。
    pub fn adopt(&mut self, handle: BufferHandle) -> BufferHandle {
        if let Some(path) = handle.path()
            && let Some(existing) = self.find(&path)
        {
            return existing;
        }
        self.buffers.insert(handle.id(), handle.clone());
        handle
    }

    /// 按规范路径标识查找缓冲区。
    pub fn find(&self, path: &Path) -> Option<BufferHandle> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
        self.buffers
            .values()
            .find(|buffer| {
                buffer
                    .path()
                    .is_some_and(|p| p.canonicalize().unwrap_or(p) == path)
            })
            .cloned()
    }

    /// 按稳定标识返回句柄。
    pub fn get(&self, id: BufferId) -> Option<BufferHandle> {
        self.buffers.get(&id).cloned()
    }

    /// 移除存储对指定缓冲区的所有权并返回句柄。
    ///
    /// 是否已关闭最后一个视图由调用方判断，本方法不会统计视图数量。其他已有句柄仍然有效，
    /// 删除存储项本身既不保存文档，也不取消后台工作。
    pub fn remove(&mut self, id: BufferId) -> Option<BufferHandle> {
        self.buffers.remove(&id)
    }

    /// 遍历工作区缓冲区句柄。
    pub fn iter(&self) -> impl Iterator<Item = &BufferHandle> {
        self.buffers.values()
    }
}
#[cfg(test)]
mod tests;
