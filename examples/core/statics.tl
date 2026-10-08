@edition("2026");

// Tier Core：模块级 static 的初始化、读取与修改。
fn initial_value() -> int {
    return 40;
}

static base = initial_value();
static answer = base + 2;
static message = "module state";

fn increment() {
    answer += 1;
}


task main() {
    print(base, answer, message);
    increment();
    answer = answer + 1;
    print(answer);
}
