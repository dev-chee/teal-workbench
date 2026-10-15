@edition("2026");

// Tier Core：丢弃绑定与 Unicode 字符索引/UTF-8 字节偏移边界。
// 丢弃绑定仍求值并保留副作用，但不会创建可读取的变量。
fn ignore( _: int, value: int) -> int {
    return value;
}

task main() {
    let _ = print("discard declaration");
    let _: int = ignore(1, 42);
    _ = print("discard assignment");
    match Error::make("core", "ignored", ErrorOrigin.user, false) {
        Error { .. } => {
            print("discard error");
        }
    }

    // 此处需要实际非 ASCII 字符：Teal 不把反斜杠 u 当作 Unicode 转义。
    // 索引和切片按 Unicode 标量计数，len 和 find 按 UTF-8 字节计数。
    let text = "café!";
    print(text[3], text[3..4], text.len(), text.find("!"));
    print(text[4], text.reverse());
}
