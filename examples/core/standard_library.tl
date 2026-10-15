@edition("2026");

// 常量和通用标准库均可用于纯计算；print 仅负责展示结果。
import "std/math.tl" as math;
import "std/encoding.tl" as encoding;
import "std/path.tl" as path;

const DEFAULTS = [2, 4, 6];

task application() -> none | Error {
    const LIMIT = 10;
    print((12).clamp(0, LIMIT), math::PI > 3.0, DEFAULTS.all(fn(value: int) {
        return value > 0;
    }));
    print(DEFAULTS.fold(0, fn(total: int, value: int) {
        return total + value;
    }));
    let bytes = encoding::utf8_encode("Teal");
    print(encoding::hex_encode(bytes) ?, encoding::base64_encode(bytes) ?);
    print(path::normalize("C:/work/../data/file.txt", path::Style.windows) ?);
    let alias = DEFAULTS;
    let result = alias.checked_push(8);
    match result {
        Error { .. } => {
            print("constant preserved");
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
