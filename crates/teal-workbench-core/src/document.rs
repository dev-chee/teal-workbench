//! 带编码、磁盘基线和撤销历史的文档。
//!
//! 内存文本统一使用 LF 与 Unicode 字符偏移，保存时恢复检测到的编码和行尾。
//! 成功持久化才推进磁盘基线与已保存历史标记；工作区授权及覆盖确认由调用方承担。

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use ropey::Rope;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::{SelectionSet, Transaction, TransactionError, UndoTree};

/// 在编辑和保存期间保留的磁盘文本表示。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextEncoding {
    /// 不带字节顺序标记的 UTF-8。
    #[default]
    Utf8,

    /// 带 `EF BB BF` 标记的 UTF-8。
    Utf8Bom,

    /// 带标记的小端 UTF-16。
    Utf16Le,

    /// 带标记的大端 UTF-16。
    Utf16Be,

    /// 以只读方式显示的不可解码或二进制内容。
    Binary,
}

/// 为新输出行保留的原始行终止符。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineEnding {
    /// 换行符。
    #[default]
    Lf,

    /// 回车后跟换行。
    CrLf,

    /// 单独的回车。
    Cr,
}

impl LineEnding {
    const fn text(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
            Self::Cr => "\r",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    hash: [u8; 32],
}

/// 解码或打开文档时的失败。
#[derive(Debug, Error)]
pub enum DocumentError {
    /// 文件系统访问失败。
    #[error("Unable to open document: {0}")]
    Io(#[from] std::io::Error),

    /// UTF-16 输入的字节数或代理项序列无效。
    #[error("Invalid UTF-16 document: {0}")]
    InvalidUtf16(String),
}

/// 持久化文档时的失败。
#[derive(Debug, Error)]
pub enum SaveError {
    /// 只读二进制文档不能作为文本写入。
    #[error("Binary documents are read-only")]
    ReadOnly,

    /// 文件在读取本文档基线后发生了变化。
    #[error("The file has been externally modified since it was opened")]
    ChangedOnDisk,

    /// 未命名缓冲区需要显式路径。
    #[error("Document has no path")]
    MissingPath,

    /// 文件系统持久化失败。
    #[error("Unable to save document: {0}")]
    Io(#[from] std::io::Error),
}

/// 包含可编辑 Rope、选择区、编码和分支保留历史的文档。
#[derive(Clone)]
pub struct Document {
    path: Option<PathBuf>,
    text: Rope,
    encoding: TextEncoding,
    line_ending: LineEnding,
    read_only: bool,
    selections: SelectionSet,
    history: UndoTree,
    saved_history_node: usize,
    revision: u64,
    baseline: Option<FileStamp>,
}

impl Document {
    /// 打开文本或只读二进制文档。
    ///
    /// 读取原始字节并保存内容哈希，检测编码与行尾后将内存换行归一化为 LF。
    /// 二进制内容以只读预览表示；文件读取或 UTF-16 解码失败会返回错误，不创建半初始化文档。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DocumentError> {
        let path = path.as_ref().to_path_buf();
        let bytes = fs::read(&path)?;
        let stamp = FileStamp {
            hash: digest(&bytes),
        };

        // 磁盘哈希与输出格式独立于内存中归一化的文本。
        let (decoded, encoding, read_only) = decode(&bytes)?;
        let line_ending = detect_line_ending(&decoded);
        let normalized = normalize_line_endings(&decoded);

        Ok(Self {
            path: Some(path),
            text: Rope::from_str(&normalized),
            encoding,
            line_ending,
            read_only,
            selections: SelectionSet::default(),
            history: UndoTree::default(),
            saved_history_node: 0,
            revision: 0,
            baseline: Some(stamp),
        })
    }

    /// 创建可编辑的未命名 UTF-8 文档。
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        let line_ending = detect_line_ending(text);
        Self {
            path: None,
            text: Rope::from_str(&normalize_line_endings(text)),
            encoding: TextEncoding::Utf8,
            line_ending,
            read_only: false,
            selections: SelectionSet::default(),
            history: UndoTree::default(),
            saved_history_node: 0,
            revision: 0,
            baseline: None,
        }
    }

    /// 返回当前 Rope。
    #[must_use]
    pub const fn text(&self) -> &Rope {
        &self.text
    }

    /// 返回命名文档的后备路径。
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// 返回检测到的输出编码。
    #[must_use]
    pub const fn encoding(&self) -> TextEncoding {
        self.encoding
    }

    /// 返回保留的输出行尾。
    #[must_use]
    pub const fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// 返回是否禁用编辑与保存。
    #[must_use]
    pub const fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// 返回历史是否与最近一次成功保存不同。
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.history.current() != self.saved_history_node
    }

    /// 返回单调递增的内存修订号。
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// 返回当前规范化选择区。
    #[must_use]
    pub const fn selections(&self) -> &SelectionSet {
        &self.selections
    }

    /// 替换选择区并将其限制在当前 Rope 范围内。
    pub fn set_selections(&mut self, mut selections: SelectionSet) {
        selections.clamp(self.text.len_chars());
        self.selections = selections;
    }

    /// 将插入文本的行尾规范化为 LF 后应用并记录事务。
    ///
    /// 事务范围相对于调用前的文本，单位是 Unicode 字符。只读文档和空事务返回成功但不改变修订；
    /// 有效编辑创建撤销分支、限制选择区端点并推进修订，尚未写入磁盘或恢复日志。
    pub fn apply(&mut self, transaction: Transaction) -> Result<(), TransactionError> {
        if self.read_only || transaction.is_empty() {
            return Ok(());
        }

        let transaction = transaction.normalized()?;
        let inverse = transaction.apply(&mut self.text)?;

        self.history.record(transaction, inverse);
        self.selections.clamp(self.text.len_chars());
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }

    /// 应用当前节点的逆事务。
    ///
    /// 没有父历史节点时返回 `Ok(false)`；成功撤销仍推进内存修订，修订不会倒退到旧值。
    /// 是否回到已保存状态由历史节点身份判断，不能用修订号相等代替。
    pub fn undo(&mut self) -> Result<bool, TransactionError> {
        let Some(transaction) = self.history.undo() else {
            return Ok(false);
        };
        transaction.apply(&mut self.text)?;
        self.selections.clamp(self.text.len_chars());
        self.revision = self.revision.wrapping_add(1);
        Ok(true)
    }

    /// 从当前历史节点重放一个分支。
    pub fn redo(&mut self, branch: usize) -> Result<bool, TransactionError> {
        let Some(transaction) = self.history.redo(branch) else {
            return Ok(false);
        };
        transaction.apply(&mut self.text)?;
        self.selections.clamp(self.text.len_chars());
        self.revision = self.revision.wrapping_add(1);
        Ok(true)
    }

    /// 检查基线哈希后保存到现有路径。
    pub fn save(&mut self) -> Result<(), SaveError> {
        let path = self.path.clone().ok_or(SaveError::MissingPath)?;
        self.save_to(path)
    }

    /// 使用同目录原子替换保存到显式路径。
    ///
    /// 恢复文档保留的编码和行尾，刷新临时文件后才替换目标；成功后才更新路径、基线和已保存标记。
    /// 仅保存到同一已知路径且能读取当前字节时比较旧基线；另存为目标的覆盖确认与工作区检查由调用方负责。
    /// 只读状态、检测到外部变化或文件操作失败均返回 [`SaveError`]。
    pub fn save_to(&mut self, path: impl AsRef<Path>) -> Result<(), SaveError> {
        if self.read_only {
            return Err(SaveError::ReadOnly);
        }

        let path = path.as_ref();
        if self.path.as_deref() == Some(path)
            && let Some(baseline) = &self.baseline
            && let Ok(current) = fs::read(path)
            && digest(&current) != baseline.hash
        {
            return Err(SaveError::ChangedOnDisk);
        }

        // 临时文件与目标位于同一目录，确保保存时可原子替换。
        let bytes = self.encoded_bytes();
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let mut temporary = NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file_mut().sync_all()?;
        temporary
            .persist(path)
            .map_err(|error| SaveError::Io(error.error))?;

        // 写入失败时不得改变磁盘基线和未保存历史标记。
        self.path = Some(path.to_path_buf());
        self.baseline = Some(FileStamp {
            hash: digest(&bytes),
        });
        self.saved_history_node = self.history.current();

        Ok(())
    }

    pub(crate) fn relocate(&mut self, path: Option<PathBuf>) {
        if path.is_none() {
            self.baseline = None;
            self.saved_history_node = usize::MAX;
        }
        self.path = path;
    }

    fn encoded_bytes(&self) -> Vec<u8> {
        let text = if self.line_ending == LineEnding::Lf {
            self.text.to_string()
        } else {
            self.text.to_string().replace('\n', self.line_ending.text())
        };
        match self.encoding {
            TextEncoding::Utf8 => text.into_bytes(),
            TextEncoding::Utf8Bom => [b"\xef\xbb\xbf".as_slice(), text.as_bytes()].concat(),
            TextEncoding::Utf16Le => encode_utf16(&text, true),
            TextEncoding::Utf16Be => encode_utf16(&text, false),
            TextEncoding::Binary => Vec::new(),
        }
    }
}

impl std::fmt::Display for Document {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for chunk in self.text.chunks() {
            formatter.write_str(chunk)?;
        }
        Ok(())
    }
}

fn decode(bytes: &[u8]) -> Result<(String, TextEncoding, bool), DocumentError> {
    if let Some(contents) = bytes.strip_prefix(b"\xef\xbb\xbf") {
        return match std::str::from_utf8(contents) {
            Ok(text) => Ok((text.to_owned(), TextEncoding::Utf8Bom, false)),
            Err(_) => Ok((binary_preview(bytes), TextEncoding::Binary, true)),
        };
    }
    if let Some(contents) = bytes.strip_prefix(&[0xff, 0xfe]) {
        return decode_utf16(contents, true).map(|text| (text, TextEncoding::Utf16Le, false));
    }
    if let Some(contents) = bytes.strip_prefix(&[0xfe, 0xff]) {
        return decode_utf16(contents, false).map(|text| (text, TextEncoding::Utf16Be, false));
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok((text.to_owned(), TextEncoding::Utf8, false)),
        Err(_) => Ok((binary_preview(bytes), TextEncoding::Binary, true)),
    }
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> Result<String, DocumentError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(DocumentError::InvalidUtf16("byte length is odd".to_owned()));
    }
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let pair = [pair[0], pair[1]];
            if little_endian {
                u16::from_le_bytes(pair)
            } else {
                u16::from_be_bytes(pair)
            }
        })
        .collect::<Vec<_>>();
    String::from_utf16(&units).map_err(|error| DocumentError::InvalidUtf16(error.to_string()))
}

fn encode_utf16(text: &str, little_endian: bool) -> Vec<u8> {
    let mut bytes = if little_endian {
        vec![0xff, 0xfe]
    } else {
        vec![0xfe, 0xff]
    };
    for unit in text.encode_utf16() {
        let encoded = if little_endian {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        };
        bytes.extend_from_slice(&encoded);
    }
    bytes
}

fn detect_line_ending(text: &str) -> LineEnding {
    let crlf = text.matches("\r\n").count();
    let cr = text.matches('\r').count().saturating_sub(crlf);
    let lf = text.matches('\n').count().saturating_sub(crlf);
    if crlf >= cr && crlf >= lf && crlf > 0 {
        LineEnding::CrLf
    } else if cr > lf && cr > 0 {
        LineEnding::Cr
    } else {
        LineEnding::Lf
    }
}

fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn binary_preview(bytes: &[u8]) -> String {
    let shown = bytes.len().min(256);
    let mut preview = format!("<Binary file: {} bytes>\n", bytes.len());
    for (row, chunk) in bytes[..shown].chunks(16).enumerate() {
        use std::fmt::Write as _;
        let _ = write!(preview, "{:08x}  ", row * 16);
        for byte in chunk {
            let _ = write!(preview, "{byte:02x} ");
        }
        preview.push('\n');
    }
    preview
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::Transaction;

    use super::{Document, LineEnding, SaveError, TextEncoding};

    #[test]
    fn preserves_utf16_and_crlf_while_editing_in_utf8_rope() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("sample.txt");
        let mut bytes = vec![0xff, 0xfe];
        for unit in "\u{4e00}\r\n\u{4e8c}".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        fs::write(&path, bytes).expect("Write test data");

        let mut document = Document::open(&path).expect("Should be able to open UTF-16 documents");
        assert_eq!(document.encoding(), TextEncoding::Utf16Le);
        assert_eq!(document.line_ending(), LineEnding::CrLf);
        assert_eq!(document.to_string(), "\u{4e00}\n\u{4e8c}");
        document
            .apply(Transaction::insert(1, "\u{754c}"))
            .expect("Should be able to apply editing");
        document.save().expect("should save successfully");

        let reopened =
            Document::open(&path).expect("You should be able to reopen the saved document");
        assert_eq!(reopened.to_string(), "\u{4e00}\u{754c}\n\u{4e8c}");
        assert_eq!(reopened.encoding(), TextEncoding::Utf16Le);
    }

    #[test]
    fn detects_external_changes_and_opens_binary_read_only() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("sample.txt");
        fs::write(&path, "first").expect("Write test data");
        let mut document = Document::open(&path).expect("should be able to open text");
        document
            .apply(Transaction::insert(5, " local"))
            .expect("Should be able to apply editing");
        fs::write(&path, "external").expect("external write");
        assert!(matches!(document.save(), Err(SaveError::ChangedOnDisk)));

        let binary_path = directory.path().join("sample.bin");
        fs::write(&binary_path, [0xff, 0x00, 0x80]).expect("Write binary data");
        let binary = Document::open(binary_path)
            .expect("Should be able to open binary files in preview mode");
        assert!(binary.is_read_only());
        assert_eq!(binary.encoding(), TextEncoding::Binary);
    }
}
