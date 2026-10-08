//! 将 LSP 工作区编辑解析为类型化文本和资源操作。
//!
//! 文本坐标相对于调用方提供的旧快照转换；此层不写磁盘、不修改打开缓冲区，也不授予工作区权限。
//! 执行顺序、修订核对和资源授权由嵌入应用协调器负责。

use std::{collections::HashMap, path::PathBuf};

use serde_json::Value;
use thiserror::Error;
use url::Url;

use crate::LineIndex;

/// 针对一个文档版本的文本编辑。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentEdit {
    /// 文档 URI。
    pub uri: String,

    /// 可选 LSP 文档版本。
    pub version: Option<i32>,

    /// 使用原始文本 Unicode 字符偏移的非重叠编辑。
    pub edits: Vec<TextEdit>,
}

/// 针对原始文本 Unicode 字符坐标的一次替换。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    /// 被替换的字符范围。
    pub range: std::ops::Range<usize>,

    /// 要插入的文本。
    pub insert: String,
}

/// 一个 LSP 工作区编辑操作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceOperation {
    /// 应用文本更改。
    Edit(DocumentEdit),

    /// 创建文件。
    Create(PathBuf),

    /// 重命名或移动文件。
    Rename {
        /// 现有路径。
        old: PathBuf,

        /// 新路径。
        new: PathBuf,
    },

    /// 根据服务器选项删除文件或目录。
    Delete(PathBuf),
}

/// 格式错误的编辑或不可用的文档快照。
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum WorkspaceEditError {
    /// 预期字段或 URI 无效。
    #[error("LSP workspace editing format error: {0}")]
    Malformed(String),

    /// 文本编辑指向没有已知快照的文档。
    #[error("`{0}` has no source code snapshot")]
    MissingDocument(String),

    /// 文本编辑在坐标转换后重叠。
    #[error("LSP text edits overlap with each other")]
    Overlap,
}

/// 解析 `changes` 和有序 `documentChanges` 两种工作区编辑形式。
pub fn parse_workspace_edit<S: std::hash::BuildHasher>(
    value: &Value,
    documents: &HashMap<String, String, S>,
) -> Result<Vec<WorkspaceOperation>, WorkspaceEditError> {
    let mut operations = Vec::new();
    if let Some(changes) = value.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            operations.push(WorkspaceOperation::Edit(parse_document_edits(
                uri, None, edits, documents,
            )?));
        }
    }
    if let Some(changes) = value.get("documentChanges").and_then(Value::as_array) {
        for change in changes {
            if let Some(kind) = change.get("kind").and_then(Value::as_str) {
                match kind {
                    "create" => {
                        operations.push(WorkspaceOperation::Create(uri_path(change.get("uri"))?));
                    }
                    "rename" => operations.push(WorkspaceOperation::Rename {
                        old: uri_path(change.get("oldUri"))?,
                        new: uri_path(change.get("newUri"))?,
                    }),
                    "delete" => {
                        operations.push(WorkspaceOperation::Delete(uri_path(change.get("uri"))?));
                    }
                    _ => {
                        return Err(WorkspaceEditError::Malformed(format!(
                            "Unknown resource operation `{kind}`"
                        )));
                    }
                }
            } else {
                let document = change.get("textDocument").ok_or_else(|| {
                    WorkspaceEditError::Malformed("textDocument is missing".to_owned())
                })?;
                let uri = document.get("uri").and_then(Value::as_str).ok_or_else(|| {
                    WorkspaceEditError::Malformed("Missing document URI".to_owned())
                })?;
                let version = document
                    .get("version")
                    .and_then(Value::as_i64)
                    .and_then(|value| i32::try_from(value).ok());
                operations.push(WorkspaceOperation::Edit(parse_document_edits(
                    uri,
                    version,
                    change.get("edits").unwrap_or(&Value::Null),
                    documents,
                )?));
            }
        }
    }
    Ok(operations)
}

fn parse_document_edits<S: std::hash::BuildHasher>(
    uri: &str,
    version: Option<i32>,
    edits: &Value,
    documents: &HashMap<String, String, S>,
) -> Result<DocumentEdit, WorkspaceEditError> {
    let source = documents
        .get(uri)
        .ok_or_else(|| WorkspaceEditError::MissingDocument(uri.to_owned()))?;
    let index = LineIndex::new(source.clone());
    let edits = edits
        .as_array()
        .ok_or_else(|| WorkspaceEditError::Malformed("Text edit must be an array".to_owned()))?
        .iter()
        .map(|edit| {
            let range = edit
                .get("range")
                .ok_or_else(|| WorkspaceEditError::Malformed("Missing edit scope".to_owned()))?;
            let start = position(range.get("start"), &index)?;
            let end = position(range.get("end"), &index)?;
            let new_text = edit
                .get("newText")
                .and_then(Value::as_str)
                .ok_or_else(|| WorkspaceEditError::Malformed("newText is missing".to_owned()))?;
            Ok(TextEdit {
                range: start..end,
                insert: new_text.to_owned(),
            })
        })
        .collect::<Result<Vec<_>, WorkspaceEditError>>()?;
    let mut ordered = edits;
    ordered.sort_by_key(|edit| (edit.range.start, edit.range.end));
    if ordered
        .windows(2)
        .any(|pair| pair[0].range.end > pair[1].range.start)
    {
        return Err(WorkspaceEditError::Overlap);
    }
    Ok(DocumentEdit {
        uri: uri.to_owned(),
        version,
        edits: ordered,
    })
}

fn position(value: Option<&Value>, index: &LineIndex) -> Result<usize, WorkspaceEditError> {
    let value =
        value.ok_or_else(|| WorkspaceEditError::Malformed("Missing location".to_owned()))?;
    let line = value
        .get("line")
        .and_then(Value::as_u64)
        .ok_or_else(|| WorkspaceEditError::Malformed("Invalid line number".to_owned()))?;
    let character = value
        .get("character")
        .and_then(Value::as_u64)
        .ok_or_else(|| WorkspaceEditError::Malformed("Invalid character position".to_owned()))?;
    let byte = index.offset(line, character).ok_or_else(|| {
        WorkspaceEditError::Malformed(
            "Position truncated UTF-16 character or out of range".to_owned(),
        )
    })?;
    index.char_offset(byte).ok_or_else(|| {
        WorkspaceEditError::Malformed("position splits a UTF-8 character".to_owned())
    })
}

fn uri_path(value: Option<&Value>) -> Result<PathBuf, WorkspaceEditError> {
    let uri = value
        .and_then(Value::as_str)
        .ok_or_else(|| WorkspaceEditError::Malformed("Missing resource URI".to_owned()))?;
    Url::parse(uri)
        .ok()
        .and_then(|uri| uri.to_file_path().ok())
        .ok_or_else(|| WorkspaceEditError::Malformed(format!("File URI `{uri}` is invalid")))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::{WorkspaceOperation, parse_workspace_edit};

    #[test]
    fn parses_utf16_changes_and_resource_operations_in_order() {
        let uri = "file:///C:/work/main.rs";
        let documents = HashMap::from([(uri.to_owned(), "a🙂b\n".to_owned())]);
        let edit = json!({"documentChanges":[
            {"textDocument":{"uri":uri,"version":3},"edits":[{"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":3}},"newText":"boundary"}]},
            {"kind":"create","uri":"file:///C:/work/new.rs"}
        ]});
        let operations =
            parse_workspace_edit(&edit, &documents).expect("Effective workspace editing");
        let WorkspaceOperation::Edit(document) = &operations[0] else {
            panic!("The first action should be edit")
        };
        assert_eq!(document.version, Some(3));
        assert_eq!(document.edits[0].range, 1..2);
        assert!(matches!(operations[1], WorkspaceOperation::Create(_)));
    }
}
