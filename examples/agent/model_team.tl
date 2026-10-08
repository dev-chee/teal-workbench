@edition("2026");

// Tier Agent：模型可见的受控 class 工厂、委派消息、游标确认、查询和打断工具。
// 模型动态 spawn 还要求宿主通过 RuntimeServices::with_agent_factories(["Worker"])
// 明确开放已编译 Worker；白名单不会加载新源码或扩大工具权限。
import "std/agent.tl" as Agent;

class Worker { total: int }

impl Worker {
    @allow("unused-private-method")
    pub task calculate(value: int) -> int {
        self.total += value;
        return self.total;
    }
}

@model("codex", "gpt-5.6-terra")
agent Coordinator {}

impl Coordinator {
    pub task coordinate() -> string {
        let options = self.session_options(self.session("team"));
        options.context = [
            "Only spawn the compiled Worker definition.",
            "The collaboration surface is spawn_agent, send_message, followup_task, wait_agent, ack_messages, interrupt_agent, and list_agents.",
            "Use wait_agent to read messages and ack_messages with its returned cursor; use list_agents to verify the final state.",
        ];
        return self.think(
            "Create a Worker with total 0, delegate a calculation, acknowledge its completion message, and submit a short result.",
            options,
        ).await;
    }
}

task main() { entry_task().await; }

task entry_task() {
    let coordinator = Coordinator {};
    print(coordinator.coordinate().await);
}
