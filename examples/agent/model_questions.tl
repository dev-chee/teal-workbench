@edition("2026");

// Tier Agent：模型自主提问、持久暂停与回答后的受控继续。
// 需要已配置的 Codex 认证，会产生真实模型请求；CLI 为模型问答保存持久检查点。
import "std/agent.tl" as Agent;

enum Plan { ready { summary: string }, cancelled }

@model("codex", "gpt-5.6-terra")
agent Planner {}

impl Planner {
    pub task plan() -> Plan {
        let options = self.options();
        options.model_rounds = 6;
        options.tool_calls = 8;
        options.propose_memory = false;
        return self.think("Before drafting a plan, use __teal_ask_user alone to ask which topic to cover. Use empty choices for a text question. Then submit the result through __teal_complete alone: cancelled if the user skips, otherwise a short ready plan. Do not ask for credentials or permissions.", options).await;
    }
}

task main() { workflow().await; }

task workflow() {
    let planner = Planner {};
    match planner.plan().await {
        Plan.ready { summary } => { print(summary); }
        Plan.cancelled => { print("No plan requested"); }
    }
}
