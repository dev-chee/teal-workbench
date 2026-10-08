@edition("2026");

// Tier Core 负例：未初始化绑定只能用于编译期 typeof，不能求值。
fn main() {
    let value: int;

    // 未初始化值仍带有 NONE 标志，不能参与打印或其他运行时计算。
    print(value);
}
