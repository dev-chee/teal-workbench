//! 语言服务器路由与每个 URI 的有序文档同步。
//!
//! 版本锁覆盖通知写入与版本登记；只有旧修订连续且服务器支持增量时才发送局部变化。
//! 出现修订断层时发送完整快照，防止服务器在错误基础上应用增量。

use crate::{LineIndex, LspClient, LspError};
use serde_json::json;
use std::{collections::HashMap, path::Path, sync::Arc};
use tokio::sync::Mutex;

/// 一次已提交文本快照之间的最小 UTF-8 变化。
///
/// 该类型只描述协议同步所需的快照、修订和字节范围，不依赖任何编辑器的事务或缓冲区
/// 模型。调用方必须保证范围属于 `old_text`，并在调用 [`ManagedServer::change`] 前保留
/// 相关修订的顺序。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextDocumentChange {
    /// 用于协议坐标转换的旧文本快照。
    pub old_text: String,

    /// 已提交的文本快照。
    pub new_text: String,

    /// 前一文档修订。
    pub old_revision: u64,

    /// 已提交的文档修订。
    pub revision: u64,

    /// 旧快照中被替换的 UTF-8 字节范围。
    pub old_byte_range: std::ops::Range<usize>,

    /// 替换的 UTF-8 文本。
    pub inserted: String,
}

impl TextDocumentChange {
    /// 构建两个快照之间保持 Unicode 边界的最小差异。
    #[must_use]
    pub fn between(old: &str, new: &str, old_revision: u64, revision: u64) -> Self {
        let prefix = old
            .chars()
            .zip(new.chars())
            .take_while(|(left, right)| left == right)
            .map(|(character, _)| character.len_utf8())
            .sum::<usize>();
        let suffix = old[prefix..]
            .chars()
            .rev()
            .zip(new[prefix..].chars().rev())
            .take_while(|(left, right)| left == right)
            .map(|(character, _)| character.len_utf8())
            .sum::<usize>();
        let old_end = old.len() - suffix;
        Self {
            old_text: old.to_owned(),
            new_text: new.to_owned(),
            old_revision,
            revision,
            old_byte_range: prefix..old_end,
            inserted: new[prefix..new.len() - suffix].to_owned(),
        }
    }
}

/// 语言服务器及其有序文档同步状态。
pub struct ManagedServer {
    /// 用于路由缓冲区的语言标识。
    pub language: String,

    /// 已协商的协议客户端。
    pub client: Arc<LspClient>,
    versions: Mutex<HashMap<String, u64>>,
}

impl ManagedServer {
    /// 按捕获快照补齐服务器文档，不倒退已发送的版本。
    ///
    /// 版本锁保持到通知写入完成才登记新修订，防止并发请求重排文档同步。
    /// 服务器已处于相同或更新版本时不再发送旧文本；原请求是否仍有效由根应用另行核对。
    pub async fn synchronize(&self, uri: &str, revision: u64, text: &str) -> Result<(), LspError> {
        let mut versions = self.versions.lock().await;
        match versions.get(uri).copied() {
            None => {
                self.client
                    .did_open(uri, &self.language, version(revision), text)
                    .await?;
            }
            Some(previous) if previous < revision => {
                self.client
                    .did_change(uri, version(revision), json!([{"text":text}]))
                    .await?;
            }
            Some(_) => return Ok(()),
        }
        versions.insert(uri.to_owned(), revision);
        Ok(())
    }

    /// 每个 URI 仅发布一次初始快照。
    pub async fn open(&self, uri: &str, revision: u64, text: &str) -> Result<(), LspError> {
        let mut versions = self.versions.lock().await;
        if versions.contains_key(uri) {
            return Ok(());
        }
        self.client
            .did_open(uri, &self.language, version(revision), text)
            .await?;
        versions.insert(uri.to_owned(), revision);
        Ok(())
    }

    /// 同步已提交变化，忽略旧修订并在版本断层后发送完整快照。
    ///
    /// 只有服务器支持增量、已发送版本等于变化的旧修订且坐标可转换时才使用范围编辑。
    /// 通知写入失败不会推进本地版本记录，后续调用仍可重新同步。
    pub async fn change(&self, uri: &str, change: &TextDocumentChange) -> Result<(), LspError> {
        let mut versions = self.versions.lock().await;
        let Some(previous) = versions.get(uri).copied() else {
            self.client
                .did_open(
                    uri,
                    &self.language,
                    version(change.revision),
                    &change.new_text,
                )
                .await?;
            versions.insert(uri.to_owned(), change.revision);
            return Ok(());
        };
        if previous >= change.revision {
            return Ok(());
        }
        let index = LineIndex::new(change.old_text.clone());
        let sync = self.client.capabilities().get("textDocumentSync");
        let incremental = sync.and_then(|v| {
            v.as_u64()
                .or_else(|| v.get("change").and_then(serde_json::Value::as_u64))
        }) == Some(2);
        // 字节范围属于旧快照；若服务端错过一个版本，即使支持增量也不能沿用该范围。
        let changes = if incremental && previous == change.old_revision {
            match (
                index.position(change.old_byte_range.start),
                index.position(change.old_byte_range.end),
            ) {
                (Some((sl, sc)), Some((el, ec))) => {
                    json!([{"range":{"start":{"line":sl,"character":sc},"end":{"line":el,"character":ec}},"text":change.inserted}])
                }
                _ => json!([{"text":change.new_text}]),
            }
        } else {
            json!([{"text":change.new_text}])
        };
        self.client
            .did_change(uri, version(change.revision), changes)
            .await?;
        versions.insert(uri.to_owned(), change.revision);
        Ok(())
    }

    /// 关闭已同步的协议文档。
    pub async fn close(&self, uri: &str) -> Result<(), LspError> {
        let mut versions = self.versions.lock().await;
        if versions.remove(uri).is_some() {
            self.client.did_close(uri).await?;
        }
        Ok(())
    }
}

fn version(revision: u64) -> i32 {
    i32::try_from(revision).unwrap_or(i32::MAX)
}

/// 独立于面板和终端 UI、按语言路由的服务器集合。
#[derive(Default)]
pub struct LspManager {
    servers: Vec<Arc<ManagedServer>>,
}

impl LspManager {
    /// 登记已初始化的语言服务器。
    pub fn insert(&mut self, language: String, client: Arc<LspClient>) -> Arc<ManagedServer> {
        let server = Arc::new(ManagedServer {
            language,
            client,
            versions: Mutex::new(HashMap::new()),
        });
        self.servers.push(server.clone());
        server
    }

    /// 查找与文件扩展名匹配的最新服务器。
    pub fn for_path(&self, path: &Path) -> Option<Arc<ManagedServer>> {
        let language = match path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
        {
            "rs" => "rust",
            "tl" | "teal" => "teal",
            other => other,
        };
        self.servers
            .iter()
            .rev()
            .find(|s| s.language == language)
            .cloned()
    }

    /// 遍历登记的服务器以执行关闭。
    pub fn iter(&self) -> impl Iterator<Item = &Arc<ManagedServer>> {
        self.servers.iter()
    }
}
