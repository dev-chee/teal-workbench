@edition("2026");

// Tier Core：throw、按错误文本筛选的 catch，以及捕获全部错误的绑定。
import "std/error.tl" as errors;
task main() {
    try {
        throw(errors::make("core", "missing", errors::ErrorOrigin.user, false));
    } catch "missing" {
        print("handled missing");
    } catch "denied" {
        print("handled denied");
    } catch error {
        print(error);
    }

    try {
        let values = [1, 2];
        print(values[99]);
    } catch "index_out_of_bounds" {
        print("handled bounds error");
    } catch error {
        print("unexpected: " + error);
    }
}
