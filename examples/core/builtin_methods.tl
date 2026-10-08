@edition("2026");

// Tier Core：覆盖字符串、数组、映射和数字的全部内建方法。
// 字符串变换返回新值；数组的 sort、reverse、remove 原地修改。
task main() {
    let text = " Teal ";
    print(text.trim(), text.trim_left(), text.trim_right());
    print("Teal".uppercase(), "Teal".lowercase());
    print("teal".starts_with("te"), "teal".ends_with("al"), "teal".contains("ea"));
    print("teal".replace("ea", "oo"), "teal".find("ea"), "teal".find("x"));
    print("..teal..".trim_sequence("."), "..teal..".trim_sequence_left("."), "..teal..".trim_sequence_right("."));
    print("42".is_int(), "3.5".is_float(), "x".is_int());
    print("ab".repeat(2), "abc".reverse(), "a,b,c".split(",").join("-"));
    print("abc".map(fn(c: string) { return c.uppercase(); }));
    print("abc".filter(fn(c: string) { return c != "b"; }));

    let numbers = [3, 1, 2];
    print(numbers.contains(2), numbers.find(2), numbers.find(9));
    print([1, 2].repeat(2), [1, 0, 2, 0, 3].partition(0));
    numbers.sort();
    print(numbers);
    numbers.reverse();
    numbers.remove(1);
    print(numbers, numbers.filter(fn(n: int) { return n > 1; }));
    let lookup = {"ready": 1};
    print(lookup.get("ready"));
    print(["a", "b"].join(), ["a", "b"].join("/"));
    print((-3).abs(), (-2.5).abs(), 9.0.sqrt(), 2.6.round(), (-2.1).floor());
}
