@edition("2026");

// Tier Host：控制台输入、程序参数和退出状态都由嵌入宿主提供。
// 自动示例通过 stdin 提供一行文本，并期待显式退出码 7。
task main() {
    let name = input("Name: ");
    print(name);
    let arguments = argv();
    print(arguments.len(), arguments[0], arguments[1]);
    print(42);
    exit(7);
}
