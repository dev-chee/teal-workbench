//! 模型可见的计划记录工具。
//!
//! 工具只校验 `update_plan` 参数并返回确认；计划本身由源码 Agent 保存在 RuntimeStore，
//! 因此不产生任何工作区或进程副作用，也没有可审批的外部资源。工具不写入交付证据。

use super::*;
use crate::agent_algorithms::{
    ADOPT_TOOL_DESCRIPTION, ADOPT_TOOL_FUNCTION, ADOPT_TOOL_NAME, ADOPT_TOOL_PARAMETERS, AgentPlan,
    AgentProposal, PLAN_TOOL_DESCRIPTION, PLAN_TOOL_FUNCTION, PLAN_TOOL_MODULE, PLAN_TOOL_NAME,
    PLAN_TOOL_PARAMETERS, PROPOSAL_TOOL_DESCRIPTION, PROPOSAL_TOOL_FUNCTION, PROPOSAL_TOOL_MODULE,
    PROPOSAL_TOOL_NAME, PROPOSAL_TOOL_PARAMETERS,
};

/// 注册 `update_plan` Host 函数，并且只注册到 Agent 工具目录。
pub(super) fn register_plan_host(
    registry: &mut teal::HostRegistry,
) -> Result<(), teal::HostRegistryError> {
    registry.register(
        HostFunctionDefinition::blocking(
            PLAN_TOOL_MODULE,
            PLAN_TOOL_FUNCTION,
            teal::HostCallSignature::new([RuntimeType::Array], RuntimeType::Map)
                .expect("plan host signature")
                .task(),
            "agent.plan",
            ExternalCallRecovery::Pure,
            |arguments| {
                let steps = arguments
                    .first()
                    .ok_or_else(|| host_error("Plan update requires a `steps` array argument"))?;
                let steps = host_to_json(steps)?;
                let plan = AgentPlan::from_arguments(&serde_json::json!({ "steps": steps }))
                    .map_err(host_error)?;
                Ok(HostValue::Map(vec![
                    (
                        HostValue::String("status".into()),
                        HostValue::String("recorded".into()),
                    ),
                    (
                        HostValue::String("steps".into()),
                        HostValue::String(plan.steps().len().to_string()),
                    ),
                    (
                        HostValue::String("completed".into()),
                        HostValue::String(plan.completed().to_string()),
                    ),
                    (
                        HostValue::String("unfinished".into()),
                        HostValue::String(plan.unfinished().to_string()),
                    ),
                ]))
            },
        )
        .with_tool_export(HostToolExport {
            name: PLAN_TOOL_NAME.into(),
            description: PLAN_TOOL_DESCRIPTION.into(),
            parameters: PLAN_TOOL_PARAMETERS
                .iter()
                .map(|(name, description)| HostToolParameter {
                    name: (*name).to_owned(),
                    description: (*description).to_owned(),
                })
                .collect(),
            effect: HostToolEffect::ReadOnly,
        }),
    )?;

    // 探索阶段的候选方案校验与采纳：两者都没有外部资源，因此没有可审批的副作用。
    registry.register(
        HostFunctionDefinition::blocking(
            PROPOSAL_TOOL_MODULE,
            PROPOSAL_TOOL_FUNCTION,
            teal::HostCallSignature::new(
                [RuntimeType::String, RuntimeType::Array],
                RuntimeType::Map,
            )
            .expect("proposal host signature")
            .task(),
            "agent.plan",
            ExternalCallRecovery::Pure,
            |arguments| {
                let goal =
                    host_to_json(arguments.first().ok_or_else(|| {
                        host_error("Proposal recording requires a `goal` argument")
                    })?)?;
                let candidates = host_to_json(arguments.get(1).ok_or_else(|| {
                    host_error("Proposal recording requires a `candidates` argument")
                })?)?;
                let proposal = AgentProposal::from_arguments(&serde_json::json!({
                    "goal": goal,
                    "candidates": candidates,
                }))
                .map_err(host_error)?;
                Ok(HostValue::Map(vec![
                    (
                        HostValue::String("status".into()),
                        HostValue::String("recorded".into()),
                    ),
                    (
                        HostValue::String("candidates".into()),
                        HostValue::String(proposal.candidates.len().to_string()),
                    ),
                    (
                        HostValue::String("summary".into()),
                        HostValue::String(proposal.summary()),
                    ),
                ]))
            },
        )
        .with_tool_export(HostToolExport {
            name: PROPOSAL_TOOL_NAME.into(),
            description: PROPOSAL_TOOL_DESCRIPTION.into(),
            parameters: PROPOSAL_TOOL_PARAMETERS
                .iter()
                .map(|(name, description)| HostToolParameter {
                    name: (*name).to_owned(),
                    description: (*description).to_owned(),
                })
                .collect(),
            effect: HostToolEffect::ReadOnly,
        }),
    )?;

    registry.register(
        HostFunctionDefinition::blocking(
            PROPOSAL_TOOL_MODULE,
            ADOPT_TOOL_FUNCTION,
            teal::HostCallSignature::new([RuntimeType::String], RuntimeType::Map)
                .expect("proposal adoption signature")
                .task(),
            "agent.plan",
            ExternalCallRecovery::Pure,
            |arguments| {
                let id = arguments
                    .first()
                    .ok_or_else(|| host_error("Proposal adoption requires an `id` argument"))?;
                let HostValue::String(id) = id else {
                    return Err(host_error("Proposal adoption `id` must be a string"));
                };
                Ok(HostValue::Map(vec![
                    (
                        HostValue::String("status".into()),
                        HostValue::String("adopted".into()),
                    ),
                    (
                        HostValue::String("id".into()),
                        HostValue::String(id.clone()),
                    ),
                ]))
            },
        )
        .with_tool_export(HostToolExport {
            name: ADOPT_TOOL_NAME.into(),
            description: ADOPT_TOOL_DESCRIPTION.into(),
            parameters: ADOPT_TOOL_PARAMETERS
                .iter()
                .map(|(name, description)| HostToolParameter {
                    name: (*name).to_owned(),
                    description: (*description).to_owned(),
                })
                .collect(),
            effect: HostToolEffect::ReadOnly,
        }),
    )?;
    Ok(())
}

fn host_to_json(value: &HostValue) -> Result<serde_json::Value, HostError> {
    Ok(match value {
        HostValue::None => serde_json::Value::Null,
        HostValue::Bool(value) => serde_json::Value::Bool(*value),
        HostValue::Int(value) => serde_json::Value::from(*value),
        HostValue::Float(value) => serde_json::Number::from_f64(*value)
            .map(serde_json::Value::Number)
            .ok_or_else(|| host_error("Plan argument contains a non-finite number"))?,
        HostValue::String(value) => serde_json::Value::String(value.clone()),
        HostValue::Array(values) => serde_json::Value::Array(
            values
                .iter()
                .map(host_to_json)
                .collect::<Result<Vec<_>, HostError>>()?,
        ),
        HostValue::Map(entries) => {
            let mut object = serde_json::Map::with_capacity(entries.len());
            for (key, value) in entries {
                let HostValue::String(key) = key else {
                    return Err(host_error("Plan argument map keys must be strings"));
                };
                object.insert(key.clone(), host_to_json(value)?);
            }
            serde_json::Value::Object(object)
        }
        HostValue::Class { .. } | HostValue::Enum { .. } => {
            return Err(host_error(
                "Plan arguments cannot contain nominal class or enum values",
            ));
        }
    })
}
