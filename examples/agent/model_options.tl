@edition("2026");

// Tier Agent：模型限制、补充上下文和类型化完成结果。
// 需要已配置的 Codex 认证；示例会执行真实模型调用。
import "std/agent.tl" as Agent;

enum Review {
    accepted { summary: string },
    changes_required { details: string },
}

@model("codex", "gpt-5.6-terra")
@options({"effort": "high", "max_output_tokens": 2048})
agent Reviewer {}

impl Reviewer {
    pub task review(text: string) -> Review {
        let options = self.options();
        options.context = ["Check correctness and authorization boundaries."];
        options.model_rounds = 4;
        options.max_output_tokens = 1024;
        options.effort = "low";
        options.propose_memory = false;
        return self.think("Review this change and submit a structured verdict through __teal_complete: " + text, options).await;
    }
}

task main() { workflow().await; }

task workflow() {
    let reviewer = Reviewer {};
    let result = reviewer.review("A function deletes a caller-provided file path.").await;
    match result {
        Review.accepted { summary } => { print(summary); }
        Review.changes_required { details } => { print(details); }
    }
}
