@edition("2026");

// Tier Core：命名函数、递归、函数值、匿名闭包和外层局部捕获。
fn add(left, right) -> int {
    return left + right;
}

fn typed_add(left: int, right: int) -> int {
    return left + right;
}

task announce(message: string) {
    print(message);
    return;
}

fn factorial(value: int) -> int {
    if value <= 1 {
        return 1;
    }
    return value * factorial(value - 1);
}

// 闭包可以离开声明帧，同时保留共享状态。
fn make_counter(start: int) -> fn(int) -> int {
    let value = start;
    return fn(step: int) {
        value = value + step;
        return value;
    };
}


// 需要递归的局部算法提升为模块函数，原捕获数据改为显式参数。
fn countdown(value: int, offset: int) -> int {
    if value <= 0 {
        return offset;
    }
    return countdown(value - 1, offset);
}

task main() {
    let offset = 1;
    // 块内使用匿名闭包；变量的函数类型保留显式返回契约。
    let local_multiply: fn(int, int) -> int = fn(left: int, right: int) {
        return left * right + offset;
    };

    let double = fn(value: int) {
        return value * 2;
    };

    // 每次求值都会创建独立的闭包实例。
    let first_counter = make_counter(10);
    let second_counter = make_counter(100);

    // 集合回调同样支持捕获变量。
    let scale = 3;
    let scaled = [1, 2, 3].map(fn(value: int) {
        return value * scale;
    });

    let procedure: fn();
    let transform: fn(int) -> int;

    announce("named function").await;
    print(add(2, 3), typed_add(4, 5));
    print(local_multiply(3, 4), countdown(3, offset), double(6), factorial(5));
    print(first_counter(1), first_counter(2), second_counter(1));
    print(scaled);
    print(typeof(first_counter));
    print(typeof(procedure), typeof(transform));
}
