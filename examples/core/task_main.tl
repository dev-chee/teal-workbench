@edition("2026");

// Tier Core：task main 作为异步入口，直接等待 Task<T> 并返回进程退出码。
task add(left: int, right: int) -> int {
    return left + right;
}

task main() -> int {
    let pending: Task<int> = add(20, 22);
    print(pending.await);
    return 0;
}
