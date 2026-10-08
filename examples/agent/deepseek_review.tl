@edition("2026");

// Tier Agent：使用服务的固定 DeepSeek 模型绑定，返回类型化模型结果。
// 先运行 teal auth login deepseek；执行会发送真实模型请求，编译检查不会联网。
import "std/agent.tl" as Agent;

@model("deepseek", "deepseek-v4-flash")
agent Reviewer {}

impl Reviewer {
    pub task review(text: string) -> string {
        let options = self.options();
        options.model_rounds = 4;
        options.max_output_tokens = 2048;
        options.propose_memory = false;
        return self.think("Review this change and submit a concise verdict through __teal_complete: " + text, options).await;
    }
}

task main() { workflow().await; }

task workflow() {
    let reviewer = Reviewer {};
    print(reviewer.review("A function validates a file path before reading it.").await);
}
