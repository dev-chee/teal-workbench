@edition("2026");

// Tier Core：字面量、转义与插值字符串，以及局部绑定的声明、推演和 shadow。
// 单行注释会被词法分析器忽略。

task main() {
    let integer = 42;
    let minimum = -2147483648;
    let decimal = .5;
    let enabled = true;
    let disabled = false;
    let escaped = "line 1\nline 2\t\"quoted\"\\path\r\0";

    // 声明可以省略或指定类型，也可以省略或提供初始化器。
    let inferred;
    let typed: int;
    let initialized = "ready";
    let typed_and_initialized: float = 1.25;
    let language = "Teal";
    let release = 1;
    let interpolated = "Welcome to {language} {release}";
    let wrapped = "{{{language}}}";

    // 首次赋值确定推演类型；显式类型无需初始化即可查询。
    inferred = 7;
    print(typeof(inferred));
    print(typeof(typed));

    typed = 8;
    print(integer, minimum, decimal, enabled, disabled);
    print(escaped);
    print(inferred, typed, initialized, typed_and_initialized);
    print(interpolated, wrapped);

    // 同名 let 创建一个新绑定，可以像 Rust 一样 shadow 并使用另一种类型。
    let typed = "shadowed";
    print(typeof(typed), typed);
}
