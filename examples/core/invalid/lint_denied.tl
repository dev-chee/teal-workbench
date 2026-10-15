@edition("2026");

// Tier Core 负例：deny 把检查项提升为编译错误，不能当作可忽略的警告。

@lint { deny: ["unused-variable"] };

fn main() {
    let unused_value = 1;
    print("the denied lint stops code generation");
}
