@edition("2026");

// Tier Host：声明可选的本机 C ABI；默认示例只编译，不加载真实动态库。
// 实际执行依赖目标平台、完全匹配的符号以及显式库和符号授权。
dylib "native" {
    int add(int, int);
    float distance(float, float, float, float);
    log(string);
}


task main() {
    print("Use `teal c` for this declaration-only example.");
}
