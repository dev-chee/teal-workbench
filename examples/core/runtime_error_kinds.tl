@edition("2026");
// checked 操作显式返回 Error；普通 VM 错误仍终止执行。
fn report(value: int | float | bool | string | int[] | Error) {
    match value {
        Error { kind, .. } => {
            print(kind);
        } _ => {}
    }
}

task main() {
    report(checked_int("not-a-number"));
    report(checked_float("not-a-number"));
    report(checked_bool("not-a-number"));
    report([1, 2].checked_get(9));
    report([1, 2].checked_slice(2, 1));
    report({ "present": 1 }.checked_get("missing"));
    report(checked_div(10, 0));
    report(checked_mod(10, 0));
    report(checked_pow(2, -1));
    report("ab".checked_repeat(-1));
}
