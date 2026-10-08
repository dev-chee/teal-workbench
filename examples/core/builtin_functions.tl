@edition("2026");

// Tier Core：覆盖不依赖平台服务的内建类型查询、转换与范围生成。
// 转换要求完整字符串有效；失败不会静默使用默认值。
task main() {
    print(typeof(1), float("1.5"), int("42"), string(true), bool("false"));
    print(range(0, 3), range(2, 5));
}
