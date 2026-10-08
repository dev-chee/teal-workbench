@edition("2026");

// Tier Core：命名、方法和匿名 CPU task，以及 Task<T> 和 await。
// task 与 fn 共用参数、返回、递归、方法和调用规则，同时保留可恢复帧身份。
task factorial(value: int) -> int {
    if value <= 1 { return 1; }
    return value * factorial(value - 1).await;
}

class Multiplier { factor: int }

impl Multiplier {
    task apply(value: int) -> int {
        return self.factor * value;
    }
}

task main() { entry_task().await; }

task entry_task() {
    // 命名 task 仅作模块声明或 impl 成员；块内使用保留任务身份的匿名表达式。
    let identity: task(int) -> int = task(value: int) { return value; };
    let multiplier = Multiplier { factor: 1 };
    print(identity(multiplier.apply(factorial(5).await).await).await);
}
