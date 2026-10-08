@edition("2026");

// Tier Core 负例：同步打印结果不能作为任务等待。

fn main() {
    print("not a task").await;
}
