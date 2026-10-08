//! 探索阶段的结构化方案记录。
//!
//! 方案描述目标下的候选做法、权衡与选定结果，供展示、采纳与恢复使用。它不产生任何
//! 工作区或外部副作用，也不是交付证据；最终交付仍必须通过 `TaskDelivery` 验收。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 记录候选方案的模型可见工具名。
pub const PROPOSAL_TOOL_NAME: &str = "record_proposals";

/// 采纳候选方案的模型可见工具名。
pub const ADOPT_TOOL_NAME: &str = "adopt_proposal";

/// 方案工具的 Host 模块名。
pub const PROPOSAL_TOOL_MODULE: &str = "std.bench.plan";

/// 记录候选方案的 Host 函数名。
pub const PROPOSAL_TOOL_FUNCTION: &str = "record_proposals";

/// 采纳候选方案的 Host 函数名。
pub const ADOPT_TOOL_FUNCTION: &str = "adopt_proposal";

/// 记录候选方案的模型说明。
pub const PROPOSAL_TOOL_DESCRIPTION: &str = "Record candidate approaches and their trade-offs for the current goal. \
     Recording never changes the workspace and is not delivery evidence";

/// 采纳候选方案的模型说明。
pub const ADOPT_TOOL_DESCRIPTION: &str = "Adopt one recorded candidate as the approach to follow";

/// 记录候选方案的参数说明。
pub const PROPOSAL_TOOL_PARAMETERS: [(&str, &str); 2] = [
    ("goal", "Goal the candidates address"),
    (
        "candidates",
        "Ordered array of at most 8 objects shaped {\"id\": string, \"title\": string, \
         \"summary\": string, \"tradeoffs\": [string]}",
    ),
];

/// 采纳候选方案的参数说明。
pub const ADOPT_TOOL_PARAMETERS: [(&str, &str); 1] = [("id", "Identifier of a recorded candidate")];

/// 一次记录的最大候选数。
pub const MAX_PROPOSAL_CANDIDATES: usize = 8;

/// 单个候选的最大权衡条目数。
pub const MAX_PROPOSAL_TRADEOFFS: usize = 8;

/// 候选标题允许的最大 UTF-8 字节数。
pub const MAX_PROPOSAL_TITLE_BYTES: usize = 200;

/// 候选摘要允许的最大 UTF-8 字节数。
pub const MAX_PROPOSAL_SUMMARY_BYTES: usize = 1024;

/// 单条权衡允许的最大 UTF-8 字节数。
pub const MAX_PROPOSAL_TRADEOFF_BYTES: usize = 512;

/// 候选方案的当前状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProposalStatus {
    /// 已记录但未选定。
    Proposed,

    /// 已选定为后续执行方案。
    Adopted,

    /// 明确否决。
    Rejected,
}

impl AgentProposalStatus {
    /// 返回持久化与展示使用的稳定名称。
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Adopted => "adopted",
            Self::Rejected => "rejected",
        }
    }
}

/// 一个候选方案。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProposalCandidate {
    /// 稳定短标识，供采纳与回链使用。
    pub id: String,

    /// 面向用户的标题。
    pub title: String,

    /// 方案概要。
    pub summary: String,

    /// 已知权衡与代价。
    pub tradeoffs: Vec<String>,

    /// 当前状态。
    pub status: AgentProposalStatus,
}

/// 一个 Agent 运行当前使用的方案集合。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProposal {
    /// 候选方案共同针对的目标。
    pub goal: String,

    /// 按记录顺序排列的候选方案。
    pub candidates: Vec<AgentProposalCandidate>,

    /// 已采纳的候选标识。
    pub adopted: Option<String>,

    /// 同一 Agent 运行内从 1 开始递增的修订号。
    pub revision: u32,

    /// 最近更新时间，Unix 毫秒。
    pub updated_at_ms: i64,
}

/// 方案参数或持久内容无效。
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum AgentProposalError {
    /// 参数不是合法的方案输入。
    #[error("Proposal arguments are invalid: {0}")]
    Invalid(String),
}

impl AgentProposal {
    /// 解析一次 `record_proposals` 工具调用参数。
    ///
    /// 只接受对象形式的 `{"goal": string, "candidates": [...]}`；未知字段、空目标、
    /// 空候选集、重复标识、越界数量或越界文本都被拒绝。
    ///
    /// # Errors
    /// 参数缺失、类型不符或越界时返回 [`AgentProposalError::Invalid`]。
    pub fn from_arguments(arguments: &serde_json::Value) -> Result<Self, AgentProposalError> {
        let object = arguments
            .as_object()
            .ok_or_else(|| AgentProposalError::Invalid("arguments must be an object".into()))?;
        if let Some(unknown) = object
            .keys()
            .find(|key| !matches!(key.as_str(), "goal" | "candidates"))
        {
            return Err(AgentProposalError::Invalid(format!(
                "unknown argument `{unknown}`"
            )));
        }
        let goal = object
            .get("goal")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| AgentProposalError::Invalid("`goal` must be a string".into()))?;
        let goal = validate_text(goal, MAX_PROPOSAL_SUMMARY_BYTES, "goal")?;
        let candidates = object
            .get("candidates")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| AgentProposalError::Invalid("`candidates` must be an array".into()))?;
        if candidates.is_empty() {
            return Err(AgentProposalError::Invalid(
                "`candidates` must not be empty".into(),
            ));
        }
        if candidates.len() > MAX_PROPOSAL_CANDIDATES {
            return Err(AgentProposalError::Invalid(format!(
                "`candidates` must not exceed {MAX_PROPOSAL_CANDIDATES} entries"
            )));
        }
        let mut parsed = Vec::with_capacity(candidates.len());
        for entry in candidates {
            let entry = entry.as_object().ok_or_else(|| {
                AgentProposalError::Invalid("each candidate must be an object".into())
            })?;
            if let Some(unknown) = entry
                .keys()
                .find(|key| !matches!(key.as_str(), "id" | "title" | "summary" | "tradeoffs"))
            {
                return Err(AgentProposalError::Invalid(format!(
                    "unknown candidate field `{unknown}`"
                )));
            }
            let id = entry
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    AgentProposalError::Invalid("each candidate requires a string `id`".into())
                })?;
            let id = validate_identifier(id)?;
            if parsed
                .iter()
                .any(|candidate: &AgentProposalCandidate| candidate.id == id)
            {
                return Err(AgentProposalError::Invalid(format!(
                    "candidate id `{id}` is duplicated"
                )));
            }
            let title = entry
                .get("title")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    AgentProposalError::Invalid("each candidate requires a string `title`".into())
                })?;
            let title = validate_text(title, MAX_PROPOSAL_TITLE_BYTES, "title")?;
            let summary = entry
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    AgentProposalError::Invalid("each candidate requires a string `summary`".into())
                })?;
            let summary = validate_text(summary, MAX_PROPOSAL_SUMMARY_BYTES, "summary")?;
            let mut tradeoffs = Vec::new();
            match entry.get("tradeoffs") {
                None | Some(serde_json::Value::Null) => {}
                Some(value) => {
                    let value = value.as_array().ok_or_else(|| {
                        AgentProposalError::Invalid("`tradeoffs` must be an array".into())
                    })?;
                    if value.len() > MAX_PROPOSAL_TRADEOFFS {
                        return Err(AgentProposalError::Invalid(format!(
                            "`tradeoffs` must not exceed {MAX_PROPOSAL_TRADEOFFS} entries"
                        )));
                    }
                    for item in value {
                        let item = item.as_str().ok_or_else(|| {
                            AgentProposalError::Invalid("each trade-off must be a string".into())
                        })?;
                        tradeoffs.push(validate_text(
                            item,
                            MAX_PROPOSAL_TRADEOFF_BYTES,
                            "trade-off",
                        )?);
                    }
                }
            }
            parsed.push(AgentProposalCandidate {
                id,
                title,
                summary,
                tradeoffs,
                status: AgentProposalStatus::Proposed,
            });
        }
        Ok(Self {
            goal,
            candidates: parsed,
            adopted: None,
            revision: 1,
            updated_at_ms: 0,
        })
    }

    /// 采纳一个已记录的候选，并把其余候选标记为否决。
    ///
    /// # Errors
    /// 标识未知或已被采纳时返回 [`AgentProposalError::Invalid`]。
    pub fn adopt(&mut self, id: &str) -> Result<(), AgentProposalError> {
        let id = validate_identifier(id)?;
        if !self.candidates.iter().any(|candidate| candidate.id == id) {
            return Err(AgentProposalError::Invalid(format!(
                "candidate `{id}` was not recorded"
            )));
        }
        for candidate in &mut self.candidates {
            candidate.status = if candidate.id == id {
                AgentProposalStatus::Adopted
            } else {
                AgentProposalStatus::Rejected
            };
        }
        self.adopted = Some(id);
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    /// 返回已采纳的候选。
    #[must_use]
    pub fn adopted_candidate(&self) -> Option<&AgentProposalCandidate> {
        let id = self.adopted.as_deref()?;
        self.candidates.iter().find(|candidate| candidate.id == id)
    }

    /// 返回候选顺序、文本与状态的稳定摘要。
    #[must_use]
    pub fn digest(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(self.goal.as_bytes());
        digest.update([0]);
        for candidate in &self.candidates {
            digest.update(candidate.status.name().as_bytes());
            digest.update([0]);
            digest.update(candidate.id.as_bytes());
            digest.update([0]);
            digest.update(candidate.title.as_bytes());
            digest.update([0]);
            digest.update(candidate.summary.as_bytes());
            for tradeoff in &candidate.tradeoffs {
                digest.update([0xff]);
                digest.update(tradeoff.as_bytes());
            }
            digest.update([0xfe]);
        }
        format!("{:x}", digest.finalize())
    }

    /// 返回供宿主与模型阅读的单行摘要。
    #[must_use]
    pub fn summary(&self) -> String {
        let adopted = self.adopted.as_deref().unwrap_or("none");
        format!(
            "proposals revision {}: {} candidates (adopted: {adopted})",
            self.revision,
            self.candidates.len()
        )
    }

    /// 返回按行渲染的展示文本。
    #[must_use]
    pub fn render(&self) -> String {
        let mut lines = Vec::with_capacity(self.candidates.len() * 2 + 2);
        lines.push(self.summary());
        lines.push(format!("Goal: {}", self.goal));
        for candidate in &self.candidates {
            lines.push(format!(
                "[{}] {} — {}",
                candidate.status.name(),
                candidate.id,
                candidate.title
            ));
            lines.push(format!("    {}", candidate.summary));
            for tradeoff in &candidate.tradeoffs {
                lines.push(format!("    - {tradeoff}"));
            }
        }
        lines.join("\n")
    }

    /// 校验反序列化得到的方案仍然满足当前边界。
    ///
    /// # Errors
    /// 目标、候选数量、修订号、标识、文本或采纳状态无效时返回 [`AgentProposalError::Invalid`]。
    pub fn validate(&self) -> Result<(), AgentProposalError> {
        validate_text(&self.goal, MAX_PROPOSAL_SUMMARY_BYTES, "goal")?;
        if self.candidates.is_empty() || self.candidates.len() > MAX_PROPOSAL_CANDIDATES {
            return Err(AgentProposalError::Invalid(format!(
                "candidates must contain 1 to {MAX_PROPOSAL_CANDIDATES} entries"
            )));
        }
        if self.revision == 0 {
            return Err(AgentProposalError::Invalid(
                "proposal revision must be positive".into(),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        let mut adopted = 0_usize;
        for candidate in &self.candidates {
            if validate_identifier(&candidate.id)? != candidate.id {
                return Err(AgentProposalError::Invalid(
                    "candidate id must be trimmed".into(),
                ));
            }
            if !seen.insert(candidate.id.as_str()) {
                return Err(AgentProposalError::Invalid(format!(
                    "candidate id `{}` is duplicated",
                    candidate.id
                )));
            }
            if validate_text(&candidate.title, MAX_PROPOSAL_TITLE_BYTES, "title")?
                != candidate.title
            {
                return Err(AgentProposalError::Invalid(
                    "candidate title must be trimmed".into(),
                ));
            }
            if validate_text(&candidate.summary, MAX_PROPOSAL_SUMMARY_BYTES, "summary")?
                != candidate.summary
            {
                return Err(AgentProposalError::Invalid(
                    "candidate summary must be trimmed".into(),
                ));
            }
            if candidate.tradeoffs.len() > MAX_PROPOSAL_TRADEOFFS {
                return Err(AgentProposalError::Invalid(format!(
                    "tradeoffs must not exceed {MAX_PROPOSAL_TRADEOFFS} entries"
                )));
            }
            for tradeoff in &candidate.tradeoffs {
                if validate_text(tradeoff, MAX_PROPOSAL_TRADEOFF_BYTES, "trade-off")? != *tradeoff {
                    return Err(AgentProposalError::Invalid(
                        "trade-off text must be trimmed".into(),
                    ));
                }
            }
            if candidate.status == AgentProposalStatus::Adopted {
                adopted += 1;
            }
        }
        match (&self.adopted, adopted) {
            (None, 0) => {}
            (Some(id), 1) => {
                let candidate = self
                    .candidates
                    .iter()
                    .find(|candidate| candidate.id == *id)
                    .ok_or_else(|| {
                        AgentProposalError::Invalid(
                            "adopted candidate must be part of the proposal".into(),
                        )
                    })?;
                if candidate.status != AgentProposalStatus::Adopted {
                    return Err(AgentProposalError::Invalid(
                        "adopted candidate must carry the adopted status".into(),
                    ));
                }
            }
            _ => {
                return Err(AgentProposalError::Invalid(
                    "exactly one adopted candidate is required when a proposal is adopted".into(),
                ));
            }
        }
        Ok(())
    }
}

fn validate_identifier(id: &str) -> Result<String, AgentProposalError> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return Err(AgentProposalError::Invalid(
            "candidate id must not be empty".into(),
        ));
    }
    if trimmed.len() > 64 {
        return Err(AgentProposalError::Invalid(
            "candidate id must not exceed 64 bytes".into(),
        ));
    }
    if !trimmed
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        return Err(AgentProposalError::Invalid(
            "candidate id accepts ASCII letters, digits, `-`, `_` and `.` only".into(),
        ));
    }
    Ok(trimmed.to_owned())
}

fn validate_text(value: &str, limit: usize, field: &str) -> Result<String, AgentProposalError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AgentProposalError::Invalid(format!(
            "{field} must not be empty"
        )));
    }
    if trimmed.len() > limit {
        return Err(AgentProposalError::Invalid(format!(
            "{field} must not exceed {limit} bytes"
        )));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(AgentProposalError::Invalid(format!(
            "{field} must not contain control characters"
        )));
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{AgentProposal, AgentProposalError, AgentProposalStatus, MAX_PROPOSAL_CANDIDATES};

    fn candidate(id: &str, title: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "title": title,
            "summary": "summary",
            "tradeoffs": ["one cost"]
        })
    }

    #[test]
    fn parses_candidates_and_adopts_exactly_one() {
        let mut proposal = AgentProposal::from_arguments(&serde_json::json!({
            "goal": "choose a storage layout",
            "candidates": [candidate("a", "row store"), candidate("b", "column store")],
        }))
        .expect("valid proposal");
        assert_eq!(proposal.candidates.len(), 2);
        assert_eq!(proposal.candidates[0].status, AgentProposalStatus::Proposed);
        proposal.adopt("b").expect("adopt a recorded candidate");
        assert_eq!(proposal.adopted.as_deref(), Some("b"));
        assert_eq!(proposal.candidates[0].status, AgentProposalStatus::Rejected);
        assert_eq!(
            proposal
                .adopted_candidate()
                .map(|candidate| candidate.id.as_str()),
            Some("b")
        );
        assert_eq!(proposal.revision, 2);
        proposal.validate().expect("adopted proposal stays valid");
        assert!(proposal.render().contains("Goal: choose a storage layout"));
        assert!(proposal.summary().contains("adopted: b"));
    }

    #[test]
    fn rejects_unknown_duplicate_and_out_of_range_arguments() {
        assert!(matches!(
            AgentProposal::from_arguments(&serde_json::json!({"goal": "g"})),
            Err(AgentProposalError::Invalid(_))
        ));
        assert!(matches!(
            AgentProposal::from_arguments(&serde_json::json!({
                "goal": "g",
                "candidates": [candidate("a", "one"), candidate("a", "two")],
            })),
            Err(AgentProposalError::Invalid(reason)) if reason.contains("duplicated")
        ));
        let many = (0..=MAX_PROPOSAL_CANDIDATES)
            .map(|index| candidate(&format!("c{index}"), "title"))
            .collect::<Vec<_>>();
        assert!(matches!(
            AgentProposal::from_arguments(&serde_json::json!({"goal": "g", "candidates": many})),
            Err(AgentProposalError::Invalid(reason)) if reason.contains("exceed")
        ));
        assert!(matches!(
            AgentProposal::from_arguments(&serde_json::json!({
                "goal": "g",
                "candidates": [{"id": "bad id", "title": "t", "summary": "s"}],
            })),
            Err(AgentProposalError::Invalid(reason)) if reason.contains("ASCII")
        ));
        let mut proposal = AgentProposal::from_arguments(&serde_json::json!({
            "goal": "g",
            "candidates": [candidate("a", "one")],
        }))
        .expect("valid proposal");
        assert!(matches!(
            proposal.adopt("missing"),
            Err(AgentProposalError::Invalid(reason)) if reason.contains("not recorded")
        ));
    }

    #[test]
    fn digest_changes_with_status_and_text() {
        let mut proposal = AgentProposal::from_arguments(&serde_json::json!({
            "goal": "g",
            "candidates": [candidate("a", "one"), candidate("b", "two")],
        }))
        .expect("valid proposal");
        let before = proposal.digest();
        proposal.adopt("a").expect("adopt");
        assert_ne!(before, proposal.digest());

        let mut tampered = proposal.clone();
        tampered.candidates[0].status = AgentProposalStatus::Proposed;
        assert!(
            tampered.validate().is_err(),
            "adopted state must stay consistent"
        );
    }
}
