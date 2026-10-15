@edition("2026");

// Tier Agent：两个独立模型实例并行审查；执行需要 Codex 认证并会产生真实请求。
// 编译检查不会登录，也不会联系模型供应商。
import "std/agent.tl" as Agent;

@model("codex", "gpt-5.6-terra")
agent Reviewer {
    specialty: string,
    reviews: int,
}

impl Reviewer {
    pub task review(text: string) -> string {
        self.reviews += 1;
        let specialty = self.specialty;
        let round = self.reviews;
        let prompt = "You are a code-review Agent. Focus on {specialty}. " + "This is review round {round}. Review the following content and return only a concise conclusion: {text}";
        let options = self.options();
        options.model_rounds = 1;
        options.propose_memory = false;
        return self.think(prompt, options).await;
    }
}

task main() {
    entry_task().await;
}

task entry_task() {
    let correctness = Reviewer {
        specialty: "correctness and boundary conditions",
        reviews: 0,
    };
    let security = Reviewer {
        specialty: "security and authorization boundaries",
        reviews: 0,
    };

    // 两个长期 Agent 拥有独立状态和模型历史，并在全局并发额度内并行执行。
    let input = "A function accepts a path and immediately deletes the corresponding file";
    let correctness_task = correctness.review(input);
    let security_task = security.review(input);
    let reports = Task::all([correctness_task, security_task]).await;

    print(reports);
    print(correctness.status());
    print(security.status());
}
