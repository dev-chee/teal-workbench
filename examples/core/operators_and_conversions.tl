@edition("2026");

// Tier Core：算术、比较、逻辑、赋值运算符和显式类型转换。
// 幂运算按右结合解析；转换失败会抛出稳定的运行时错误。
task main() {
    let arithmetic = (2 + 3) * 4 - 5 / 5;
    let remainder = 17 % 5;
    let power = 2 ^ 3 ^ 2;
    let negative = -arithmetic;

    let comparisons =
        1 < 2 && 2 <= 2 && 3 > 2 && 3 >= 3 && 4 == 4 && 4 != 5;
    let logic = !false || false;
    let joined = "Teal" + " language";

    let value = 10;
    value += 5;
    value -= 3;
    value *= 2;
    value /= 4;
    value %= 4;
    value ^= 3;
    value = value + 1;

    let as_float = float(12);
    let parsed_float = float("3.5");
    let as_int = int(4.8);
    let parsed_int = int("9");
    let as_string = string(true);
    let parsed_bool = bool("true");

    // 所有浮点 -0.0 都被规范化为 0.0。
    let zero = -0.0;

    print(arithmetic, remainder, power, negative);
    print(comparisons, logic, joined, value);
    print(as_float, parsed_float, as_int, parsed_int, as_string, parsed_bool, zero);
    print(typeof(value));
}
