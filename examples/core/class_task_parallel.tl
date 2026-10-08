@edition("2026");

// Tier Core：class 的实例状态、并行 task 和顺序调用。
// 本例不包含模型调用，因此保持确定性并可离线运行。
class Counter {
    value: int,
}

impl Counter {
    pub task add(delta: int) -> int {
        self.value += delta;
        return self.value;
    }
}

task main() { entry_task().await; } task entry_task() {
    let left = Counter { value: 10 };
    let right = Counter { value: 100 };

    // 不同实例并行；同一实例的委派仍按邮箱顺序串行。
    let left_first = left.add(2);
    let right_first = right.add(5);
    let left_second = left.add(3);

    let left_values = Task::all([left_first, left_second]).await;
    let right_value: int = right_first.await;
    print(left_values);
    print(right_value);
}
