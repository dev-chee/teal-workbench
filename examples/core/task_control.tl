@edition("2026");

// Tier Core：演示纯 CPU Task 的取消、失败传播、同级任务收集和终态查询。
// 该示例不声明 Agent，也不需要 agent 扩展。
import "std/error.tl" as errors;
task value(number: int) -> int { return number; }
task fail() -> int { throw(errors::make("core", "expected_failure", errors::ErrorOrigin.user, false)); return 0; }

task main() { workflow().await; }

task workflow() {
    // 同一 VM 尚未切换到子任务时取消，结果不依赖线程竞速。
    let cancelled: Task<int> = value(99);
    cancelled.cancel();
    print(cancelled.status());
    try { print(cancelled.await); }
    catch "task_cancelled" { print("handled cancellation"); }

    let broken = fail();
    let sibling = value(42);
    try { print(broken.await); }
    catch "expected_failure" { print("handled failure"); }
    // 一个分支失败不会自动取消同级分支，调用方仍需收集或取消它。
    print(sibling.await, broken.status(), sibling.status());
}
