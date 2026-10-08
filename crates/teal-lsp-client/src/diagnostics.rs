//! 按文档 URI 保存的线程安全诊断快照。
//!
//! 原始 UTF-16 范围始终保留，只有拿到相应文本且坐标有效时才生成字节范围。
//! 文档编辑、关闭或重命名后的失效操作由根应用驱动。

use crate::LineIndex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

/// 编辑器渲染与自动化共享的带版本诊断。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    /// 文件 URI。
    pub uri: String,

    /// 关联的文档修订（若已报告）。
    pub revision: Option<u64>,

    /// LSP 严重度：1 错误，2 警告，3 信息，4 提示。
    pub severity: u64,

    /// 诊断说明。
    pub message: String,

    /// 产生诊断的服务器或分析器。
    pub source: Option<String>,

    /// 原始 LSP UTF-16 范围。
    pub range_utf16: Value,

    /// 源码可用时经过校验的 UTF-8 字节范围。
    pub range_utf8: Option<std::ops::Range<usize>>,
}

/// 按 URI 索引的线程安全诊断快照。
#[derive(Clone, Default)]
pub struct DiagnosticStore {
    entries: Arc<RwLock<BTreeMap<String, Vec<Diagnostic>>>>,
}

impl DiagnosticStore {
    /// 在文档编辑、重命名或关闭后使其诊断失效。
    pub fn remove(&self, uri: &str) {
        if let Ok(mut entries) = self.entries.write() {
            entries.remove(uri);
        }
    }

    /// 替换一个 URI 的诊断，拒绝显式过期版本。
    pub fn update(
        &self,
        params: &Value,
        source: Option<&str>,
        current_revision: Option<u64>,
    ) -> bool {
        let Some(uri) = params.get("uri").and_then(Value::as_str) else {
            return false;
        };
        let revision = params.get("version").and_then(Value::as_u64);
        if revision
            .zip(current_revision)
            .is_some_and(|(reported, current)| reported < current)
        {
            return false;
        }
        let index = source.map(LineIndex::new);
        let records = params
            .get("diagnostics")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|item| {
                let range = item.get("range").cloned().unwrap_or(Value::Null);
                let bytes = index.as_ref().and_then(|index| {
                    let start = index.offset(
                        range.pointer("/start/line")?.as_u64()?,
                        range.pointer("/start/character")?.as_u64()?,
                    )?;
                    let end = index.offset(
                        range.pointer("/end/line")?.as_u64()?,
                        range.pointer("/end/character")?.as_u64()?,
                    )?;
                    (start <= end).then_some(start..end)
                });
                Diagnostic {
                    uri: uri.to_owned(),
                    revision,
                    severity: item.get("severity").and_then(Value::as_u64).unwrap_or(1),
                    message: item
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    source: item
                        .get("source")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    range_utf16: range,
                    range_utf8: bytes,
                }
            })
            .collect();
        let Ok(mut entries) = self.entries.write() else {
            return false;
        };
        entries.insert(uri.to_owned(), records);
        true
    }

    /// 复制当前记录，可限制为单个 URI。
    pub fn snapshot(&self, uri: Option<&str>) -> Vec<Diagnostic> {
        self.entries
            .read()
            .map(|entries| {
                entries
                    .iter()
                    .filter(|(key, _)| uri.is_none_or(|uri| uri == key.as_str()))
                    .flat_map(|(_, v)| v.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}
