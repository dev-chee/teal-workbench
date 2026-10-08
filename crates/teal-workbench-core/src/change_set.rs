//! 可审查变更集及其文件、hunk 身份。
//!
//! 本模块只描述由同一工作区快照产生的候选修改，不执行文件 I/O 或授权。调用方必须在应用前
//! 重新核对摘要，并通过宿主事务提交选择的内容。

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// 一组由同一任务和请求代次生成的候选文件修改。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSet {
    /// 由完整内容身份派生的稳定摘要。
    pub id: String,
    /// 发起变更的任务；普通交互修改可以没有任务身份。
    pub source_task: Option<String>,
    /// 生成该变更时的 Agent 请求代次。
    pub generation: u64,
    /// 按展示和应用顺序排列的文件修改。
    pub files: Vec<ChangeSetFile>,
}

/// ChangeSet 中一个文件的完整候选修改。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSetFile {
    /// 工作区相对路径。
    pub path: PathBuf,
    /// 修改前规范文本的 SHA-256。
    pub original_sha256: String,
    /// 修改后规范文本的 SHA-256。
    pub new_sha256: String,
    /// 包含文件头和全部 hunk 的统一 diff。
    pub diff: String,
    /// 可独立选择的 hunk。
    pub hunks: Vec<ChangeSetHunk>,
}

/// 一个可独立审查的 unified diff hunk。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSetHunk {
    /// 由文件路径和 hunk 正文派生的稳定身份。
    pub id: String,
    /// 文件内从一开始的展示序号。
    pub index: usize,
    /// 包含 `@@` 头的完整 hunk 文本。
    pub diff: String,
}

/// 用户从 ChangeSet 中选择的应用粒度。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeSetSelection {
    /// 应用全部文件的全部 hunk。
    All,
    /// 应用指定文件的全部 hunk。
    File(PathBuf),
    /// 只应用指定文件中的一个 hunk。
    Hunk {
        /// hunk 所属的工作区相对路径。
        path: PathBuf,
        /// [`ChangeSetHunk::id`] 中的稳定身份。
        id: String,
    },
}

/// 无法建立确定性 ChangeSet 的原因。
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ChangeSetError {
    /// 变更集没有文件。
    #[error("ChangeSet must contain at least one file")]
    Empty,
    /// 文件路径不是非空工作区相对路径。
    #[error("ChangeSet path must be workspace-relative: {0}")]
    InvalidPath(PathBuf),
    /// 同一路径在变更集中出现多次。
    #[error("ChangeSet contains duplicate path: {0}")]
    DuplicatePath(PathBuf),
    /// 文件修改没有有效的 unified diff hunk。
    #[error("ChangeSet file has no unified diff hunks: {0}")]
    MissingHunks(PathBuf),
    /// 文件摘要不是 SHA-256 十六进制值。
    #[error("ChangeSet file has an invalid {kind} digest: {path}")]
    InvalidDigest {
        /// 无效摘要所属文件。
        path: PathBuf,
        /// 摘要代表修改前还是修改后内容。
        kind: &'static str,
    },
    /// 同一文件内出现重复 hunk 身份。
    #[error("ChangeSet file contains a duplicate hunk id: {0}")]
    DuplicateHunk(String),
    /// 选择引用了 ChangeSet 中不存在的文件或 hunk。
    #[error("ChangeSet selection does not exist")]
    UnknownSelection,
    /// 调用方没有提供选中文件的当前文本。
    #[error("ChangeSet source is missing: {0}")]
    MissingSource(PathBuf),
    /// 文件在审查后已经变化。
    #[error("ChangeSet source digest changed: {0}")]
    Stale(PathBuf),
    /// hunk 与原始文本或 unified diff 结构不一致。
    #[error("Invalid ChangeSet hunk for {path}: {reason}")]
    InvalidHunk {
        /// 无法应用 hunk 的文件。
        path: PathBuf,
        /// 具体结构或上下文错误。
        reason: String,
    },
}

impl ChangeSet {
    /// 校验文件集合并生成绑定任务、代次、路径、摘要和 hunk 的稳定身份。
    pub fn new(
        source_task: Option<String>,
        generation: u64,
        files: Vec<ChangeSetFile>,
    ) -> Result<Self, ChangeSetError> {
        if files.is_empty() {
            return Err(ChangeSetError::Empty);
        }
        let mut paths = BTreeSet::new();
        for file in &files {
            if file.path.as_os_str().is_empty()
                || !file
                    .path
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
            {
                return Err(ChangeSetError::InvalidPath(file.path.clone()));
            }
            if !paths.insert(file.path.clone()) {
                return Err(ChangeSetError::DuplicatePath(file.path.clone()));
            }
            if file.hunks.is_empty() {
                return Err(ChangeSetError::MissingHunks(file.path.clone()));
            }
            for (kind, digest) in [
                ("original", &file.original_sha256),
                ("new", &file.new_sha256),
            ] {
                if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(ChangeSetError::InvalidDigest {
                        path: file.path.clone(),
                        kind,
                    });
                }
            }
            let mut hunks = BTreeSet::new();
            for hunk in &file.hunks {
                if hunk.id.is_empty() || !hunks.insert(&hunk.id) {
                    return Err(ChangeSetError::DuplicateHunk(hunk.id.clone()));
                }
            }
        }
        let identity = serde_json::to_vec(&(&source_task, generation, &files))
            .expect("ChangeSet identity fields are serializable");
        Ok(Self {
            id: format!("{:x}", Sha256::digest(identity)),
            source_task,
            generation,
            files,
        })
    }

    /// 在纯内存快照上应用选中的整组、单文件或单个 hunk。
    ///
    /// 返回值只包含被选择的文件。所有选择和输入摘要会在产生任何结果前完成校验；调用方仍须
    /// 在写盘前再次执行自己的版本检查，并负责跨文件提交或回滚。
    pub fn apply_selected(
        &self,
        originals: &BTreeMap<PathBuf, String>,
        selection: &ChangeSetSelection,
    ) -> Result<BTreeMap<PathBuf, String>, ChangeSetError> {
        let selected = match selection {
            ChangeSetSelection::All => self
                .files
                .iter()
                .map(|file| (file, None))
                .collect::<Vec<_>>(),
            ChangeSetSelection::File(path) => vec![(
                self.files
                    .iter()
                    .find(|file| &file.path == path)
                    .ok_or(ChangeSetError::UnknownSelection)?,
                None,
            )],
            ChangeSetSelection::Hunk { path, id } => {
                let file = self
                    .files
                    .iter()
                    .find(|file| &file.path == path)
                    .ok_or(ChangeSetError::UnknownSelection)?;
                if !file.hunks.iter().any(|hunk| &hunk.id == id) {
                    return Err(ChangeSetError::UnknownSelection);
                }
                vec![(file, Some(id.as_str()))]
            }
        };

        for (file, _) in &selected {
            let original = originals
                .get(&file.path)
                .ok_or_else(|| ChangeSetError::MissingSource(file.path.clone()))?;
            if digest(original) != file.original_sha256 {
                return Err(ChangeSetError::Stale(file.path.clone()));
            }
        }

        selected
            .into_iter()
            .map(|(file, hunk)| {
                let original = &originals[&file.path];
                let text = apply_hunks(file, original, hunk)?;
                // 只有整文件应用才承诺覆盖调用方绑定的修改后摘要；单个 hunk 允许产生中间结果。
                if hunk.is_none() && digest(&text) != file.new_sha256 {
                    return Err(invalid_hunk(
                        file,
                        "result digest does not match reviewed content",
                    ));
                }
                Ok((file.path.clone(), text))
            })
            .collect()
    }
}

impl ChangeSetFile {
    /// 从修改前后摘要和 unified diff 建立带稳定 hunk 身份的文件变更。
    pub fn new(
        path: PathBuf,
        original_sha256: String,
        new_sha256: String,
        diff: String,
    ) -> Result<Self, ChangeSetError> {
        let hunks = split_hunks(&path, &diff);
        if hunks.is_empty() {
            return Err(ChangeSetError::MissingHunks(path));
        }
        Ok(Self {
            path,
            original_sha256,
            new_sha256,
            diff,
            hunks,
        })
    }

    /// 在调用方给出的当前文本上应用全部 hunk，并核对修改前摘要。
    ///
    /// 与 [`ChangeSet::apply_selected`] 不同，本方法不核对 [`ChangeSetFile::new_sha256`]，
    /// 供生成流程在还没有可信修改后摘要时求出真实结果。调用方必须用返回文本的摘要替换
    /// 构造时使用的占位值，否则后续整文件应用会因摘要不符而失败。
    pub fn apply_all(&self, original: &str) -> Result<String, ChangeSetError> {
        if digest(original) != self.original_sha256 {
            return Err(ChangeSetError::Stale(self.path.clone()));
        }
        apply_hunks(self, original, None)
    }
}

fn split_hunks(path: &std::path::Path, diff: &str) -> Vec<ChangeSetHunk> {
    let lines = diff.lines().collect::<Vec<_>>();
    let starts = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.starts_with("@@ ").then_some(index))
        .collect::<Vec<_>>();
    starts
        .iter()
        .enumerate()
        .map(|(index, start)| {
            let end = starts.get(index + 1).copied().unwrap_or(lines.len());
            let diff = lines[*start..end].join("\n");
            let identity = format!("{}\0{}\0{diff}", path.to_string_lossy(), index + 1);
            ChangeSetHunk {
                id: format!("{:x}", Sha256::digest(identity.as_bytes())),
                index: index + 1,
                diff,
            }
        })
        .collect()
}

fn apply_hunks(
    file: &ChangeSetFile,
    original: &str,
    selected_hunk: Option<&str>,
) -> Result<String, ChangeSetError> {
    let normalized = original.replace("\r\n", "\n").replace('\r', "\n");
    let mut final_newline = normalized.ends_with('\n');
    let source = if normalized.is_empty() {
        Vec::new()
    } else {
        normalized
            .strip_suffix('\n')
            .unwrap_or(&normalized)
            .split('\n')
            .collect::<Vec<_>>()
    };
    let mut output = Vec::new();
    let mut source_index = 0;

    for hunk in &file.hunks {
        let mut lines = hunk.diff.lines();
        let header = lines
            .next()
            .ok_or_else(|| invalid_hunk(file, "missing header"))?;
        let (old_start, old_count, new_count) =
            hunk_ranges(header).ok_or_else(|| invalid_hunk(file, "invalid unified diff header"))?;
        if old_count > 0 && old_start == 0 {
            return Err(invalid_hunk(file, "non-empty source range starts at zero"));
        }
        let target = if old_count == 0 {
            old_start
        } else {
            old_start.saturating_sub(1)
        };
        if target < source_index || target.saturating_add(old_count) > source.len() {
            return Err(invalid_hunk(file, "source range is invalid or overlaps"));
        }
        output.extend(source[source_index..target].iter().copied());

        if selected_hunk.is_none_or(|selected| selected == hunk.id) {
            let mut consumed = 0;
            let mut produced = 0;
            let mut last_operation = None;
            let mut new_side_has_no_newline = false;
            for line in lines {
                if line == "\\ No newline at end of file" {
                    match last_operation {
                        Some('+') | Some(' ') => new_side_has_no_newline = true,
                        Some('-') => {}
                        _ => return Err(invalid_hunk(file, "newline marker has no operation")),
                    }
                    continue;
                }
                let Some(marker) = line.chars().next() else {
                    return Err(invalid_hunk(file, "empty hunk line"));
                };
                let value = &line[marker.len_utf8()..];
                match marker {
                    ' ' => {
                        if source.get(target + consumed).copied() != Some(value) {
                            return Err(invalid_hunk(file, "context does not match source"));
                        }
                        output.push(value);
                        consumed += 1;
                        produced += 1;
                        last_operation = Some(' ');
                    }
                    '-' => {
                        if source.get(target + consumed).copied() != Some(value) {
                            return Err(invalid_hunk(file, "removed line does not match source"));
                        }
                        consumed += 1;
                        last_operation = Some('-');
                    }
                    '+' => {
                        output.push(value);
                        produced += 1;
                        last_operation = Some('+');
                    }
                    _ => return Err(invalid_hunk(file, "invalid hunk line marker")),
                }
            }
            if consumed != old_count || produced != new_count {
                return Err(invalid_hunk(file, "line counts do not match hunk header"));
            }
            if target + consumed == source.len() {
                final_newline = match last_operation {
                    Some('+') | Some(' ') => !new_side_has_no_newline,
                    Some('-') => !output.is_empty(),
                    None => final_newline,
                    _ => unreachable!("hunk markers are validated above"),
                };
            }
        } else {
            output.extend(source[target..target + old_count].iter().copied());
        }
        source_index = target + old_count;
    }
    output.extend(source[source_index..].iter().copied());
    let mut result = output.join("\n");
    if final_newline {
        result.push('\n');
    }
    Ok(result)
}

fn hunk_ranges(header: &str) -> Option<(usize, usize, usize)> {
    let end = header.get(3..)?.find(" @@")? + 3;
    let mut ranges = header.get(3..end)?.split_whitespace();
    let old = ranges.next()?.strip_prefix('-')?;
    let new = ranges.next()?.strip_prefix('+')?;
    if ranges.next().is_some() {
        return None;
    }
    let parse = |value: &str| -> Option<(usize, usize)> {
        let mut values = value.split(',');
        let start = values.next()?.parse().ok()?;
        let count = values.next().map_or(Some(1), |value| value.parse().ok())?;
        values.next().is_none().then_some((start, count))
    };
    let (old_start, old_count) = parse(old)?;
    let (_, new_count) = parse(new)?;
    Some((old_start, old_count, new_count))
}

fn invalid_hunk(file: &ChangeSetFile, reason: &str) -> ChangeSetError {
    ChangeSetError::InvalidHunk {
        path: file.path.clone(),
        reason: reason.into(),
    }
}

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(value: char) -> String {
        std::iter::repeat_n(value, 64).collect()
    }

    fn diff(path: &str) -> String {
        format!("--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-old\n+new\n@@ -4 +4 @@\n-before\n+after")
    }

    #[test]
    fn multi_file_multi_hunk_changeset_has_stable_distinct_identities() {
        let first = ChangeSetFile::new(
            PathBuf::from("src/main.tl"),
            digest('a'),
            digest('b'),
            diff("src/main.tl"),
        )
        .unwrap();
        let second = ChangeSetFile::new(
            PathBuf::from("src/lib.tl"),
            digest('c'),
            digest('d'),
            diff("src/lib.tl"),
        )
        .unwrap();
        let change_set = ChangeSet::new(
            Some("task-1".into()),
            7,
            vec![first.clone(), second.clone()],
        )
        .unwrap();
        let repeated = ChangeSet::new(Some("task-1".into()), 7, vec![first, second]).unwrap();

        assert_eq!(change_set.id, repeated.id);
        assert_eq!(change_set.files.len(), 2);
        assert_eq!(change_set.files[0].hunks.len(), 2);
        assert_eq!(change_set.files[1].hunks.len(), 2);
        assert_eq!(
            change_set.files[0]
                .hunks
                .iter()
                .map(|hunk| hunk.index)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_ne!(
            change_set.files[0].hunks[0].id,
            change_set.files[0].hunks[1].id
        );
        assert_ne!(
            change_set.files[0].hunks[0].id,
            change_set.files[1].hunks[0].id
        );
    }

    #[test]
    fn changeset_rejects_invalid_files_and_hunks() {
        let valid = || {
            ChangeSetFile::new(
                PathBuf::from("src/main.tl"),
                digest('a'),
                digest('b'),
                diff("src/main.tl"),
            )
            .unwrap()
        };

        assert_eq!(
            ChangeSet::new(None, 0, Vec::new()),
            Err(ChangeSetError::Empty)
        );
        assert!(matches!(
            ChangeSet::new(
                None,
                0,
                vec![ChangeSetFile {
                    path: PathBuf::from("/absolute.tl"),
                    ..valid()
                }]
            ),
            Err(ChangeSetError::InvalidPath(_))
        ));
        assert!(matches!(
            ChangeSet::new(
                None,
                0,
                vec![ChangeSetFile {
                    path: PathBuf::from("../escape.tl"),
                    ..valid()
                }]
            ),
            Err(ChangeSetError::InvalidPath(_))
        ));
        assert!(matches!(
            ChangeSet::new(None, 0, vec![valid(), valid()]),
            Err(ChangeSetError::DuplicatePath(_))
        ));
        assert!(matches!(
            ChangeSetFile::new(
                PathBuf::from("src/main.tl"),
                digest('a'),
                digest('b'),
                "header".into()
            ),
            Err(ChangeSetError::MissingHunks(_))
        ));
        assert!(matches!(
            ChangeSet::new(
                None,
                0,
                vec![ChangeSetFile {
                    hunks: Vec::new(),
                    ..valid()
                }]
            ),
            Err(ChangeSetError::MissingHunks(_))
        ));
        assert!(matches!(
            ChangeSet::new(
                None,
                0,
                vec![ChangeSetFile {
                    original_sha256: "not-a-digest".into(),
                    ..valid()
                }]
            ),
            Err(ChangeSetError::InvalidDigest {
                kind: "original",
                ..
            })
        ));
        assert!(matches!(
            ChangeSet::new(None, 0, vec![ChangeSetFile {
                hunks: vec![
                    ChangeSetHunk { id: "duplicate".into(), index: 1, diff: "@@ -1 +1 @@\n-old\n+new".into() },
                    ChangeSetHunk { id: "duplicate".into(), index: 2, diff: "@@ -4 +4 @@\n-before\n+after".into() },
                ],
                ..valid()
            }]),
            Err(ChangeSetError::DuplicateHunk(id)) if id == "duplicate"
        ));
    }

    fn file(path: &str, original: &str, replacement: &str, diff: &str) -> ChangeSetFile {
        ChangeSetFile::new(
            PathBuf::from(path),
            super::digest(original),
            super::digest(replacement),
            diff.into(),
        )
        .unwrap()
    }

    #[test]
    fn apply_selected_handles_all_file_and_single_hunk_in_memory() {
        let main_original = "one\ntwo\nthree\nfour\n";
        let main_new = "ONE\ntwo\nthree\nFOUR\n";
        let main = file(
            "src/main.tl",
            main_original,
            main_new,
            "--- a/src/main.tl\n+++ b/src/main.tl\n@@ -1 +1 @@\n-one\n+ONE\n@@ -4 +4 @@\n-four\n+FOUR",
        );
        let lib = file(
            "src/lib.tl",
            "old\n",
            "new\n",
            "--- a/src/lib.tl\n+++ b/src/lib.tl\n@@ -1 +1 @@\n-old\n+new",
        );
        let change_set = ChangeSet::new(None, 1, vec![main.clone(), lib.clone()]).unwrap();
        let originals = BTreeMap::from([
            (main.path.clone(), main_original.into()),
            (lib.path.clone(), "old\n".into()),
        ]);

        let all = change_set
            .apply_selected(&originals, &ChangeSetSelection::All)
            .unwrap();
        assert_eq!(all[&main.path], main_new);
        assert_eq!(all[&lib.path], "new\n");
        assert_eq!(super::digest(&all[&main.path]), main.new_sha256);
        assert_eq!(super::digest(&all[&lib.path]), lib.new_sha256);

        let selected_file = change_set
            .apply_selected(&originals, &ChangeSetSelection::File(main.path.clone()))
            .unwrap();
        assert_eq!(selected_file.len(), 1);
        assert_eq!(selected_file[&main.path], main_new);

        let selected_hunk = change_set
            .apply_selected(
                &originals,
                &ChangeSetSelection::Hunk {
                    path: main.path.clone(),
                    id: main.hunks[0].id.clone(),
                },
            )
            .unwrap();
        assert_eq!(selected_hunk.len(), 1);
        assert_eq!(selected_hunk[&main.path], "ONE\ntwo\nthree\nfour\n");
        assert_ne!(super::digest(&selected_hunk[&main.path]), main.new_sha256);
    }

    #[test]
    fn apply_selected_handles_empty_additions_deletions_and_no_newline_markers() {
        let added = file(
            "src/added.tl",
            "",
            "new\n",
            "--- a/src/added.tl\n+++ b/src/added.tl\n@@ -0,0 +1 @@\n+new",
        );
        let deleted = file(
            "src/deleted.tl",
            "old\n",
            "",
            "--- a/src/deleted.tl\n+++ b/src/deleted.tl\n@@ -1 +0,0 @@\n-old",
        );
        let no_newline = file(
            "src/no-newline.tl",
            "old",
            "new",
            "--- a/src/no-newline.tl\n+++ b/src/no-newline.tl\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file",
        );
        let change_set = ChangeSet::new(
            None,
            1,
            vec![added.clone(), deleted.clone(), no_newline.clone()],
        )
        .unwrap();
        let originals = BTreeMap::from([
            (added.path.clone(), String::new()),
            (deleted.path.clone(), "old\n".into()),
            (no_newline.path.clone(), "old".into()),
        ]);

        let applied = change_set
            .apply_selected(&originals, &ChangeSetSelection::All)
            .unwrap();
        assert_eq!(applied[&added.path], "new\n");
        assert_eq!(applied[&deleted.path], "");
        assert_eq!(applied[&no_newline.path], "new");
    }

    #[test]
    fn apply_selected_rejects_unknown_missing_stale_and_invalid_hunks() {
        let original = "old\n";
        let mut file = file(
            "src/main.tl",
            original,
            "new\n",
            "--- a/src/main.tl\n+++ b/src/main.tl\n@@ -1 +1 @@\n-old\n+new",
        );
        let change_set = ChangeSet::new(None, 1, vec![file.clone()]).unwrap();
        let originals = BTreeMap::from([(file.path.clone(), original.into())]);

        assert_eq!(
            change_set.apply_selected(
                &originals,
                &ChangeSetSelection::File(PathBuf::from("src/missing.tl"))
            ),
            Err(ChangeSetError::UnknownSelection)
        );
        assert_eq!(
            change_set.apply_selected(
                &originals,
                &ChangeSetSelection::Hunk {
                    path: file.path.clone(),
                    id: "missing-hunk".into(),
                }
            ),
            Err(ChangeSetError::UnknownSelection)
        );
        assert_eq!(
            change_set.apply_selected(&BTreeMap::new(), &ChangeSetSelection::All),
            Err(ChangeSetError::MissingSource(file.path.clone()))
        );
        assert_eq!(
            change_set.apply_selected(
                &BTreeMap::from([(file.path.clone(), "changed\n".into())]),
                &ChangeSetSelection::All
            ),
            Err(ChangeSetError::Stale(file.path.clone()))
        );

        file.hunks[0].diff = "@@ -1 +1,2 @@\n-old\n+new".into();
        let malformed = ChangeSet::new(None, 1, vec![file]).unwrap();
        assert!(matches!(
            malformed.apply_selected(&originals, &ChangeSetSelection::All),
            Err(ChangeSetError::InvalidHunk { reason, .. }) if reason == "line counts do not match hunk header"
        ));
    }

    #[test]
    fn apply_all_binds_the_result_without_asserting_a_reviewed_digest() {
        let original = "old\n";
        let mut file = file(
            "src/main.tl",
            original,
            "new\n",
            "--- a/src/main.tl\n+++ b/src/main.tl\n@@ -1 +1 @@\n-old\n+new",
        );
        file.new_sha256 = digest('f');

        assert_eq!(file.apply_all(original).unwrap(), "new\n");
        assert_eq!(
            file.apply_all("changed\n"),
            Err(ChangeSetError::Stale(file.path.clone()))
        );

        // 整文件审查路径仍以调用方绑定的修改后摘要为准。
        let change_set = ChangeSet::new(None, 1, vec![file]).unwrap();
        assert!(matches!(
            change_set.apply_selected(
                &BTreeMap::from([(PathBuf::from("src/main.tl"), original.into())]),
                &ChangeSetSelection::All
            ),
            Err(ChangeSetError::InvalidHunk { reason, .. })
                if reason == "result digest does not match reviewed content"
        ));
    }
}
