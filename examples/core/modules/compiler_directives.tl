@edition("2026");

// Tier Core 辅助模块：导入文件中的构建标量指令会被完整校验，但不改变根文件构建结果。

@optimization("none");
@compiler_debug(true);
@debug_info(false);

pub fn value() -> int {
    return 7;
}
