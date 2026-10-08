//! 以基础内容哈希绑定文档的追加式恢复日志。
//!
//! 每条记录保存顺序号、事务与校验和；读取只验证记录，不自行修改文档。
//! 日志创建会截断旧文件，必须由上层在确认基线与恢复策略后调用。

use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::Transaction;

const JOURNAL_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct JournalHeader {
    version: u16,
    base_hash: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct JournalPayload {
    sequence: u64,
    transaction: Transaction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct JournalRecord {
    payload: JournalPayload,
    checksum: [u8; 32],
}

/// 损坏、不兼容或无法访问的恢复日志。
#[derive(Debug, Error)]
pub enum JournalError {
    /// 文件系统访问失败。
    #[error("Recovery log reading and writing failed: {0}")]
    Io(#[from] std::io::Error),

    /// JSON 解码失败。
    #[error("Recovery log format error: {0}")]
    Json(#[from] serde_json::Error),

    /// 不支持磁盘上的日志版本。
    #[error("Unsupported recovery log version {0}")]
    Version(u16),

    /// 序列顺序或记录校验和无效。
    #[error("Recovery log integrity check failed at record {0}")]
    Integrity(u64),
}

/// 用于单个未保存缓冲区、仅追加且带校验和的事务日志。
pub struct RecoveryJournal {
    path: PathBuf,
    file: File,
    next_sequence: u64,
    base_hash: [u8; 32],
}

impl RecoveryJournal {
    /// 为已知磁盘基线创建或截断日志。
    ///
    /// 此操作会覆盖已有日志，调用方必须先完成恢复决策；不能为了读取旧记录而调用它。
    /// 头部刷新成功后才返回写入器，后续记录从序号零开始。
    pub fn create(path: impl AsRef<Path>, base_hash: [u8; 32]) -> Result<Self, JournalError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .read(true)
            .open(&path)?;
        serde_json::to_writer(
            &mut file,
            &JournalHeader {
                version: JOURNAL_VERSION,
                base_hash,
            },
        )?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        Ok(Self {
            path,
            file,
            next_sequence: 0,
            base_hash,
        })
    }

    /// 追加一个完整事务并刷新到稳定存储。
    ///
    /// 校验和覆盖序号与事务的序列化内容，刷新成功后才推进内存序号。
    /// 写入失败可能留下不完整尾部，本方法不会截断或修补它；调用方必须保留并报告错误。
    pub fn append(&mut self, transaction: &Transaction) -> Result<(), JournalError> {
        let payload = JournalPayload {
            sequence: self.next_sequence,
            transaction: transaction.clone(),
        };
        let encoded = serde_json::to_vec(&payload)?;
        let record = JournalRecord {
            payload,
            checksum: Sha256::digest(&encoded).into(),
        };

        serde_json::to_writer(&mut self.file, &record)?;
        self.file.write_all(b"\n")?;
        self.file.sync_data()?;

        // 完整记录持久化之后才能推进序号。
        self.next_sequence = self.next_sequence.wrapping_add(1);
        Ok(())
    }

    /// 读取并验证日志而不更改它。
    ///
    /// 验证头部版本、每条序号和校验和，任何损坏都会返回错误，不静默接受有效前缀。
    /// 返回基础哈希及事务序列；调用方仍须匹配文档基线、校验事务边界并决定是否重放。
    pub fn replay(path: impl AsRef<Path>) -> Result<([u8; 32], Vec<Transaction>), JournalError> {
        let file = File::open(path)?;
        let mut lines = BufReader::new(file).lines();
        let header: JournalHeader =
            serde_json::from_str(&lines.next().transpose()?.unwrap_or_default())?;
        if header.version != JOURNAL_VERSION {
            return Err(JournalError::Version(header.version));
        }

        let mut transactions = Vec::new();
        for (expected, line) in lines.enumerate() {
            let record: JournalRecord = serde_json::from_str(&line?)?;
            let encoded = serde_json::to_vec(&record.payload)?;
            if record.payload.sequence != expected as u64
                || <[u8; 32]>::from(Sha256::digest(encoded)) != record.checksum
            {
                return Err(JournalError::Integrity(expected as u64));
            }
            transactions.push(record.payload.transaction);
        }

        // 调用方仍须匹配基础哈希，之后才能重放这些编辑。
        Ok((header.base_hash, transactions))
    }

    /// 返回日志路径。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 返回头部记录的基线内容哈希。
    #[must_use]
    pub const fn base_hash(&self) -> [u8; 32] {
        self.base_hash
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::Transaction;

    use super::{JournalError, RecoveryJournal};

    #[test]
    fn replays_ordered_transactions_and_rejects_tampering() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("buffer.journal");
        let mut journal = RecoveryJournal::create(&path, [7; 32]).expect("Create log");
        journal
            .append(&Transaction::insert(0, "hello"))
            .expect("Append first record");
        journal
            .append(&Transaction::insert(5, "boundary"))
            .expect("Append second record");
        drop(journal);

        let (hash, transactions) = RecoveryJournal::replay(&path).expect("Replay log");
        assert_eq!(hash, [7; 32]);
        assert_eq!(transactions.len(), 2);

        let mut bytes = fs::read(&path).expect("read log");
        let last = bytes.len() - 3;
        bytes[last] ^= 1;
        fs::write(&path, bytes).expect("Write tampered data");
        assert!(matches!(
            RecoveryJournal::replay(path),
            Err(JournalError::Json(_) | JournalError::Integrity(_))
        ));
    }
}
