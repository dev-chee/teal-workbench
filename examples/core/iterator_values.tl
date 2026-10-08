@edition("2026");

// Tier Core：内建集合的 iter()、惰性适配器、collect() 与 for 直接驱动迭代器。
class Box {
    value: int,
}

fn doubled(value: int) -> int {
    return value * 2;
}

fn even(value: int) -> bool {
    return value % 2 == 0;
}

task main() {
    let values = [1, 2, 3, 4];

    // 数组、字符串和映射都能产生迭代器；字符串按 Unicode 标量推进。
    print(values.iter().map(doubled).collect());
    print("ab c".iter().collect());
    print({ "x": 1, "y": 2 }.iter().collect().len());

    // filter 后接 map 的链式顺序，以及 take 的数量上限。
    let chained = values
        .iter()
        .filter(even)
        .map(fn(value: int) { return value + 100; })
        .take(1)
        .collect();
    print(chained);

    // 适配器是惰性的：未被消费前不调用回调；take 达到数量后不再读取源。
    let calls = 0;
    let lazy = values.iter().map(fn(value: int) {
        calls += 1;
        return value;
    });
    print(calls);
    print(lazy.take(2).collect(), calls);

    // for 直接驱动迭代器，元素可以是类实例。
    let boxes = [Box { value: 1 }, Box { value: 2 }];
    let total = 0;
    for item in boxes.iter().map(fn(item: Box) { return Box { value: item.value * 10 }; }) {
        total += item.value;
    }
    print(total);

    // 迭代器是单次消费值：第二次遍历只剩未消费的元素。
    let remaining = values.iter();
    print(remaining.collect().len(), remaining.collect().len());

    // 迭代器的静态类型写作 Iterator<T>。
    print(typeof(values.iter()), typeof(values.iter().map(doubled)));

    // skip/take_while/skip_while/chain 与终结方法共享同一份惰性阶段。
    print(values.iter().skip(2).collect());
    print(values.iter().take_while(fn(value: int) { return value < 3; }).collect());
    print(values.iter().skip_while(fn(value: int) { return value < 3; }).collect());
    print(values.iter().chain([9, 8].iter()).collect());
    print(values.iter().count(), values.iter().any(even), values.iter().all(fn(value: int) { return value > 0; }));
    print(values.iter().find(fn(value: int) { return value > 2; }));
    print(values.iter().fold(0, fn(total: int, value: int) { return total + value; }));
}
