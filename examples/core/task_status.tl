@edition("2026");

// Tier Core：普通 class task 的结果与句柄状态。
// 确定性 task 的取消和终态查询使用 task 句柄，不需要 Agent 实例。
class Worker { count: int }

impl Worker {
    pub task read() -> int { return self.count; }
}

task main() { workflow().await; }

task workflow() {
    let worker = Worker { count: 7 };
    let pending = worker.read();
    let state = pending.status();
    print(pending.await, state);
}
