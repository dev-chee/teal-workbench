//! 按需加载的文件树、光标上下文与资源操作。
//!
//! 树模型区分根目录、展开状态、可见行和多选择；选中祖先会覆盖其后代操作目标。
//! 规范祖先路径用于阻止目录循环，预览经 Document 解码以保持与编辑器一致。

use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use teal_workbench_core::{Document, TextEncoding};

/// 目录条目的排序方式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileSort {
    /// 目录优先，然后按不区分大小写的名称排序。
    #[default]
    Name,

    /// 在目录/文件组内按最大文件优先。
    Size,

    /// 最近修改的优先。
    Modified,
}

/// 浏览器显示的一个文件系统条目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    /// 提供给加载器的绝对或工作区相对路径。
    pub path: PathBuf,

    /// 最终路径组件。
    pub name: String,

    /// 条目是否为目录。
    pub is_dir: bool,

    /// 普通文件的字节长度。
    pub size: u64,

    /// 文件系统提供时的最后修改时间。
    pub modified: Option<SystemTime>,

    /// 平台隐藏状态或前导点约定。
    pub hidden: bool,
}

/// 惰性加载目录树中的一个扁平可见行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTreeEntry {
    /// 该行的文件系统元数据。
    pub entry: FileEntry,

    /// 从零开始的缩进深度，树根为零。
    pub depth: usize,

    /// 目录行当前是否展开；文件始终为 false。
    pub expanded: bool,
}

/// 悬停条目的预览结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preview {
    /// 带有从扩展名推断语言标识的 UTF 文本预览。
    Text {
        /// UTF-8 预览内容。
        content: String,

        /// 从扩展名推导的语言 ID。
        language: String,
    },

    /// 应使用 Markdown 渲染器的 Markdown 源码。
    Markdown(String),

    /// 目录子项名称。
    Directory(Vec<String>),

    /// 二进制元数据与十六进制预览。
    Binary(String),

    /// 因大小或不支持的类型而有意省略预览。
    Unavailable(String),
}

/// 浏览器文件系统操作失败。
#[derive(Debug, Error)]
pub enum ExplorerError {
    /// 文件系统访问失败。
    #[error("file browser operation failed: {0}")]
    Io(#[from] std::io::Error),

    /// 可恢复地删除到平台回收站失败。
    #[error("unable to move path to recycle bin: {0}")]
    Trash(#[from] trash::Error),

    /// 条目没有有效的最终组件。
    #[error("path has no displayable file name")]
    InvalidName,

    /// 展开链接会再次访问祖先目录。
    #[error("refusing to expand directory cycle at {0}")]
    DirectoryCycle(PathBuf),

    /// 修改树根前必须先提升树根。
    #[error("elevate the tree root before modifying its current root entry")]
    RootMutation,
}

/// 有状态、惰性加载的树形文件浏览器模型。
#[derive(Clone, Debug)]
pub struct FileExplorer {
    root: FileEntry,
    children: HashMap<PathBuf, Vec<FileEntry>>,
    expanded: BTreeSet<PathBuf>,
    visible: Vec<FileTreeEntry>,
    cursor: usize,
    selection_anchor: Option<usize>,
    selected: BTreeSet<PathBuf>,
    show_hidden: bool,
    sort: FileSort,
}

impl FileExplorer {
    /// 将目录作为已展开的树根打开。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ExplorerError> {
        let root_path = canonical_or_original(path.as_ref());
        let root = entry_for_path(&root_path)?;
        if !root.is_dir {
            return Err(not_a_directory(&root_path));
        }
        let mut explorer = Self {
            root,
            children: HashMap::new(),
            expanded: BTreeSet::new(),
            visible: Vec::new(),
            cursor: 0,
            selection_anchor: None,
            selected: BTreeSet::new(),
            show_hidden: false,
            sort: FileSort::Name,
        };
        explorer.expanded.insert(root_path.clone());
        let children = explorer.read_children(&root_path)?;
        explorer.children.insert(root_path, children);
        explorer.rebuild_visible();
        Ok(explorer)
    }

    /// 恢复树，但不扫描未展开的目录。
    pub fn restore(
        root: impl AsRef<Path>,
        expanded: &[PathBuf],
        cursor: Option<&Path>,
        show_hidden: bool,
        sort: FileSort,
    ) -> Result<Self, ExplorerError> {
        let requested_root = root.as_ref().to_path_buf();
        let mut explorer = Self::open(&requested_root)?;
        explorer.show_hidden = show_hidden;
        explorer.sort = sort;
        explorer.children.clear();
        explorer.expanded.clear();
        let root_path = explorer.root.path.clone();
        let root_children = explorer.read_children(&root_path)?;
        explorer.children.insert(root_path.clone(), root_children);

        let mut requested = expanded
            .iter()
            .map(|path| normalize_restored_path(path, &requested_root, &explorer.root.path))
            .filter(|path| path.starts_with(&explorer.root.path))
            .collect::<Vec<_>>();
        requested.sort_by_key(|path| path.components().count());
        requested.dedup();
        for path in requested {
            if !path.is_dir() || explorer.expansion_would_cycle(&path) {
                continue;
            }
            if let Ok(children) = explorer.read_children(&path) {
                explorer.children.insert(path.clone(), children);
                explorer.expanded.insert(path);
            }
        }
        explorer.rebuild_visible();
        if let Some(path) = cursor {
            let path = normalize_restored_path(path, &requested_root, &explorer.root.path);
            explorer.cursor = explorer.closest_visible_index(&path);
        }
        Ok(explorer)
    }

    /// 在阻塞工作线程上加载树根，使终端输入可以继续。
    pub async fn open_async(path: PathBuf) -> Result<Self, ExplorerError> {
        observed_io(move || Self::open(path))
            .await
            .map_err(|error| ExplorerError::Io(std::io::Error::other(error.to_string())))?
    }

    /// 返回当前树根。
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root.path
    }

    /// 返回用于创建和相对目标路径的目录。
    #[must_use]
    pub fn operation_directory(&self) -> &Path {
        self.hovered().map_or(self.root.path.as_path(), |row| {
            if row.entry.is_dir {
                row.entry.path.as_path()
            } else {
                row.entry.path.parent().unwrap_or(self.root.path.as_path())
            }
        })
    }

    /// 返回扁平化的可见树行。
    #[must_use]
    pub fn visible_entries(&self) -> &[FileTreeEntry] {
        &self.visible
    }

    /// 返回悬停的可见树行。
    #[must_use]
    pub fn hovered(&self) -> Option<&FileTreeEntry> {
        self.visible.get(self.cursor)
    }

    /// 返回当前从零开始的可见行。
    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    /// 按确定顺序返回已展开目录路径。
    #[must_use]
    pub fn expanded_paths(&self) -> Vec<PathBuf> {
        self.expanded.iter().cloned().collect()
    }

    /// 返回是否显示隐藏条目。
    #[must_use]
    pub const fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// 返回当前同级排序方式。
    #[must_use]
    pub const fn sort(&self) -> FileSort {
        self.sort
    }

    /// 将光标直接移到可见行而不更改选择。
    pub fn set_cursor(&mut self, index: usize) {
        self.cursor = index.min(self.visible.len().saturating_sub(1));
    }

    /// 用一个可见行替换选择集合并记录范围锚点。
    pub fn select_only(&mut self, index: usize) {
        if self.visible.is_empty() {
            self.cursor = 0;
            self.selection_anchor = None;
            self.selected.clear();
            return;
        }
        self.set_cursor(index);
        self.selected.clear();
        self.selected
            .insert(self.visible[self.cursor].entry.path.clone());
        self.selection_anchor = Some(self.cursor);
    }

    /// 切换选择集合中的一个可见行并将其设为范围锚点。
    pub fn toggle_at(&mut self, index: usize) {
        if self.visible.is_empty() {
            return;
        }
        self.set_cursor(index);
        self.toggle_selection();
        self.selection_anchor = Some(self.cursor);
    }

    /// 选择从最近锚点到 `index` 的连续可见范围。
    pub fn select_range(&mut self, index: usize) {
        if self.visible.is_empty() {
            return;
        }
        let anchor = self.selection_anchor.unwrap_or(self.cursor);
        self.set_cursor(index);
        let start = anchor.min(self.cursor);
        let end = anchor.max(self.cursor);
        self.selected.clear();
        self.selected.extend(
            self.visible[start..=end]
                .iter()
                .map(|row| row.entry.path.clone()),
        );
        self.selection_anchor = Some(anchor);
    }

    /// 返回可见行是否属于显式选择集合。
    #[must_use]
    pub fn is_selected(&self, index: usize) -> bool {
        self.visible
            .get(index)
            .is_some_and(|row| self.selected.contains(&row.entry.path))
    }

    /// 返回规范化后的选中路径；选择为空时返回悬停路径。
    #[must_use]
    pub fn selected_or_hovered(&self) -> Vec<PathBuf> {
        let paths = if self.selected.is_empty() {
            self.hovered()
                .map(|row| vec![row.entry.path.clone()])
                .unwrap_or_default()
        } else {
            self.visible
                .iter()
                .filter(|row| self.selected.contains(&row.entry.path))
                .map(|row| row.entry.path.clone())
                .collect()
        };
        normalize_operation_paths(paths)
    }

    /// 按有符号可见行数移动光标。
    pub fn move_cursor(&mut self, delta: isize) {
        if self.visible.is_empty() {
            self.cursor = 0;
        } else {
            self.cursor = self
                .cursor
                .saturating_add_signed(delta)
                .min(self.visible.len() - 1);
        }
    }

    /// 切换悬停行的选择状态。
    pub fn toggle_selection(&mut self) {
        let Some(path) = self.hovered().map(|row| row.entry.path.clone()) else {
            return;
        };
        if !self.selected.remove(&path) {
            self.selected.insert(path);
        }
        self.selection_anchor = Some(self.cursor);
    }

    /// 显示或隐藏平台隐藏和前导点条目。
    pub fn set_show_hidden(&mut self, show: bool) -> Result<(), ExplorerError> {
        self.show_hidden = show;
        self.refresh()
    }

    /// 更改同级排序方式而不重新读取元数据。
    pub fn set_sort(&mut self, sort: FileSort) {
        self.sort = sort;
        for entries in self.children.values_mut() {
            sort_entries(entries, sort);
        }
        self.rebuild_visible_preserving_cursor();
    }

    /// 重新读取树根和每个已展开目录，同时保留有效视图状态。
    pub fn refresh(&mut self) -> Result<(), ExplorerError> {
        let hovered = self.hovered().map(|row| row.entry.path.clone());
        self.root = entry_for_path(&self.root.path)?;
        if !self.root.is_dir {
            return Err(not_a_directory(&self.root.path));
        }

        let root_path = self.root.path.clone();
        let root_children = self.read_children(&root_path)?;
        self.children.insert(root_path.clone(), root_children);
        let mut expanded = self.expanded.iter().cloned().collect::<Vec<_>>();
        expanded.sort_by_key(|path| path.components().count());
        for path in expanded {
            if !path.starts_with(&root_path) || !path.is_dir() || self.expansion_would_cycle(&path)
            {
                self.expanded.remove(&path);
                self.children.remove(&path);
                continue;
            }
            if path == root_path {
                continue;
            }
            if let Ok(entries) = self.read_children(&path) {
                self.children.insert(path, entries);
            } else {
                self.expanded.remove(&path);
                self.children.remove(&path);
            }
        }
        self.children.retain(|path, _| path.starts_with(&root_path));
        self.rebuild_visible();
        self.cursor = hovered
            .as_deref()
            .map_or(0, |path| self.closest_visible_index(path));
        self.retain_visible_selection();
        Ok(())
    }

    /// 切换悬停目录，或返回要打开的悬停文件。
    pub fn activate_hovered(&mut self) -> Result<Option<PathBuf>, ExplorerError> {
        let Some(row) = self.hovered().cloned() else {
            return Ok(None);
        };
        if row.entry.is_dir {
            self.toggle_expansion(&row.entry.path)?;
            Ok(None)
        } else {
            Ok(Some(row.entry.path))
        }
    }

    /// 执行工作台风格的向右导航，并在应打开文件时返回路径。
    pub fn navigate_right(&mut self) -> Result<Option<PathBuf>, ExplorerError> {
        let Some(row) = self.hovered().cloned() else {
            return Ok(None);
        };
        if !row.entry.is_dir {
            return Ok(Some(row.entry.path));
        }
        if !row.expanded {
            self.expand(&row.entry.path)?;
        } else if self
            .visible
            .get(self.cursor + 1)
            .is_some_and(|child| child.depth == row.depth + 1)
        {
            self.cursor += 1;
        }
        Ok(None)
    }

    /// 执行工作台风格的向左导航，并报告树根是否改变。
    pub fn navigate_left(&mut self) -> Result<bool, ExplorerError> {
        let Some(row) = self.hovered().cloned() else {
            return Ok(false);
        };
        if row.entry.is_dir && row.expanded {
            self.collapse(&row.entry.path);
            return Ok(false);
        }
        if row.entry.path == self.root.path {
            return self.elevate_root();
        }
        if let Some(parent) = row.entry.path.parent()
            && let Some(index) = self
                .visible
                .iter()
                .position(|candidate| candidate.entry.path == parent)
        {
            self.cursor = index;
        }
        Ok(false)
    }

    /// 切换一个可见行上的目录。
    pub fn toggle_expansion_at(&mut self, index: usize) -> Result<(), ExplorerError> {
        let Some(row) = self.visible.get(index).cloned() else {
            return Ok(());
        };
        if row.entry.is_dir {
            self.set_cursor(index);
            self.toggle_expansion(&row.entry.path)?;
        }
        Ok(())
    }

    /// 将树根提升一级，并使原树根保持在光标下。
    pub fn elevate_root(&mut self) -> Result<bool, ExplorerError> {
        let old_root = self.root.path.clone();
        let Some(parent) = old_root.parent().map(Path::to_path_buf) else {
            return Ok(false);
        };
        let root = entry_for_path(&parent)?;
        let children = self.read_children(&parent)?;
        self.root = root;
        self.children.insert(parent.clone(), children);
        self.expanded.insert(parent);
        self.rebuild_visible();
        self.cursor = self
            .visible
            .iter()
            .position(|row| row.entry.path == old_root)
            .unwrap_or(0);
        self.retain_visible_selection();
        Ok(true)
    }

    /// 在光标上下文目录中创建空文件。
    pub fn create_file(&mut self, name: &str) -> Result<PathBuf, ExplorerError> {
        let directory = self.operation_directory().to_path_buf();
        let path = directory.join(name);
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        self.expanded.insert(directory);
        self.refresh()?;
        self.select_path(&path);
        Ok(path)
    }

    /// 在光标上下文目录中创建目录。
    pub fn create_directory(&mut self, name: &str) -> Result<PathBuf, ExplorerError> {
        let directory = self.operation_directory().to_path_buf();
        let path = directory.join(name);
        fs::create_dir(&path)?;
        self.expanded.insert(directory);
        self.refresh()?;
        self.select_path(&path);
        Ok(path)
    }

    /// 在父目录内重命名一个路径。
    pub fn rename(&mut self, path: &Path, new_name: &str) -> Result<PathBuf, ExplorerError> {
        self.reject_root_mutation([path])?;
        let target = path
            .parent()
            .ok_or(ExplorerError::InvalidName)?
            .join(new_name);
        fs::rename(path, &target)?;
        self.remap_prefix(path, &target);
        self.refresh()?;
        self.select_path(&target);
        Ok(target)
    }

    /// 将文件或目录复制到目标目录。
    pub fn copy_to(&mut self, paths: &[PathBuf], destination: &Path) -> Result<(), ExplorerError> {
        for path in normalize_operation_paths(paths.to_vec()) {
            let name = path.file_name().ok_or(ExplorerError::InvalidName)?;
            let target = destination.join(name);
            if path.is_dir() {
                copy_directory(&path, &target)?;
            } else {
                fs::copy(path, target)?;
            }
        }
        self.refresh()
    }

    /// 将文件或目录移动到目标目录。
    pub fn move_to(&mut self, paths: &[PathBuf], destination: &Path) -> Result<(), ExplorerError> {
        let paths = normalize_operation_paths(paths.to_vec());
        self.reject_root_mutation(paths.iter().map(PathBuf::as_path))?;

        let mut moved = Vec::with_capacity(paths.len());
        for path in paths {
            let name = path.file_name().ok_or(ExplorerError::InvalidName)?;
            let target = destination.join(name);
            fs::rename(&path, &target)?;
            moved.push((path, target));
        }

        for (source, target) in &moved {
            self.remap_prefix(source, target);
        }
        self.selected.clear();
        self.refresh()?;
        if let Some((_, target)) = moved.first() {
            self.select_path(target);
        }
        Ok(())
    }

    /// 将路径移入操作系统回收站。
    pub fn trash(&mut self, paths: &[PathBuf]) -> Result<(), ExplorerError> {
        let paths = normalize_operation_paths(paths.to_vec());
        self.reject_root_mutation(paths.iter().map(PathBuf::as_path))?;
        for path in paths {
            trash::delete(path)?;
        }
        self.selected.clear();
        self.refresh()
    }

    /// 永久删除显式路径；调用方必须执行确认。
    pub fn delete_permanently(&mut self, paths: &[PathBuf]) -> Result<(), ExplorerError> {
        let paths = normalize_operation_paths(paths.to_vec());
        self.reject_root_mutation(paths.iter().map(PathBuf::as_path))?;
        for path in paths {
            if path.is_dir() {
                fs::remove_dir_all(path)?;
            } else {
                fs::remove_file(path)?;
            }
        }
        self.selected.clear();
        self.refresh()
    }

    /// 为路径构建大小受限的预览。
    pub fn preview(path: &Path, byte_limit: usize) -> Result<Preview, ExplorerError> {
        if path.is_dir() {
            let names = fs::read_dir(path)?
                .take(200)
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            return Ok(Preview::Directory(names));
        }

        let metadata = fs::metadata(path)?;
        if metadata.len() > u64::try_from(byte_limit).unwrap_or(u64::MAX) {
            return Ok(Preview::Unavailable(format!(
                "{} bytes; preview capped at {byte_limit}",
                metadata.len()
            )));
        }

        // 使用 Document 解码，使预览与编辑器的编码检测一致。
        let document = Document::open(path)?;
        if document.encoding() == TextEncoding::Binary {
            return Ok(Preview::Binary(document.to_string()));
        }

        let content = document.to_string();
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if matches!(extension.to_ascii_lowercase().as_str(), "md" | "markdown") {
            Ok(Preview::Markdown(content))
        } else {
            Ok(Preview::Text {
                content,
                language: extension.to_ascii_lowercase(),
            })
        }
    }

    /// 在阻塞工作线程上构建大小受限的预览。
    pub async fn preview_async(path: PathBuf, byte_limit: usize) -> Result<Preview, ExplorerError> {
        observed_io(move || Self::preview(&path, byte_limit))
            .await
            .map_err(|error| ExplorerError::Io(std::io::Error::other(error.to_string())))?
    }

    fn read_children(&self, path: &Path) -> Result<Vec<FileEntry>, ExplorerError> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::metadata(&path)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let hidden = name.starts_with('.') || is_hidden(&metadata);
            if hidden && !self.show_hidden {
                continue;
            }
            entries.push(FileEntry {
                path,
                name,
                is_dir: metadata.is_dir(),
                size: metadata.len(),
                modified: metadata.modified().ok(),
                hidden,
            });
        }
        sort_entries(&mut entries, self.sort);
        Ok(entries)
    }

    fn toggle_expansion(&mut self, path: &Path) -> Result<(), ExplorerError> {
        if self.expanded.contains(path) {
            self.collapse(path);
        } else {
            self.expand(path)?;
        }
        Ok(())
    }

    fn expand(&mut self, path: &Path) -> Result<(), ExplorerError> {
        if self.expansion_would_cycle(path) {
            return Err(ExplorerError::DirectoryCycle(path.to_path_buf()));
        }
        let entries = self.read_children(path)?;
        let cursor_path = self.hovered().map(|row| row.entry.path.clone());
        self.children.insert(path.to_path_buf(), entries);
        self.expanded.insert(path.to_path_buf());
        self.rebuild_visible();
        if let Some(cursor_path) = cursor_path {
            self.cursor = self.closest_visible_index(&cursor_path);
        }
        Ok(())
    }

    fn collapse(&mut self, path: &Path) {
        self.expanded.remove(path);
        self.rebuild_visible_preserving_cursor();
        self.retain_visible_selection();
    }

    fn expansion_would_cycle(&self, path: &Path) -> bool {
        if path == self.root.path {
            return false;
        }
        let Ok(candidate) = path.canonicalize() else {
            return false;
        };
        let mut ancestor = path.parent();
        while let Some(current) = ancestor {
            if let Ok(canonical) = current.canonicalize()
                && canonical == candidate
            {
                return true;
            }
            if current == self.root.path {
                break;
            }
            ancestor = current.parent();
        }
        false
    }

    fn rebuild_visible_preserving_cursor(&mut self) {
        let cursor_path = self.hovered().map(|row| row.entry.path.clone());
        self.rebuild_visible();
        if let Some(cursor_path) = cursor_path {
            self.cursor = self.closest_visible_index(&cursor_path);
        }
    }

    fn rebuild_visible(&mut self) {
        let mut visible = Vec::new();
        let mut stack = vec![(self.root.clone(), 0_usize)];
        while let Some((entry, depth)) = stack.pop() {
            let expanded = entry.is_dir && self.expanded.contains(&entry.path);
            if expanded && let Some(children) = self.children.get(&entry.path) {
                stack.extend(
                    children
                        .iter()
                        .rev()
                        .cloned()
                        .map(|child| (child, depth + 1)),
                );
            }
            visible.push(FileTreeEntry {
                entry,
                depth,
                expanded,
            });
        }
        self.visible = visible;
        self.cursor = self.cursor.min(self.visible.len().saturating_sub(1));
        self.selection_anchor = self
            .selection_anchor
            .map(|anchor| anchor.min(self.visible.len().saturating_sub(1)));
    }

    fn closest_visible_index(&self, path: &Path) -> usize {
        let mut candidate = Some(path);
        while let Some(path) = candidate {
            if let Some(index) = self.visible.iter().position(|row| row.entry.path == path) {
                return index;
            }
            candidate = path.parent();
        }
        0
    }

    fn retain_visible_selection(&mut self) {
        let visible = self
            .visible
            .iter()
            .map(|row| row.entry.path.clone())
            .collect::<BTreeSet<_>>();
        self.selected
            .retain(|path| visible.contains(path) && path.exists());
    }

    /// 在应用资源操作后显示并选择已有后代。
    pub fn reveal(&mut self, path: &Path) -> Result<(), ExplorerError> {
        if !path.starts_with(self.root()) {
            return Ok(());
        }
        let mut parents = path
            .ancestors()
            .skip(1)
            .take_while(|p| p.starts_with(self.root()))
            .map(Path::to_path_buf)
            .collect::<Vec<_>>();
        parents.reverse();
        for parent in parents {
            self.expand(&parent)?;
        }
        self.select_path(path);
        Ok(())
    }

    fn select_path(&mut self, path: &Path) {
        if let Some(index) = self.visible.iter().position(|row| row.entry.path == path) {
            self.select_only(index);
        }
    }

    fn reject_root_mutation<'a>(
        &self,
        paths: impl IntoIterator<Item = &'a Path>,
    ) -> Result<(), ExplorerError> {
        if paths.into_iter().any(|path| path == self.root.path) {
            Err(ExplorerError::RootMutation)
        } else {
            Ok(())
        }
    }

    fn remap_prefix(&mut self, source: &Path, target: &Path) {
        self.expanded = self
            .expanded
            .iter()
            .map(|path| replace_prefix(path, source, target))
            .collect();
        self.selected = self
            .selected
            .iter()
            .map(|path| replace_prefix(path, source, target))
            .collect();
        self.children = std::mem::take(&mut self.children)
            .into_iter()
            .map(|(path, entries)| {
                let path = replace_prefix(&path, source, target);
                let entries = entries
                    .into_iter()
                    .map(|mut entry| {
                        entry.path = replace_prefix(&entry.path, source, target);
                        entry
                    })
                    .collect();
                (path, entries)
            })
            .collect();
    }
}

impl From<teal_workbench_core::DocumentError> for ExplorerError {
    fn from(error: teal_workbench_core::DocumentError) -> Self {
        Self::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }
}

fn entry_for_path(path: &Path) -> Result<FileEntry, ExplorerError> {
    let metadata = fs::metadata(path)?;
    let name = path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    Ok(FileEntry {
        path: path.to_path_buf(),
        hidden: name.starts_with('.') || is_hidden(&metadata),
        name,
        is_dir: metadata.is_dir(),
        size: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn not_a_directory(path: &Path) -> ExplorerError {
    ExplorerError::Io(std::io::Error::new(
        std::io::ErrorKind::NotADirectory,
        format!("{} is not a directory", path.display()),
    ))
}

fn canonical_or_original(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn normalize_restored_path(path: &Path, requested_root: &Path, canonical_root: &Path) -> PathBuf {
    path.strip_prefix(requested_root).map_or_else(
        |_| canonical_or_original(path),
        |suffix| canonical_root.join(suffix),
    )
}

fn normalize_operation_paths(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort_by(|left, right| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then_with(|| left.cmp(right))
    });
    let mut normalized: Vec<PathBuf> = Vec::new();
    for path in paths {
        if !normalized
            .iter()
            .any(|ancestor| path != *ancestor && path.starts_with(ancestor))
        {
            normalized.push(path);
        }
    }
    normalized
}

fn replace_prefix(path: &Path, source: &Path, target: &Path) -> PathBuf {
    path.strip_prefix(source)
        .map_or_else(|_| path.to_path_buf(), |suffix| target.join(suffix))
}

fn sort_entries(entries: &mut [FileEntry], sort: FileSort) {
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| match sort {
                FileSort::Name => left.name.to_lowercase().cmp(&right.name.to_lowercase()),
                FileSort::Size => right.size.cmp(&left.size),
                FileSort::Modified => right.modified.cmp(&left.modified),
            })
            .then_with(|| left.name.cmp(&right.name))
    });
}

#[cfg(windows)]
fn is_hidden(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x2 != 0
}

#[cfg(not(windows))]
const fn is_hidden(_metadata: &fs::Metadata) -> bool {
    false
}

fn copy_directory(source: &Path, target: &Path) -> Result<(), std::io::Error> {
    fs::create_dir(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let destination = target.join(entry.file_name());
        if path.is_dir() {
            copy_directory(&path, &destination)?;
        } else {
            fs::copy(path, destination)?;
        }
    }
    Ok(())
}

/// 跨文件系统工作线程保留调用方的订阅器与跨度上下文。
fn observed_io<T: Send + 'static, E: Send + 'static>(
    work: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> tokio::task::JoinHandle<Result<T, E>> {
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    let span = tracing::info_span!(
        "teal.bench.file.open",
        operation.result = tracing::field::Empty,
        otel.status_code = tracing::field::Empty
    );
    tokio::task::spawn_blocking(move || {
        tracing::dispatcher::with_default(&dispatch, || {
            span.in_scope(|| {
                let result = work();
                span.record(
                    "operation.result",
                    if result.is_ok() { "ok" } else { "error" },
                );
                if result.is_err() {
                    span.record("otel.status_code", "ERROR");
                    tracing::warn!(event.name = "failed");
                } else {
                    tracing::info!(event.name = "completed");
                }
                result
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{FileExplorer, FileSort, Preview, canonical_or_original};

    #[test]
    fn opens_root_expanded_and_navigates_the_visible_tree() {
        let directory = tempdir().expect("temporary directory");
        fs::create_dir(directory.path().join("folder")).expect("directory test data");
        fs::write(directory.path().join("folder").join("nested.txt"), "nested")
            .expect("nested test data");
        fs::write(directory.path().join("readme.md"), "# Title").expect("text test data");
        let mut explorer = FileExplorer::open(directory.path()).expect("open file explorer");

        assert_eq!(explorer.visible_entries()[0].depth, 0);
        assert!(explorer.visible_entries()[0].expanded);
        assert!(explorer.visible_entries()[1].entry.is_dir);
        explorer.move_cursor(1);
        explorer.navigate_right().expect("expand folder");
        assert!(explorer.visible_entries()[1].expanded);
        explorer
            .navigate_right()
            .expect("Enter the first child node");
        assert_eq!(
            explorer.hovered().expect("nested row").entry.name,
            "nested.txt"
        );
        explorer.navigate_left().expect("move to parent");
        explorer.navigate_left().expect("collapse parent");
        assert!(!explorer.visible_entries()[1].expanded);
    }

    #[test]
    fn selection_is_visible_and_operation_paths_drop_selected_descendants() {
        let directory = tempdir().expect("temporary directory");
        fs::create_dir(directory.path().join("folder")).expect("directory test data");
        fs::write(directory.path().join("folder").join("child.txt"), "child")
            .expect("child item test data");
        let mut explorer = FileExplorer::open(directory.path()).expect("open file explorer");
        explorer.move_cursor(1);
        explorer.navigate_right().expect("expand folder");
        explorer.select_only(1);
        explorer.select_range(2);
        assert_eq!(
            explorer.selected_or_hovered(),
            vec![canonical_or_original(&directory.path().join("folder"))]
        );
        explorer.navigate_left().expect("collapse folder");
        assert_eq!(
            explorer.selected_or_hovered(),
            vec![canonical_or_original(&directory.path().join("folder"))]
        );
    }

    #[test]
    fn creates_relative_to_cursor_context_and_preserves_state_on_refresh() {
        let directory = tempdir().expect("temporary directory");
        fs::create_dir(directory.path().join("folder")).expect("directory test data");
        let mut explorer = FileExplorer::open(directory.path()).expect("open file explorer");
        explorer.move_cursor(1);
        explorer
            .create_file("created.txt")
            .expect("Create in hover directory");
        assert!(
            directory
                .path()
                .join("folder")
                .join("created.txt")
                .is_file()
        );
        assert_eq!(
            explorer.hovered().expect("File created").entry.path,
            canonical_or_original(directory.path())
                .join("folder")
                .join("created.txt")
        );
        explorer.set_sort(FileSort::Modified);
        explorer.refresh().expect("refresh");
        assert_eq!(explorer.sort(), FileSort::Modified);
        assert!(
            explorer
                .expanded_paths()
                .contains(&canonical_or_original(&directory.path().join("folder")))
        );
    }

    #[test]
    fn restores_only_valid_expansions_and_clamps_missing_cursor_to_ancestor() {
        let directory = tempdir().expect("temporary directory");
        let folder = directory.path().join("folder");
        fs::create_dir(&folder).expect("directory test data");
        fs::write(folder.join("child.txt"), "child").expect("child item test data");
        let missing = folder.join("missing");
        let explorer = FileExplorer::restore(
            directory.path(),
            &[
                directory.path().to_path_buf(),
                folder.clone(),
                missing.clone(),
            ],
            Some(&missing),
            true,
            FileSort::Name,
        )
        .expect("restore tree");
        assert!(
            explorer
                .expanded_paths()
                .contains(&canonical_or_original(&folder))
        );
        assert!(!explorer.expanded_paths().contains(&missing));
        assert_eq!(
            explorer.hovered().expect("ancestor row cursor").entry.path,
            canonical_or_original(&folder)
        );
    }

    #[test]
    fn elevates_a_collapsed_root_and_keeps_the_old_root_selected() {
        let directory = tempdir().expect("temporary directory");
        let child = directory.path().join("child");
        fs::create_dir(&child).expect("child directory");
        let mut explorer = FileExplorer::open(&child).expect("open file explorer");
        explorer.navigate_left().expect("collapse root");
        assert!(explorer.navigate_left().expect("promote root directory"));
        assert_eq!(explorer.root(), canonical_or_original(directory.path()));
        assert_eq!(
            explorer.hovered().expect("old root row").entry.path,
            canonical_or_original(&child)
        );
    }

    #[test]
    fn refresh_preserves_a_collapsed_root() {
        let directory = tempdir().expect("temporary directory");
        fs::write(directory.path().join("child.txt"), "child").expect("file test data");
        let mut explorer = FileExplorer::open(directory.path()).expect("open file explorer");
        explorer.navigate_left().expect("collapse root");
        explorer.refresh().expect("refresh collapsed root");
        assert_eq!(explorer.visible_entries().len(), 1);
        assert!(!explorer.visible_entries()[0].expanded);
    }

    #[test]
    fn previews_text_and_binary() {
        let directory = tempdir().expect("temporary directory");
        fs::write(directory.path().join("readme.md"), "# Title").expect("text test data");
        fs::write(directory.path().join("data.bin"), [0xff, 0x00]).expect("binary test data");
        assert!(matches!(
            FileExplorer::preview(&directory.path().join("readme.md"), 1024)
                .expect("Markdown preview"),
            Preview::Markdown(_)
        ));
        assert!(matches!(
            FileExplorer::preview(&directory.path().join("data.bin"), 1024)
                .expect("binary preview"),
            Preview::Binary(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_expand_a_symbolic_link_cycle() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().expect("temporary directory");
        symlink(directory.path(), directory.path().join("again")).expect("circular link");
        let mut explorer = FileExplorer::open(directory.path()).expect("open file explorer");
        explorer.move_cursor(1);
        assert!(explorer.navigate_right().is_err());
        assert!(!explorer.visible_entries()[1].expanded);
    }

    #[cfg(windows)]
    #[test]
    fn refuses_to_expand_a_symbolic_link_cycle() {
        use std::os::windows::fs::symlink_dir;

        let directory = tempdir().expect("temporary directory");
        if symlink_dir(directory.path(), directory.path().join("again")).is_err() {
            return;
        }
        let mut explorer = FileExplorer::open(directory.path()).expect("open file explorer");
        explorer.move_cursor(1);
        assert!(explorer.navigate_right().is_err());
        assert!(!explorer.visible_entries()[1].expanded);
    }
}
