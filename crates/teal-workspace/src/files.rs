//! 版本化文件写入与独立修改日志。只管理通过本服务执行的修改。

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

static NEXT_CHANGE: AtomicU64 = AtomicU64::new(1);

/// 受支持的无损文本编码；不支持的二进制或有损文本会明确拒绝。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextEncoding {
    /// 无 BOM 的 UTF-8。
    Utf8,
    /// 带 BOM 的 UTF-8。
    Utf8Bom,
    /// 带 BOM 的 UTF-16 小端。
    Utf16Le,
    /// 带 BOM 的 UTF-16 大端。
    Utf16Be,
}

/// 文件的一致性检查依据；version 是实际原始字节的 SHA-256。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSnapshot {
    /// 规范化工作区内的绝对路径。
    pub path: PathBuf,
    /// 原始文件内容摘要。
    pub version: String,
    /// 无 BOM 的文本，保留读取时的换行。
    pub text: String,
    /// 原始编码。
    pub encoding: TextEncoding,
}

/// 单个文件变更的持久状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeState {
    /// 备份与预期结果已落盘，实际替换可能尚未发生。
    Prepared,
    /// 目标文件已替换为预期结果。
    Applied,
    /// 已记录撤销意图，目标可能已经恢复；重启时根据摘要核对。
    UndoPrepared,
    /// 目标文件已恢复到备份内容。
    Undone,
}

/// 可核对与撤销的单文件修改。它不是跨文件事务。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeRecord {
    /// 日志格式版本。
    pub format_version: u32,
    /// 本服务分配的安全文件名标识。
    pub id: String,
    /// 工作区内相对路径，不接受任意日志提供的绝对目标。
    pub path: PathBuf,
    /// 修改前原始字节摘要。
    pub before: String,
    /// 修改后原始字节摘要。
    pub after: String,
    /// 当前已记录状态。
    pub state: ChangeState,
}

/// 版本冲突、路径或持久化操作错误。
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    /// 文件系统错误。
    #[error("Workspace I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// 请求越过本工作区范围；其他根目录应单独登记工作区。
    #[error("The path does not belong to the current workspace: {0}")]
    OutsideWorkspace(PathBuf),
    /// 实际文件与读取或撤销时的版本不一致。
    #[error("The file has been modified and overwriting is refused: {0}")]
    Conflict(PathBuf),
    /// 无法无损解码为支持的文本。
    #[error("File encoding is not supported or text is invalid")]
    InvalidEncoding,
    /// 修改日志损坏、版本不兼容或引用越界。
    #[error("Invalid modification log: {0}")]
    InvalidJournal(String),
    /// 同一工作区已经有修改操作。
    #[error("The workspace is performing other modifications")]
    Busy,
}

/// 一个明确根目录内的版本化文件服务。
///
/// 本服务串行化自身的修改，并在替换前比较原始字节。它不隔离任意外部进程，也不宣称
/// 操作系统提供跨文件原子事务。日志目录默认位于 .teal/runtime/changes。
#[derive(Clone, Debug)]
pub struct Workspace {
    root: PathBuf,
    journal: PathBuf,
}

impl Workspace {
    /// 打开既有工作区并创建修改日志目录。
    pub fn open(root: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let root = root.as_ref().canonicalize()?;
        let mut journal = root.clone();
        for directory in [".teal", "runtime", "changes"] {
            journal.push(directory);
            match fs::symlink_metadata(&journal) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&journal)?
                }
                Err(error) => return Err(error.into()),
            }
            journal = journal.canonicalize()?;
            if !journal.starts_with(&root) {
                return Err(WorkspaceError::OutsideWorkspace(journal));
            }
        }
        Ok(Self { root, journal })
    }

    /// 返回规范化根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 无损读取既有文本文件，并生成基于原始字节的版本。
    pub fn read(&self, path: impl AsRef<Path>) -> Result<TextSnapshot, WorkspaceError> {
        let path = self.resolve(path.as_ref())?;
        let bytes = fs::read(&path)?;
        let (text, encoding) = decode(&bytes)?;
        Ok(TextSnapshot {
            path,
            version: digest(&bytes),
            text,
            encoding,
        })
    }

    /// 按快照版本替换文件，保留原编码和统一 CRLF 换行风格。
    ///
    /// 发生版本冲突时不写文件。若替换后更新日志失败，Prepared 日志仍可通过 inspect 核对。
    pub fn replace(
        &self,
        snapshot: &TextSnapshot,
        text: &str,
    ) -> Result<ChangeRecord, WorkspaceError> {
        let _lease = self.lease()?;
        let path = self.resolve(&snapshot.path)?;
        let before = fs::read(&path)?;
        if digest(&before) != snapshot.version {
            return Err(WorkspaceError::Conflict(path));
        }
        let (actual_text, encoding) = decode(&before)?;
        // 编码与换行根据当前已核对的字节决定，不信任调用方可修改的快照元数据。
        let normalized =
            if actual_text.contains("\r\n") && !actual_text.replace("\r\n", "").contains('\n') {
                text.replace("\r\n", "\n").replace('\n', "\r\n")
            } else {
                text.to_owned()
            };
        let after = encode(&normalized, encoding);
        let id = change_id();
        let mut record = ChangeRecord {
            format_version: 1,
            id,
            path: path
                .strip_prefix(&self.root)
                .map_err(|_| WorkspaceError::OutsideWorkspace(path.clone()))?
                .to_owned(),
            before: digest(&before),
            after: digest(&after),
            state: ChangeState::Prepared,
        };
        write_new(&self.backup_path(&record.id), &before)?;
        self.save(&record)?;
        if digest(&fs::read(&path)?) != record.before {
            return Err(WorkspaceError::Conflict(path));
        }
        atomic_write(&path, &after)?;
        record.state = ChangeState::Applied;
        self.save(&record)?;
        Ok(record)
    }

    /// 读取并核对日志；Prepared 状态通过实际内容判定是否已应用。
    ///
    /// 未知内容不被猜测或覆盖。此方法不改变日志和目标文件。
    pub fn inspect(&self, id: &str) -> Result<ChangeRecord, WorkspaceError> {
        validate_id(id)?;
        let mut record: ChangeRecord = serde_json::from_slice(&fs::read(self.record_path(id))?)
            .map_err(|error| WorkspaceError::InvalidJournal(error.to_string()))?;
        if record.format_version != 1
            || record.id != id
            || record.path.is_absolute()
            || record
                .path
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(WorkspaceError::InvalidJournal(
                "version, identity, or relative path is invalid".into(),
            ));
        }
        let path = self.resolve(&record.path)?;
        let actual = digest(&fs::read(&path)?);
        match record.state {
            ChangeState::Prepared if actual == record.after => record.state = ChangeState::Applied,
            ChangeState::Prepared if actual == record.before => {}
            ChangeState::Applied if actual == record.after => {}
            ChangeState::UndoPrepared if actual == record.before => {
                record.state = ChangeState::Undone
            }
            ChangeState::UndoPrepared if actual == record.after => {}
            ChangeState::Undone if actual == record.before => {}
            _ => return Err(WorkspaceError::Conflict(path)),
        }
        Ok(record)
    }

    /// 只撤销仍保持本变更结果的文件；后续用户修改会导致冲突而不会被覆盖。
    pub fn undo(&self, id: &str) -> Result<ChangeRecord, WorkspaceError> {
        let _lease = self.lease()?;
        let mut record = self.inspect(id)?;
        if !matches!(
            record.state,
            ChangeState::Applied | ChangeState::UndoPrepared
        ) {
            return Ok(record);
        }
        let before = fs::read(self.backup_path(id))?;
        if digest(&before) != record.before {
            return Err(WorkspaceError::InvalidJournal(
                "Backup content digest does not match".into(),
            ));
        }
        let path = self.resolve(&record.path)?;
        if digest(&fs::read(&path)?) != record.after {
            return Err(WorkspaceError::Conflict(path));
        }
        record.state = ChangeState::UndoPrepared;
        self.save(&record)?;
        atomic_write(&path, &before)?;
        record.state = ChangeState::Undone;
        self.save(&record)?;
        Ok(record)
    }

    /// 解析真实路径，阻止链接越界和对修改账本本身的工具编辑。
    fn resolve(&self, path: &Path) -> Result<PathBuf, WorkspaceError> {
        let candidate = if path.is_absolute() {
            path.to_owned()
        } else {
            self.root.join(path)
        };
        let candidate = candidate.canonicalize()?;
        if !candidate.starts_with(&self.root) || candidate.starts_with(&self.journal) {
            return Err(WorkspaceError::OutsideWorkspace(candidate));
        }
        Ok(candidate)
    }

    /// 取得跨 Workspace 实例的文件锁；File 丢弃时释放，不删除锁文件。
    fn lease(&self) -> Result<File, WorkspaceError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.journal.join("writer.lock"))?;
        file.try_lock().map_err(|_| WorkspaceError::Busy)?;
        Ok(file)
    }

    /// 原子更新一条修改记录。
    fn save(&self, record: &ChangeRecord) -> Result<(), WorkspaceError> {
        let bytes = serde_json::to_vec(record)
            .map_err(|error| WorkspaceError::InvalidJournal(error.to_string()))?;
        atomic_write(&self.record_path(&record.id), &bytes)?;
        Ok(())
    }

    /// 返回已经验证的记录路径。
    fn record_path(&self, id: &str) -> PathBuf {
        self.journal.join(format!("{id}.json"))
    }

    /// 返回已经验证的备份路径。
    fn backup_path(&self, id: &str) -> PathBuf {
        self.journal.join(format!("{id}.before"))
    }
}

/// 修改身份不能控制文件路径。
fn validate_id(id: &str) -> Result<(), WorkspaceError> {
    if id.is_empty()
        || id.len() > 96
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Err(WorkspaceError::InvalidJournal(
            "modification ID is invalid".into(),
        ));
    }
    Ok(())
}

/// 生成跨进程区分的日志身份；创建文件仍使用 create_new 检查碰撞。
fn change_id() -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{:x}-{stamp:x}-{:x}",
        std::process::id(),
        NEXT_CHANGE.fetch_add(1, Ordering::Relaxed)
    )
}

/// 内容版本只依赖原始字节，不依赖文件时间戳。
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// 创建不会覆盖已有文件的持久数据。
fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// 同目录暂存并替换单个文件，保留目标文件的基本权限。
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = path.with_file_name(format!(".teal-{}.tmp", change_id()));
    // 只有成功取得新文件所有权后才允许清理它，不能删除碰撞的已有文件。
    let mut temporary = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| {
        temporary.write_all(bytes)?;
        temporary.sync_all()?;
        if let Ok(metadata) = fs::metadata(path) {
            if metadata.permissions().readonly() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "target file is read-only",
                ));
            }
            fs::set_permissions(&temp, metadata.permissions())?;
        }
        Ok(())
    })();
    drop(temporary);
    let result = result.and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// 解码支持的文本，拒绝无 BOM 的 UTF-16 和无效码点。
fn decode(bytes: &[u8]) -> Result<(String, TextEncoding), WorkspaceError> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        if !bytes.len().is_multiple_of(2) {
            return Err(WorkspaceError::InvalidEncoding);
        }
        let little = bytes[0] == 0xff;
        let units = bytes[2..].as_chunks::<2>().0.iter().map(|pair| {
            if little {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        });
        let text = char::decode_utf16(units)
            .collect::<Result<String, _>>()
            .map_err(|_| WorkspaceError::InvalidEncoding)?;
        if text.contains('\0') {
            return Err(WorkspaceError::InvalidEncoding);
        }
        Ok((
            text,
            if little {
                TextEncoding::Utf16Le
            } else {
                TextEncoding::Utf16Be
            },
        ))
    } else {
        let bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
        let content = if bom { &bytes[3..] } else { bytes };
        let text = std::str::from_utf8(content).map_err(|_| WorkspaceError::InvalidEncoding)?;
        if text.contains('\0') {
            return Err(WorkspaceError::InvalidEncoding);
        }
        Ok((
            text.to_owned(),
            if bom {
                TextEncoding::Utf8Bom
            } else {
                TextEncoding::Utf8
            },
        ))
    }
}

/// 按原编码输出字节，不进行有损替换。
fn encode(text: &str, encoding: TextEncoding) -> Vec<u8> {
    let mut bytes = match encoding {
        TextEncoding::Utf8 => Vec::new(),
        TextEncoding::Utf8Bom => vec![0xef, 0xbb, 0xbf],
        TextEncoding::Utf16Le => vec![0xff, 0xfe],
        TextEncoding::Utf16Be => vec![0xfe, 0xff],
    };
    match encoding {
        TextEncoding::Utf8 | TextEncoding::Utf8Bom => bytes.extend_from_slice(text.as_bytes()),
        _ => {
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&if encoding == TextEncoding::Utf16Le {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
        }
    }
    bytes
}

#[cfg(test)]
mod tests;
