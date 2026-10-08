//! 模型可更新的结构化计划。
//!
//! 计划只服务于展示、恢复和推进判定，从来不是交付证据；Task 交付仍必须通过
//! `TaskDelivery` 工具证据验收。计划类型同时给出唯一一份模型可见的 Host 元数据，
//! 避免工具声明与解析规则出现两份定义。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// `update_plan` 的模型可见工具名。
pub const PLAN_TOOL_NAME: &str = "update_plan";

/// `update_plan` 的 Host 模块名。
pub const PLAN_TOOL_MODULE: &str = "std.bench.plan";

/// `update_plan` 的 Host 函数名。
pub const PLAN_TOOL_FUNCTION: &str = "update";

/// `update_plan` 的工具说明。
pub const PLAN_TOOL_DESCRIPTION: &str = "Record the ordered multi-step plan and each step status. The plan is displayed to the user \
     and saved for recovery; it is never delivery evidence";

/// `update_plan` 的参数说明。
pub const PLAN_TOOL_PARAMETERS: [(&str, &str); 1] = [(
    "steps",
    "Ordered array of at most 32 objects shaped {\"step\": string, \"status\": \
     \"pending\"|\"in_progress\"|\"completed\"}",
)];

/// 一次计划更新的最大步骤数。
pub const MAX_PLAN_STEPS: usize = 32;

/// 单步说明允许的最大 UTF-8 字节数。
pub const MAX_PLAN_STEP_BYTES: usize = 512;

/// 计划步骤的展示与恢复状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPlanStepStatus {
    /// 尚未开始。
    Pending,
    /// 正在执行。
    InProgress,
    /// 已经完成。
    Completed,
}

impl AgentPlanStepStatus {
    /// 返回持久化和诊断使用的稳定名称。
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }

    fn parse(value: &str) -> Result<Self, AgentPlanError> {
        match value {
            "pending" => Ok(Self::Pending),
            "in_progress" => Ok(Self::InProgress),
            "completed" => Ok(Self::Completed),
            _ => Err(AgentPlanError::Invalid(format!(
                "unknown plan step status `{value}`"
            ))),
        }
    }
}

/// 一个有序计划步骤。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPlanStep {
    /// 面向用户的步骤说明。
    pub step: String,
    /// 当前状态。
    pub status: AgentPlanStepStatus,
}

/// 一个 Agent 运行当前使用的结构化计划。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPlan {
    /// 有序步骤；顺序表达执行意图。
    pub steps: Vec<AgentPlanStep>,
    /// 同一 Agent 运行内从 1 开始递增的修订号。
    pub revision: u32,
    /// 最近更新时间，Unix 毫秒。
    pub updated_at_ms: i64,
    /// 提交本修订的工具调用；旧记录为空，新的调用据此幂等对齐。
    #[serde(default)]
    pub source_call_id: Option<String>,
    /// 提交本计划的调用代次，供恢复时避免覆盖较新的运行计划。
    #[serde(default)]
    pub source_generation: Option<u64>,
}

/// 计划参数或持久内容无效。
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum AgentPlanError {
    /// 参数不是合法的 `update_plan` 输入。
    #[error("Plan arguments are invalid: {0}")]
    Invalid(String),
}

impl AgentPlan {
    /// 解析一次 `update_plan` 工具调用参数。
    ///
    /// 只接受对象形式的 `{"steps": [...]}`；未知字段、空计划、超过
    /// [`MAX_PLAN_STEPS`] 的步骤、空或过长的步骤说明以及未知状态都被拒绝。
    ///
    /// # Errors
    /// 参数缺失、类型不符或越界时返回 [`AgentPlanError::Invalid`]。
    pub fn from_arguments(arguments: &serde_json::Value) -> Result<Self, AgentPlanError> {
        let object = arguments
            .as_object()
            .ok_or_else(|| AgentPlanError::Invalid("arguments must be an object".into()))?;
        if let Some(unknown) = object.keys().find(|key| key.as_str() != "steps") {
            return Err(AgentPlanError::Invalid(format!(
                "unknown argument `{unknown}`"
            )));
        }
        let steps = object
            .get("steps")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| AgentPlanError::Invalid("`steps` must be an array".into()))?;
        if steps.is_empty() {
            return Err(AgentPlanError::Invalid("`steps` must not be empty".into()));
        }
        if steps.len() > MAX_PLAN_STEPS {
            return Err(AgentPlanError::Invalid(format!(
                "`steps` must not exceed {MAX_PLAN_STEPS} entries"
            )));
        }
        let mut parsed = Vec::with_capacity(steps.len());
        for entry in steps {
            let entry = entry
                .as_object()
                .ok_or_else(|| AgentPlanError::Invalid("each step must be an object".into()))?;
            if let Some(unknown) = entry
                .keys()
                .find(|key| !matches!(key.as_str(), "step" | "status"))
            {
                return Err(AgentPlanError::Invalid(format!(
                    "unknown step field `{unknown}`"
                )));
            }
            let step = entry
                .get("step")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    AgentPlanError::Invalid("each step requires a string `step`".into())
                })?;
            let step = validate_step_text(step)?;
            let status = match entry.get("status") {
                Some(status) => status
                    .as_str()
                    .ok_or_else(|| AgentPlanError::Invalid("`status` must be a string".into()))
                    .and_then(AgentPlanStepStatus::parse)?,
                None => AgentPlanStepStatus::Pending,
            };
            parsed.push(AgentPlanStep { step, status });
        }
        Ok(Self {
            steps: parsed,
            revision: 1,
            updated_at_ms: 0,
            source_call_id: None,
            source_generation: None,
        })
    }

    /// 返回按顺序排列的步骤。
    #[must_use]
    pub fn steps(&self) -> &[AgentPlanStep] {
        &self.steps
    }

    /// 返回仍未完成的步骤数。
    #[must_use]
    pub fn unfinished(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| step.status != AgentPlanStepStatus::Completed)
            .count()
    }

    /// 返回已经完成的步骤数。
    #[must_use]
    pub fn completed(&self) -> usize {
        self.steps.len() - self.unfinished()
    }

    /// 返回步骤顺序、文本和状态的稳定摘要。
    #[must_use]
    pub fn digest(&self) -> String {
        let mut digest = Sha256::new();
        for step in &self.steps {
            digest.update(step.status.name().as_bytes());
            digest.update([0]);
            digest.update(step.step.as_bytes());
            digest.update([0xff]);
        }
        format!("{:x}", digest.finalize())
    }

    /// 返回供宿主与模型阅读的单行摘要。
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "plan revision {}: {} steps ({} completed, {} unfinished)",
            self.revision,
            self.steps.len(),
            self.completed(),
            self.unfinished()
        )
    }

    /// 返回按行渲染的展示文本。
    #[must_use]
    pub fn render(&self) -> String {
        let mut lines = Vec::with_capacity(self.steps.len() + 1);
        lines.push(self.summary());
        lines.extend(self.steps.iter().enumerate().map(|(index, step)| {
            let marker = match step.status {
                AgentPlanStepStatus::Pending => "[ ]",
                AgentPlanStepStatus::InProgress => "[>]",
                AgentPlanStepStatus::Completed => "[x]",
            };
            format!("{marker} {}. {}", index + 1, step.step)
        }));
        lines.join("\n")
    }

    /// 校验反序列化得到的计划仍然满足当前边界。
    ///
    /// # Errors
    /// 步骤数量、修订号或步骤文本无效时返回 [`AgentPlanError::Invalid`]。
    pub fn validate(&self) -> Result<(), AgentPlanError> {
        if self.steps.is_empty() || self.steps.len() > MAX_PLAN_STEPS {
            return Err(AgentPlanError::Invalid(format!(
                "steps must contain 1 to {MAX_PLAN_STEPS} entries"
            )));
        }
        if self.revision == 0 {
            return Err(AgentPlanError::Invalid(
                "plan revision must be positive".into(),
            ));
        }
        if self
            .source_call_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 128 || id.chars().any(char::is_control))
        {
            return Err(AgentPlanError::Invalid(
                "plan source call id is invalid".into(),
            ));
        }
        if self.source_call_id.is_some() != self.source_generation.is_some() {
            return Err(AgentPlanError::Invalid(
                "plan source identity is incomplete".into(),
            ));
        }
        for step in &self.steps {
            if validate_step_text(&step.step)? != step.step {
                return Err(AgentPlanError::Invalid("step text must be trimmed".into()));
            }
        }
        Ok(())
    }
}

fn validate_step_text(step: &str) -> Result<String, AgentPlanError> {
    let trimmed = step.trim();
    if trimmed.is_empty() {
        return Err(AgentPlanError::Invalid(
            "step text must not be empty".into(),
        ));
    }
    if trimmed.len() > MAX_PLAN_STEP_BYTES {
        return Err(AgentPlanError::Invalid(format!(
            "step text must not exceed {MAX_PLAN_STEP_BYTES} bytes"
        )));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(AgentPlanError::Invalid(
            "step text must not contain control characters".into(),
        ));
    }
    Ok(trimmed.to_owned())
}
