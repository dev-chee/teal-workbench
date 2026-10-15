@edition("2026");

// 独立知识审核 Agent：模型只作结构化判断，发布由宿主核对并执行。
import "std/agent.tl" as Agent;

pub enum ReviewDecision {
    approve { proposal_id: string, source_digest: string, cited_refs: string[] },
    reject { proposal_id: string, source_digest: string, reason: string },
    needs_evidence { proposal_id: string, source_digest: string, reason: string },
}

agent KnowledgeReviewer {}

impl KnowledgeReviewer {
    pub task review(packet: string) -> ReviewDecision {
        let options = self.options();
        options.model_rounds = 1;
        options.tool_calls = 1;
        options.max_output_tokens = 1024;
        options.recall_memory = false;
        options.propose_memory = false;
        options.tools = [];
        options.instructions = "Review the proposed knowledge against the supplied evidence only. Approve only durable, relevant, non-sensitive facts with exact evidence references. Reject unsupported, contradictory, duplicate, or secret-bearing content. Return needs_evidence when the supplied evidence cannot settle the claim. Do not rewrite the proposed body. Submit one ReviewDecision through __teal_complete.";
        return self.think(packet, options).await;
    }
}

task application() -> none | Error {
    let reviewer = KnowledgeReviewer {};
    let input = Agent::receive().await ?;
    match input {
        Agent::Input.text { value } => {
            let verdict = reviewer.review(value).await;
            match verdict {
                ReviewDecision.approve { proposal_id, source_digest, cited_refs } => {
                    Agent::emit("proposal_id", proposal_id);
                    Agent::emit("source_digest", source_digest);
                    for reference in cited_refs {
                        Agent::emit("cited_ref", reference);
                    }
                    Agent::emit("decision", "approve");
                }
                ReviewDecision.reject { proposal_id, source_digest, reason } => {
                    Agent::emit("proposal_id", proposal_id);
                    Agent::emit("source_digest", source_digest);
                    Agent::emit("reason", reason);
                    Agent::emit("decision", "reject");
                }
                ReviewDecision.needs_evidence { proposal_id, source_digest, reason } => {
                    Agent::emit("proposal_id", proposal_id);
                    Agent::emit("source_digest", source_digest);
                    Agent::emit("reason", reason);
                    Agent::emit("decision", "needs_evidence");
                }
            }
        } _ => {
            Agent::emit("decision", "needs_evidence");
        }
    }
}

/// 入口显式处理应用错误，避免把失败当作成功结束。
task main() -> int {
    let result = application().await;
    match result {
        Error { .. } => {
            print(result.message);
            return 1;
        } _ => {
            return 0;
        }
    }
}
