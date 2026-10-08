//! 无界面工作区基线、合并算法和脱机策略。
#![deny(missing_docs)]

mod branching;
mod merge;
mod offline;
mod plan;
mod proposal;
mod util;

pub use branching::{ConversationBaseline, ConversationBranchError};
pub use plan::{
    AgentPlan, AgentPlanError, AgentPlanStep, AgentPlanStepStatus, MAX_PLAN_STEPS,
    MAX_PLAN_STEP_BYTES, PLAN_TOOL_DESCRIPTION, PLAN_TOOL_FUNCTION, PLAN_TOOL_MODULE,
    PLAN_TOOL_NAME, PLAN_TOOL_PARAMETERS,
};
pub use proposal::{
    ADOPT_TOOL_DESCRIPTION, ADOPT_TOOL_FUNCTION, ADOPT_TOOL_NAME, ADOPT_TOOL_PARAMETERS,
    AgentProposal, AgentProposalCandidate, AgentProposalError, AgentProposalStatus,
    MAX_PROPOSAL_CANDIDATES, MAX_PROPOSAL_SUMMARY_BYTES, MAX_PROPOSAL_TITLE_BYTES,
    MAX_PROPOSAL_TRADEOFFS, MAX_PROPOSAL_TRADEOFF_BYTES, PROPOSAL_TOOL_DESCRIPTION,
    PROPOSAL_TOOL_FUNCTION, PROPOSAL_TOOL_MODULE, PROPOSAL_TOOL_NAME, PROPOSAL_TOOL_PARAMETERS,
};
pub use merge::{MergeConflict, MergeResolution, ThreeWayMerge, ThreeWayMergeError};
pub use offline::{ConnectivityCapability, ConnectivityMode, OfflineBlocked, allows_offline_model_tool};
pub use util::{digest, now_ms};
