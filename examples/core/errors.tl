@edition("2026");
// 可恢复错误是普通 union 值，致命 VM 错误不参与这里的分支处理。
fn missing() -> int | Error {
    return Error::make("core", "missing", ErrorOrigin.user, false);
}

task application() -> none | Error {
    let missing_result = missing();
    match missing_result {
        Error { message, .. } => {
            if message == "missing" {
                print("handled missing");
            } else {
                print(message);
            }
        } _ => {}
    }
    let result = [1, 2].checked_get(99);
    match result {
        Error { kind, .. } => {
            if kind == "index_out_of_bounds" {
                print("handled bounds error");
            }
        } _ => {}
    }
}

/// 入口明确处理失败；未知外部效果仍由 VM 终止。
task main() -> int {
    let result = application().await;
    match result {
        Error { .. } => {
            print(result.message);
            return 1;
        } _ => {
            return 0;
        }
    }
}
