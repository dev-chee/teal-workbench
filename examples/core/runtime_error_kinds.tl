@edition("2026");

// Tier Core：可从源码触发的内建运行时错误种类与稳定错误文本。
// 语法和静态类型错误发生在 VM 启动前，不能用 try 捕获，因此不在本示例内。
fn zero() -> int {
    return 0;
}

fn negative() -> int {
    return -1;
}

task main() {
    let text = "not-a-number";
    try {
        print(int(text));
    } catch "invalid_int" {
        print("invalid_int");
    }
    try {
        print(float(text));
    } catch "invalid_float" {
        print("invalid_float");
    }
    try {
        print(bool(text));
    } catch "invalid_bool" {
        print("invalid_bool");
    }

    let values = [1, 2];
    let index = 9;
    try {
        print(values[index]);
    } catch "index_out_of_bounds" {
        print("index_out_of_bounds");
    }
    try {
        print(values[2..1]);
    } catch "slice_out_of_bounds" {
        print("slice_out_of_bounds");
    }

    let table = {"present": 1};
    let key = "missing";
    try {
        print(table[key]);
    } catch "unknown_map_key" {
        print("unknown_map_key");
    }

    // 字面量零在编译期就被拒绝，需要运行时值才能触发这些种类。
    let divisor = zero();
    try {
        print(10 / divisor);
    } catch "division_by_zero" {
        print("division_by_zero");
    }
    try {
        print(10 % divisor);
    } catch "modulo_by_zero" {
        print("modulo_by_zero");
    }

    let exponent = negative();
    try {
        print(2 ^ exponent);
    } catch "negative_exponent" {
        print("negative_exponent");
    }
    let count = negative();
    try {
        print("ab".repeat(count));
    } catch "negative_repeat" {
        print("negative_repeat");
    }
}
