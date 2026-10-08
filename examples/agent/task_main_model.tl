@edition("2026");

// Tier Agent：task main 直接等待带模型调用的 Agent task。
// 需要已配置的 Codex 认证；自动示例门禁只执行编译检查，不发送模型请求。
import "std/agent.tl" as Agent;

@model("codex", "gpt-5.6-terra")
agent Summarizer {}

impl Summarizer {
    pub task summarize(text: string) -> string {
        let options = self.options();
        options.model_rounds = 1;
        options.propose_memory = false;
        return self.think(
            "Summarize the following text in one sentence and submit it through __teal_complete: " + text,
            options,
        ).await;
    }
}

task main() -> int {
    let summarizer = Summarizer {};
    print(summarizer.summarize("Teal task main can await Agent work directly.").await);
    return 0;
}
