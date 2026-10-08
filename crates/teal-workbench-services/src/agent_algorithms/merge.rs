//! 不写入工作区的三方文本合并规划与人工解决校验。

use std::ops::Range;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use similar::{DiffTag, TextDiff};

/// 一处无法自动选择 ours/theirs 的重叠修改。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeConflict {
    /// 从零开始的 base 起始行。
    pub base_start_line: usize,
    /// 不包含在内的 base 结束行。
    pub base_end_line: usize,
    /// 冲突区域的 base 文本。
    pub base: String,
    /// 在同一区域应用 ours 修改后的文本。
    pub ours: String,
    /// 在同一区域应用 theirs 修改后的文本。
    pub theirs: String,
}

/// 一次不修改文件的三方合并结果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreeWayMerge {
    /// 自动合并文本；冲突处使用显式三方标记。
    pub merged: String,
    /// 按 base 行号排列的结构化冲突。
    pub conflicts: Vec<MergeConflict>,
    /// 三份输入的长度前缀 SHA-256，用于人工解决后的过期检查。
    pub input_digest: String,
}

/// 已在原三方输入上重新验证的人工解决结果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeResolution {
    /// 不含冲突标记的最终文本。
    pub text: String,
    /// 与产生冲突的三份输入绑定的摘要。
    pub input_digest: String,
}

/// 三方合并或人工解决验证错误。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ThreeWayMergeError {
    /// 人工解决仍包含合并标记。
    #[error("Merge resolution still contains conflict markers")]
    ConflictMarkers,
    /// base/ours/theirs 已变化，旧解决结果不能应用。
    #[error("Merge inputs changed before resolution was verified")]
    StaleInputs,
}

impl ThreeWayMerge {
    /// 计算 base、ours 和 theirs 的行级三方合并，不读取或写入文件。
    #[must_use]
    pub fn new(base: &str, ours: &str, theirs: &str) -> Self {
        let base_lines = lines(base);
        let ours_lines = lines(ours);
        let theirs_lines = lines(theirs);
        let mut changes = edits(&base_lines, &ours_lines, Side::Ours);
        changes.extend(edits(&base_lines, &theirs_lines, Side::Theirs));
        changes.sort_by_key(|edit| (edit.base.start, edit.base.end, edit.side));

        let mut regions: Vec<Vec<Edit>> = Vec::new();
        for edit in changes {
            let touching = regions
                .iter()
                .enumerate()
                .filter_map(|(index, region)| {
                    region
                        .iter()
                        .any(|existing| {
                            existing.side != edit.side && overlaps(&existing.base, &edit.base)
                        })
                        .then_some(index)
                })
                .collect::<Vec<_>>();
            if touching.is_empty() {
                regions.push(vec![edit]);
                continue;
            }
            let first = touching[0];
            regions[first].push(edit);
            for index in touching.into_iter().skip(1).rev() {
                let merged = regions.remove(index);
                regions[first].extend(merged);
            }
        }
        regions.sort_by_key(|region| region.iter().map(|edit| edit.base.start).min().unwrap_or(0));

        let mut output = String::new();
        let mut conflicts = Vec::new();
        let mut cursor = 0;
        for mut region in regions {
            region.sort_by_key(|edit| (edit.base.start, edit.base.end, edit.side));
            let start = region
                .iter()
                .map(|edit| edit.base.start)
                .min()
                .unwrap_or(cursor);
            let end = region
                .iter()
                .map(|edit| edit.base.end)
                .max()
                .unwrap_or(start);
            output.push_str(&base_lines[cursor..start].concat());

            let has_ours = region.iter().any(|edit| edit.side == Side::Ours);
            let has_theirs = region.iter().any(|edit| edit.side == Side::Theirs);
            let ours_text = apply_region(&base_lines, start..end, &region, Side::Ours);
            let theirs_text = apply_region(&base_lines, start..end, &region, Side::Theirs);
            if !has_ours {
                output.push_str(&theirs_text);
            } else if !has_theirs || ours_text == theirs_text {
                output.push_str(&ours_text);
            } else {
                let conflict = MergeConflict {
                    base_start_line: start,
                    base_end_line: end,
                    base: base_lines[start..end].concat(),
                    ours: ours_text,
                    theirs: theirs_text,
                };
                output.push_str("<<<<<<< ours\n");
                output.push_str(&conflict.ours);
                ensure_newline(&mut output);
                output.push_str("||||||| base\n");
                output.push_str(&conflict.base);
                ensure_newline(&mut output);
                output.push_str("=======\n");
                output.push_str(&conflict.theirs);
                ensure_newline(&mut output);
                output.push_str(">>>>>>> theirs\n");
                conflicts.push(conflict);
            }
            cursor = end;
        }
        output.push_str(&base_lines[cursor..].concat());

        Self {
            merged: output,
            conflicts,
            input_digest: input_digest(base, ours, theirs),
        }
    }

    /// 验证人工解决仍对应原三份输入，并且不再包含冲突标记。
    ///
    /// # Errors
    /// 输入摘要变化或解决文本仍包含标准冲突标记时返回错误。
    pub fn validate_resolution(
        &self,
        base: &str,
        ours: &str,
        theirs: &str,
        resolution: impl Into<String>,
    ) -> Result<MergeResolution, ThreeWayMergeError> {
        if self.input_digest != input_digest(base, ours, theirs) {
            return Err(ThreeWayMergeError::StaleInputs);
        }
        let text = resolution.into();
        if ["<<<<<<<", "|||||||", "=======", ">>>>>>>"]
            .iter()
            .any(|marker| text.lines().any(|line| line.starts_with(marker)))
        {
            return Err(ThreeWayMergeError::ConflictMarkers);
        }
        Ok(MergeResolution {
            text,
            input_digest: self.input_digest.clone(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Side {
    Ours,
    Theirs,
}

#[derive(Clone, Debug)]
struct Edit {
    side: Side,
    base: Range<usize>,
    replacement: Vec<String>,
}

fn lines(text: &str) -> Vec<String> {
    text.split_inclusive('\n').map(str::to_owned).collect()
}

fn edits(base: &[String], changed: &[String], side: Side) -> Vec<Edit> {
    let base = base.concat();
    let changed = changed.concat();
    TextDiff::from_lines(&base, &changed)
        .ops()
        .iter()
        .filter(|operation| operation.tag() != DiffTag::Equal)
        .map(|operation| Edit {
            side,
            base: operation.old_range(),
            replacement: changed
                .split_inclusive('\n')
                .map(str::to_owned)
                .collect::<Vec<_>>()[operation.new_range()]
            .to_vec(),
        })
        .collect()
}

fn overlaps(left: &Range<usize>, right: &Range<usize>) -> bool {
    if left.is_empty() && right.is_empty() {
        left.start == right.start
    } else if left.is_empty() {
        (right.start..=right.end).contains(&left.start)
    } else if right.is_empty() {
        (left.start..=left.end).contains(&right.start)
    } else {
        left.start < right.end && right.start < left.end
    }
}

fn apply_region(base: &[String], region: Range<usize>, edits: &[Edit], side: Side) -> String {
    let mut output = String::new();
    let mut cursor = region.start;
    for edit in edits.iter().filter(|edit| edit.side == side) {
        output.push_str(&base[cursor..edit.base.start].concat());
        output.push_str(&edit.replacement.concat());
        cursor = edit.base.end;
    }
    output.push_str(&base[cursor..region.end].concat());
    output
}

fn ensure_newline(text: &mut String) {
    if !text.ends_with('\n') {
        text.push('\n');
    }
}

fn input_digest(base: &str, ours: &str, theirs: &str) -> String {
    let mut digest = Sha256::new();
    for text in [base, ours, theirs] {
        digest.update((text.len() as u64).to_le_bytes());
        digest.update(text.as_bytes());
    }
    format!("{:x}", digest.finalize())
}
