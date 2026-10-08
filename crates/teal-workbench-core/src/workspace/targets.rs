//! 工作区路径证明：保存规范执行目标和仍存活的文件身份，执行时重新核对。

use std::{
    fs::{self, File},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileKey {
    volume: u64,
    id: [u8; 16],
}

/// 句柄保持对象存活，避免删除后的文件编号复用使身份比较误判。
#[derive(Clone, Debug)]
struct Identity {
    _file: Arc<File>,
    key: FileKey,
}

impl Identity {
    fn open(path: &Path, entry: bool) -> Result<Self, String> {
        let file = identity_file(path, entry).map_err(|error| error.to_string())?;
        let key = file_key(&file).map_err(|error| error.to_string())?;
        Ok(Self {
            _file: Arc::new(file),
            key,
        })
    }
    fn verify(&self, path: &Path, entry: bool) -> Result<(), String> {
        if Self::open(path, entry)?.key != self.key {
            return Err("Workspace path identity changed after resolution".into());
        }
        Ok(())
    }
}

#[cfg(windows)]
fn identity_file(path: &Path, entry: bool) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_SHARE_DELETE,
    };
    fs::OpenOptions::new()
        .read(true)
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(
            FILE_FLAG_BACKUP_SEMANTICS
                | if entry {
                    FILE_FLAG_OPEN_REPARSE_POINT
                } else {
                    0
                },
        )
        .open(path)
}

#[cfg(windows)]
fn file_key(file: &File) -> std::io::Result<FileKey> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandleEx, FileIdInfo, FILE_ID_INFO,
    };
    let mut info = FILE_ID_INFO::default();
    // SAFETY: 文件句柄在调用期间有效；缓冲区类型和长度对应 FileIdInfo。
    let success = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&raw mut info).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if success == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(FileKey {
        volume: info.VolumeSerialNumber,
        id: info.FileId.Identifier,
    })
}

#[cfg(unix)]
fn identity_file(path: &Path, entry: bool) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut flags = libc::O_NONBLOCK;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        flags |= libc::O_PATH;
        if entry {
            flags |= libc::O_NOFOLLOW;
        }
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        if entry && fs::symlink_metadata(path)?.file_type().is_symlink() {
            flags |= libc::O_SYMLINK;
        }
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    )))]
    {
        if entry {
            flags |= libc::O_NOFOLLOW;
        }
    }
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(path)
}

#[cfg(unix)]
fn file_key(file: &File) -> std::io::Result<FileKey> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    let mut id = [0; 16];
    id[..8].copy_from_slice(&metadata.ino().to_le_bytes());
    Ok(FileKey {
        volume: metadata.dev(),
        id,
    })
}

#[cfg(not(any(unix, windows)))]
fn identity_file(_: &Path, _: bool) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Path identities are unavailable",
    ))
}
#[cfg(not(any(unix, windows)))]
fn file_key(_: &File) -> std::io::Result<FileKey> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Path identities are unavailable",
    ))
}

#[derive(Clone, Debug)]
pub(super) struct Root {
    input: PathBuf,
    path: PathBuf,
    identity: Identity,
}

impl Root {
    pub(super) fn new(workspace: &Path) -> Result<Self, String> {
        let input = std::path::absolute(workspace).map_err(|error| error.to_string())?;
        let path = canonical(&input)?;
        if !path.is_dir() {
            return Err("Workspace root is not a directory".into());
        }
        let identity = Identity::open(&path, false)?;
        Ok(Self {
            input,
            path,
            identity,
        })
    }
    fn verify(&self) -> Result<(), String> {
        if canonical(&self.input)? != self.path {
            return Err("Workspace root changed after resolution".into());
        }
        self.identity.verify(&self.path, false)
    }
    pub(super) fn input(&self) -> &Path {
        &self.input
    }
    fn require_same(&self, other: &Self) -> Result<(), String> {
        other.verify()?;
        if self.identity.key != other.identity.key || self.path != other.path {
            return Err("Resolved path belongs to another workspace".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct Basis {
    root: Root,
    original: PathBuf,
    path: PathBuf,
    anchor: PathBuf,
    identity: Identity,
    confined: bool,
}

impl Basis {
    fn new(root: &Root, input: &Path, confined: bool) -> Result<Self, String> {
        root.verify()?;
        let root = root.clone();
        let original = std::path::absolute(if input.is_absolute() {
            input.to_owned()
        } else {
            root.input.join(input)
        })
        .map_err(|error| error.to_string())?;
        let path = resolve(&original)?;
        if confined && !path.starts_with(&root.path) {
            return Err("Path is outside the workspace".into());
        }
        let mut anchor = path.clone();
        loop {
            match fs::symlink_metadata(&anchor) {
                Ok(_) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if !anchor.pop() {
                        return Err(error.to_string());
                    }
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        let identity = Identity::open(&anchor, false)?;
        Ok(Self {
            root,
            original,
            path,
            anchor,
            identity,
            confined,
        })
    }
    fn verify(&self) -> Result<(), String> {
        self.root.verify()?;
        let current = resolve(&self.original)?;
        if current != self.path || (self.confined && !current.starts_with(&self.root.path)) {
            return Err("Workspace target changed after resolution".into());
        }
        self.identity.verify(&self.anchor, false)
    }
}

/// 已解析的读取、写入或复制目标；最终链接被解析，字段不能自行构造。
///
/// 对象固定工作区、绝对输入、实际执行路径与身份。它不冻结文件内容，也不替代文档版本检查。
#[derive(Clone, Debug)]
pub struct ResolvedTarget {
    basis: Basis,
    leaf: Option<Identity>,
    directory: bool,
}

impl ResolvedTarget {
    pub(super) fn require_workspace(&self, root: &Root) -> Result<(), String> {
        self.basis.root.require_same(root)
    }
    pub(super) fn same_object(&self, other: &Self) -> bool {
        matches!((&self.leaf,&other.leaf),(Some(left),Some(right)) if left.key==right.key)
    }
    pub(super) fn confined(&self) -> bool {
        self.basis.confined
    }
    /// 自己创建目录后更新叶身份，仍保留原始路径和原祖先证明。
    pub(super) fn materialized(&self) -> Result<Self, String> {
        self.basis.verify()?;
        if let Some(identity) = &self.leaf {
            identity.verify(self.path(), false)?;
        }
        let current = Self::new(&self.basis.root, self.original(), self.confined(), true)?;
        if current.path() != self.path() {
            return Err("Workspace target changed during creation".into());
        }
        Ok(current)
    }
    pub(super) fn new(
        root: &Root,
        input: &Path,
        confined: bool,
        must_exist: bool,
    ) -> Result<Self, String> {
        let basis = Basis::new(root, input, confined)?;
        let metadata = optional_metadata(&basis.path)?;
        if must_exist && metadata.is_none() {
            return Err("Source path does not exist".into());
        }
        if metadata
            .as_ref()
            .is_some_and(|value| !value.is_file() && !value.is_dir())
        {
            return Err("Target must be a regular file or directory".into());
        }
        let leaf = metadata
            .as_ref()
            .map(|_| Identity::open(&basis.path, false))
            .transpose()?;
        let directory = metadata.is_some_and(|value| value.is_dir());
        Ok(Self {
            basis,
            leaf,
            directory,
        })
    }
    /// 返回后续执行使用的同一个规范路径。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.basis.path
    }
    /// 返回以工作区为基准绝对化的原始输入，供保持复制文件名使用。
    #[must_use]
    pub fn original(&self) -> &Path {
        &self.basis.original
    }
    /// 解析时目标是否存在。
    #[must_use]
    pub fn exists(&self) -> bool {
        self.leaf.is_some()
    }
    /// 解析时目标是否为目录。
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.directory
    }
    /// 在副作用前复核解析路径与对象身份；发生替换或越界时失败。
    ///
    /// # Errors
    /// 工作区、祖先或最终目标被替换，或路径无法读取时返回错误。
    pub fn revalidate(&self) -> Result<(), String> {
        self.basis.verify()?;
        match (&self.leaf, optional_metadata(self.path())?) {
            (Some(identity), Some(_)) => identity.verify(self.path(), false),
            (None, None) => Ok(()),
            _ => Err("Workspace target existence changed after resolution".into()),
        }
    }
}

/// 已解析的删除或重命名目录项；只解析父目录，不跟随最终链接。
#[derive(Clone, Debug)]
pub struct ResolvedEntry {
    parent: Basis,
    path: PathBuf,
    original: PathBuf,
    leaf: Option<Identity>,
    link: bool,
    directory: bool,
}

impl ResolvedEntry {
    pub(super) fn require_workspace(&self, root: &Root) -> Result<(), String> {
        self.parent.root.require_same(root)
    }
    pub(super) fn new(
        root: &Root,
        input: &Path,
        confined: bool,
        must_exist: bool,
    ) -> Result<Self, String> {
        let absolute = std::path::absolute(if input.is_absolute() {
            input.to_owned()
        } else {
            root.input.join(input)
        })
        .map_err(|error| error.to_string())?;
        let name = absolute
            .file_name()
            .ok_or("A directory entry name is required")?;
        let parent = Basis::new(
            root,
            absolute.parent().ok_or("A parent directory is required")?,
            confined,
        )?;
        let path = parent.path.join(name);
        let metadata = optional_metadata(&path)?;
        if must_exist && metadata.is_none() {
            return Err("Source entry does not exist".into());
        }
        let leaf = metadata
            .as_ref()
            .map(|_| Identity::open(&path, true))
            .transpose()?;
        let link = metadata
            .as_ref()
            .is_some_and(|value| value.file_type().is_symlink());
        let directory = metadata.as_ref().is_some_and(entry_is_directory);
        Ok(Self {
            parent,
            path,
            original: absolute,
            leaf,
            link,
            directory,
        })
    }
    /// 返回规范父目录下的原目录项路径，保留最终链接。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// 原始绝对输入。
    #[must_use]
    pub fn original(&self) -> &Path {
        &self.original
    }
    /// 目录项是否存在；悬空链接也属于存在。
    #[must_use]
    pub fn exists(&self) -> bool {
        self.leaf.is_some()
    }
    /// 目录项是否为链接。
    #[must_use]
    pub fn is_link(&self) -> bool {
        self.link
    }
    /// 是否使用目录删除操作；Windows 目录链接也返回真。
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.directory
    }
    /// 在删除或重命名前核对父目录及最终目录项的身份。
    ///
    /// # Errors
    /// 路径、身份、存在状态或工作区发生变化时返回错误，不访问链接目标。
    pub fn revalidate(&self) -> Result<(), String> {
        self.parent.verify()?;
        match (&self.leaf, optional_metadata(&self.path)?) {
            (Some(identity), Some(_)) => identity.verify(&self.path, true),
            (None, None) => Ok(()),
            _ => Err("Workspace entry existence changed after resolution".into()),
        }
    }
}

fn optional_metadata(path: &Path) -> Result<Option<fs::Metadata>, String> {
    match fs::symlink_metadata(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn entry_is_directory(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x10 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.is_dir()
    }
}

/// 解析完整路径或最深现有前缀，允许缺失后缀与内部悬空链接，不要求系统祖先可单独访问。
pub(super) fn resolve(input: &Path) -> Result<PathBuf, String> {
    resolve_at(input, 0)
}

fn resolve_at(input: &Path, depth: usize) -> Result<PathBuf, String> {
    if depth > 64 {
        return Err("Path contains a link cycle or too many links".into());
    }
    let mut ancestor = input.to_path_buf();
    let mut suffix = Vec::new();
    loop {
        match fs::canonicalize(&ancestor) {
            Ok(existing) => {
                let mut resolved = normalize(existing);
                for part in suffix.into_iter().rev() {
                    match Path::new(&part).components().next() {
                        Some(Component::ParentDir) => {
                            resolved.pop();
                        }
                        Some(Component::CurDir) => {}
                        _ => resolved.push(part),
                    }
                }
                // 缺失部分中的 .. 可能重新暴露一个现有链接，需再次解析新路径。
                return if resolved != normalize(input.to_owned()) {
                    resolve_at(&resolved, depth + 1)
                } else {
                    Ok(resolved)
                };
            }
            Err(error) => {
                if let Ok(metadata) = fs::symlink_metadata(&ancestor) {
                    if metadata.file_type().is_symlink() {
                        let target = fs::read_link(&ancestor).map_err(|error| error.to_string())?;
                        if target
                            .components()
                            .next()
                            .is_some_and(|part| matches!(part, Component::Prefix(_)))
                            && !target.has_root()
                        {
                            return Err("Drive-relative link targets are unsupported".into());
                        }
                        let mut expanded = if target.is_absolute() {
                            target
                        } else {
                            ancestor
                                .parent()
                                .ok_or("Link parent is missing")?
                                .join(target)
                        };
                        for part in suffix.into_iter().rev() {
                            expanded.push(part);
                        }
                        return resolve_at(&expanded, depth + 1);
                    }
                    return Err(error.to_string());
                }
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(error.to_string());
                }
                suffix.push(
                    ancestor
                        .components()
                        .next_back()
                        .ok_or("Workspace ancestor is missing")?
                        .as_os_str()
                        .to_os_string(),
                );
                if !ancestor.pop() {
                    return Err("Workspace ancestor is missing".into());
                }
            }
        }
    }
}
fn canonical(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path)
        .map(normalize)
        .map_err(|error| error.to_string())
}

/// Windows 的扩展前缀只改变表示；领域路径保持与编辑器和 URI 使用的绝对路径一致。
pub(super) fn normalize(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(plain) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(plain);
        }
    }
    path
}
