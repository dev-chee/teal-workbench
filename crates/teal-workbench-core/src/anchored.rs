//! 以已打开目录为根执行文件操作，避免最终路径重新解析祖先目录。

use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(unix)]
use std::{
    ffi::CString,
    os::fd::{AsRawFd, FromRawFd},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

/// 已验证的父目录；所有叶操作均相对该目录执行。
pub(crate) struct DirectoryAnchor {
    path: PathBuf,
    #[cfg(unix)]
    directory: File,
    #[cfg(windows)]
    directory: File,
    #[cfg(windows)]
    ancestors: Vec<File>,
}

impl std::fmt::Debug for DirectoryAnchor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DirectoryAnchor")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl DirectoryAnchor {
    /// 打开并固定规范父目录。
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        // 不先 canonicalize：这一步会跟随链接/重解析点，绕过平台 walk 的拒绝检查。
        let path = std::path::absolute(path)?;
        platform::open(path)
    }

    /// 按目录句柄逐级创建缺失目录，拒绝链接或重解析点祖先。
    pub(crate) fn ensure(path: &Path) -> io::Result<Self> {
        platform::ensure(std::path::absolute(path)?)
    }

    pub(crate) fn file(&self) -> &File {
        &self.directory
    }

    /// 返回打开锚点时的规范路径，仅用于构造文档身份和诊断。
    #[must_use]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// 读取一个叶文件；叶项不能是符号链接或重解析点。
    pub(crate) fn read(&self, name: &OsStr) -> io::Result<Vec<u8>> {
        validate_name(name)?;
        platform::read(self, name)
    }

    /// 在锚定目录内原子替换一个叶文件。
    pub(crate) fn atomic_replace(&self, name: &OsStr, bytes: &[u8]) -> io::Result<()> {
        validate_name(name)?;
        platform::atomic_replace(self, name, bytes)
    }

    /// 在锚定目录内无覆盖创建一个叶文件。
    pub(crate) fn create_new(&self, name: &OsStr, bytes: &[u8]) -> io::Result<()> {
        validate_name(name)?;
        platform::create_new(self, name, bytes)
    }

    /// 在锚定目录内创建一个子目录。
    pub(crate) fn create_dir(&self, name: &OsStr) -> io::Result<()> {
        validate_name(name)?;
        platform::create_dir(self, name)
    }

    pub(crate) fn rename_to(
        &self,
        name: &OsStr,
        destination: &Self,
        new_name: &OsStr,
    ) -> io::Result<()> {
        validate_name(name)?;
        validate_name(new_name)?;
        platform::rename_to(self, name, destination, new_name)
    }

    pub(crate) fn remove(&self, name: &OsStr, directory: bool) -> io::Result<()> {
        validate_name(name)?;
        platform::remove(self, name, directory)
    }

    /// 列出锚定目录的直接子项；Unix 从目录句柄读取，Windows 依赖锚点句柄拒绝祖先替换。
    pub(crate) fn entries(&self) -> io::Result<Vec<OsString>> {
        platform::entries(self)
    }

    /// 产生一个仅用于该锚点的临时叶名。
    fn temporary_name(&self) -> OsString {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        OsString::from(format!(".teal-save-{}-{}", std::process::id(), serial))
    }
}

fn validate_name(name: &OsStr) -> io::Result<()> {
    let path = Path::new(name);
    if !matches!(path.components().next(), Some(Component::Normal(_)))
        || path.components().count() != 1
        || name.is_empty()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "anchored file operation requires a single normal name",
        ));
    }
    Ok(())
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    const DIRECTORY_FLAGS: i32 =
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
    const FILE_FLAGS: i32 = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
    const TEMP_FLAGS: i32 =
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW;

    pub(super) fn open(path: PathBuf) -> io::Result<DirectoryAnchor> {
        walk(path, false)
    }

    pub(super) fn ensure(path: PathBuf) -> io::Result<DirectoryAnchor> {
        walk(path, true)
    }

    fn walk(path: PathBuf, create: bool) -> io::Result<DirectoryAnchor> {
        let mut directory = openat_file(libc::AT_FDCWD, OsStr::new("/"), DIRECTORY_FLAGS, 0)?;
        for component in path.components() {
            let Component::Normal(name) = component else {
                if component == Component::RootDir {
                    continue;
                }
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid anchored directory path",
                ));
            };
            let next = openat_file(directory.as_raw_fd(), name, DIRECTORY_FLAGS, 0);
            directory = match next {
                Ok(next) => next,
                Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                    let name_c = c_name(name)?;
                    // SAFETY: name is a single component and directory is a live descriptor.
                    let status =
                        unsafe { libc::mkdirat(directory.as_raw_fd(), name_c.as_ptr(), 0o755) };
                    if status != 0 {
                        let error = io::Error::last_os_error();
                        if error.kind() != io::ErrorKind::AlreadyExists {
                            return Err(error);
                        }
                    }
                    openat_file(directory.as_raw_fd(), name, DIRECTORY_FLAGS, 0)?
                }
                Err(error) => return Err(error),
            };
        }
        Ok(DirectoryAnchor { path, directory })
    }

    pub(super) fn read(anchor: &DirectoryAnchor, name: &OsStr) -> io::Result<Vec<u8>> {
        let file = openat_file(anchor.directory.as_raw_fd(), name, FILE_FLAGS, 0)?;
        let mut bytes = Vec::new();
        let mut file = file;
        io::Read::read_to_end(&mut file, &mut bytes)?;
        Ok(bytes)
    }

    pub(super) fn entries(anchor: &DirectoryAnchor) -> io::Result<Vec<OsString>> {
        use std::os::unix::ffi::OsStringExt;

        // fdopendir 接管 dup 出来的描述符；原锚点仍由 DirectoryAnchor 持有。
        let duplicate = unsafe { libc::dup(anchor.directory.as_raw_fd()) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error());
        }
        let stream = unsafe { libc::fdopendir(duplicate) };
        if stream.is_null() {
            // SAFETY: fd 未被 fdopendir 接管，且仍是 dup 返回的有效描述符。
            unsafe { libc::close(duplicate) };
            return Err(io::Error::last_os_error());
        }
        let mut entries = Vec::new();
        loop {
            // SAFETY: stream 是有效 DIR*，readdir 返回的 dirent 只在下一次调用前有效。
            let item = unsafe { libc::readdir(stream) };
            if item.is_null() {
                break;
            }
            // SAFETY: d_name 是以 NUL 结尾的目录项名称。
            let name = unsafe { std::ffi::CStr::from_ptr((*item).d_name.as_ptr()) }.to_bytes();
            if name != b"." && name != b".." {
                entries.push(OsString::from_vec(name.to_owned()));
            }
        }
        // SAFETY: fdopendir 成功后由 closedir 负责释放 DIR* 及其描述符。
        if unsafe { libc::closedir(stream) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(entries)
    }

    pub(super) fn atomic_replace(
        anchor: &DirectoryAnchor,
        name: &OsStr,
        bytes: &[u8],
    ) -> io::Result<()> {
        let temporary = anchor.temporary_name();
        let mut file = openat_file(anchor.directory.as_raw_fd(), &temporary, TEMP_FLAGS, 0o600)?;
        let result = (|| {
            io::Write::write_all(&mut file, bytes)?;
            file.sync_all()?;
            renameat(anchor, &temporary, name)
        })();
        if result.is_err() {
            let _ = unlinkat(anchor, &temporary, false);
        }
        result
    }

    pub(super) fn create_new(
        anchor: &DirectoryAnchor,
        name: &OsStr,
        bytes: &[u8],
    ) -> io::Result<()> {
        let temporary = anchor.temporary_name();
        let mut file = openat_file(anchor.directory.as_raw_fd(), &temporary, TEMP_FLAGS, 0o600)?;
        let result = (|| {
            io::Write::write_all(&mut file, bytes)?;
            file.sync_all()?;
            let source = c_name(&temporary)?;
            let destination = c_name(name)?;
            // SAFETY: linkat publishes the already flushed temporary without replacing a leaf.
            let status = unsafe {
                libc::linkat(
                    anchor.directory.as_raw_fd(),
                    source.as_ptr(),
                    anchor.directory.as_raw_fd(),
                    destination.as_ptr(),
                    0,
                )
            };
            if status != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })();
        let cleanup = unlinkat(anchor, &temporary, false);
        result.and(cleanup)
    }

    pub(super) fn create_dir(anchor: &DirectoryAnchor, name: &OsStr) -> io::Result<()> {
        let name = c_name(name)?;
        // SAFETY: 叶名没有 NUL，目录句柄有效；mkdirat 不会重新解析祖先路径。
        let result = unsafe { libc::mkdirat(anchor.directory.as_raw_fd(), name.as_ptr(), 0o755) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub(super) fn remove(
        anchor: &DirectoryAnchor,
        name: &OsStr,
        directory: bool,
    ) -> io::Result<()> {
        // rmdir 不能删除指向目录的符号链接；先读取目录项类型，再选择 unlink/rmdir。
        let directory = if directory {
            let name = c_name(name)?;
            let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
            // SAFETY: 目录句柄和单一名称有效，缓冲区由 fstatat 初始化；不跟随最终链接。
            let status = unsafe {
                libc::fstatat(
                    anchor.directory.as_raw_fd(),
                    name.as_ptr(),
                    metadata.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if status != 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: fstatat 成功初始化了 metadata。
            let metadata = unsafe { metadata.assume_init() };
            (metadata.st_mode & libc::S_IFMT) == libc::S_IFDIR
        } else {
            false
        };
        unlinkat(anchor, name, directory)
    }

    pub(super) fn rename_to(
        anchor: &DirectoryAnchor,
        name: &OsStr,
        destination: &DirectoryAnchor,
        new_name: &OsStr,
    ) -> io::Result<()> {
        let name = c_name(name)?;
        let new_name = c_name(new_name)?;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            // SAFETY: 两个锚点句柄有效，名称均为单一目录项；NOREPLACE 防止并发目标覆盖。
            let status = unsafe {
                libc::renameat2(
                    anchor.directory.as_raw_fd(),
                    name.as_ptr(),
                    destination.directory.as_raw_fd(),
                    new_name.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            if status == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            let _ = (anchor, destination, name, new_name);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "atomic anchored no-replace rename is unavailable",
            ))
        }
    }

    fn openat_file(dirfd: i32, name: &OsStr, flags: i32, mode: u32) -> io::Result<File> {
        let name = if dirfd == libc::AT_FDCWD {
            c_path(name)?
        } else {
            c_name(name)?
        };
        // SAFETY: C 字符串没有 NUL，目录句柄、标志和权限对应 openat 契约。
        let fd = unsafe { libc::openat(dirfd, name.as_ptr(), flags, mode) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd 由 openat 新建且有效，所有权仅移交给 File 一次。
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    fn renameat(anchor: &DirectoryAnchor, old: &OsStr, new: &OsStr) -> io::Result<()> {
        let old = c_name(old)?;
        let new = c_name(new)?;
        // SAFETY: 名称是已验证单一目录项，两个目录句柄仍然有效。
        let result = unsafe {
            libc::renameat(
                anchor.directory.as_raw_fd(),
                old.as_ptr(),
                anchor.directory.as_raw_fd(),
                new.as_ptr(),
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn unlinkat(anchor: &DirectoryAnchor, name: &OsStr, directory: bool) -> io::Result<()> {
        let name = c_name(name)?;
        let flags = if directory { libc::AT_REMOVEDIR } else { 0 };
        // SAFETY: 名称已验证，目录句柄仍然有效。
        let result = unsafe { libc::unlinkat(anchor.directory.as_raw_fd(), name.as_ptr(), flags) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn c_name(name: &OsStr) -> io::Result<CString> {
        CString::new(name.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "file name contains NUL"))
    }

    fn c_path(path: &OsStr) -> io::Result<CString> {
        CString::new(path.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::fs;
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    pub(super) fn open(path: PathBuf) -> io::Result<DirectoryAnchor> {
        walk(path, false)
    }

    pub(super) fn ensure(path: PathBuf) -> io::Result<DirectoryAnchor> {
        walk(path, true)
    }

    fn walk(path: PathBuf, create: bool) -> io::Result<DirectoryAnchor> {
        let mut ancestors = Vec::new();
        let mut chain = path
            .ancestors()
            .filter(|ancestor| !ancestor.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .collect::<Vec<_>>();
        chain.reverse();
        for ancestor in chain {
            let file = match open_directory(&ancestor) {
                Ok(file) => file,
                Err(error)
                    if create
                        && error.kind() == io::ErrorKind::NotFound
                        && !ancestors.is_empty() =>
                {
                    fs::create_dir(&ancestor)?;
                    open_directory(&ancestor)?
                }
                Err(error) => return Err(error),
            };
            ancestors.push(file);
        }
        let directory = ancestors
            .last()
            .map(File::try_clone)
            .transpose()?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "directory anchor missing"))?;
        Ok(DirectoryAnchor {
            path,
            directory,
            ancestors,
        })
    }

    pub(super) fn read(anchor: &DirectoryAnchor, name: &OsStr) -> io::Result<Vec<u8>> {
        let path = anchor.path.join(name);
        let mut file = open_leaf(&path)?;
        let mut bytes = Vec::new();
        io::Read::read_to_end(&mut file, &mut bytes)?;
        Ok(bytes)
    }

    pub(super) fn atomic_replace(
        anchor: &DirectoryAnchor,
        name: &OsStr,
        bytes: &[u8],
    ) -> io::Result<()> {
        let temporary = anchor.path.join(anchor.temporary_name());
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            io::Write::write_all(&mut file, bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, anchor.path.join(name))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub(super) fn create_new(
        anchor: &DirectoryAnchor,
        name: &OsStr,
        bytes: &[u8],
    ) -> io::Result<()> {
        let temporary = anchor.path.join(anchor.temporary_name());
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            io::Write::write_all(&mut file, bytes)?;
            file.sync_all()?;
            drop(file);
            move_no_replace(&temporary, &anchor.path.join(name))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub(super) fn create_dir(anchor: &DirectoryAnchor, name: &OsStr) -> io::Result<()> {
        fs::create_dir(anchor.path.join(name))
    }

    pub(super) fn rename_to(
        anchor: &DirectoryAnchor,
        name: &OsStr,
        destination: &DirectoryAnchor,
        new_name: &OsStr,
    ) -> io::Result<()> {
        move_no_replace(&anchor.path.join(name), &destination.path.join(new_name))
    }

    pub(super) fn remove(
        anchor: &DirectoryAnchor,
        name: &OsStr,
        directory: bool,
    ) -> io::Result<()> {
        if directory {
            fs::remove_dir(anchor.path.join(name))
        } else {
            fs::remove_file(anchor.path.join(name))
        }
    }

    pub(super) fn entries(anchor: &DirectoryAnchor) -> io::Result<Vec<OsString>> {
        let mut output = Vec::new();
        for entry in fs::read_dir(&anchor.path)? {
            output.push(entry?.file_name());
        }
        Ok(output)
    }

    fn move_no_replace(old: &Path, new: &Path) -> io::Result<()> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::MoveFileW;
        let old = old
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let new = new
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        // SAFETY: 两个 UTF-16 字符串有终止 NUL；父链由拒绝删除共享的句柄保持。
        if unsafe { MoveFileW(old.as_ptr(), new.as_ptr()) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn open_directory(path: &Path) -> io::Result<File> {
        let file = fs::OpenOptions::new()
            .read(true)
            // 拒绝删除共享使已打开祖先不能在锚点存活期间被重命名或替换。
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let attributes = std::os::windows::fs::MetadataExt::file_attributes(&file.metadata()?);
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "reparse point is not allowed in a directory anchor",
            ));
        }
        Ok(file)
    }

    fn open_leaf(path: &Path) -> io::Result<File> {
        let file = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let attributes = std::os::windows::fs::MetadataExt::file_attributes(&file.metadata()?);
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "reparse point is not allowed as an anchored leaf",
            ));
        }
        Ok(file)
    }
}
