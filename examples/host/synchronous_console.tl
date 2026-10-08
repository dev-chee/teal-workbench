@edition("2026");

// Tier Host：普通函数直接取得输入，并在返回前完成输出。
fn greet() {
    let name = input("Name: ");
    print("Hello", name);
}

fn main() {
    greet();
    print();
    print(42);
}
