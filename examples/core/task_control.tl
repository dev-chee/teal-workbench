@edition("2026");
// 取消由任务组的 Error 结果观察；返回 Error 值的任务正常完成，不冒充 VM 失败。
task value(number: int) -> int {
    return number;
}

task fail() -> int | Error {
    return Error::make("core", "expected_failure", ErrorOrigin.user, false);
}

task application() -> none | Error {
    let group = Task::group();
    let cancelled = value(99);
    group.add(cancelled);
    cancelled.cancel();
    print(cancelled.status());
    let joined = group.join().await;
    match joined {
        Error { .. } => {
            print("handled cancellation");
        } _ => {}
    }
    let broken = fail();
    let sibling = value(42);
    let result = broken.await;
    match result {
        Error { .. } => {
            print("handled failure");
        } _ => {}
    }
    print(sibling.await, broken.status(), sibling.status());
}

/// 入口明确处理失败；未知外部效果仍由 VM 终止。
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
