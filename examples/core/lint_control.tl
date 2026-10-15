@edition("2026");
@lint { warn: ["correctness"], allow: ["naming", "style", "unused-parameter"] };

// Tier Core：文件级与声明级 lint 控制、检查项分组和前导下划线抑制。
// 级别为 allow < warn < deny < forbid；warn 只上报，deny 和 forbid 在生成阶段失败。

// 声明级 allow 收窄本声明的默认级别；文件级设置和 CLI 覆盖对它仍然有效。

fn describe(Label: string, value: int) -> string {
    return "describe";
}

task main() {
    // 前导下划线抑制 unused 绑定；精确的 `_` 才是丢弃绑定。
    let _ignored_result = describe("ignored", 1);
    print(describe("label", 2));
}
