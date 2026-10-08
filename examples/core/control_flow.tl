@edition("2026");

// Tier Core：语句/表达式 if、块作用域、loop、for、标签及跳转。
task main() {
    let label = "";
    if true {
        label = "if";
    } else if false {
        label = "else if";
    } else {
        label = "else";
    }

    let sign = if -3 < 0 {
        "negative"
    } else if -3 == 0 {
        "zero"
    } else {
        "positive"
    };

    let counter = 0;
    'counter_loop: loop {
        if counter >= 4 {
            break 'counter_loop;
        }
        counter += 1;
        if counter == 2 {
            continue 'counter_loop;
        }
        if counter == 4 {
            break 'counter_loop;
        }
    }

    let loop_count = 0;
    loop {
        loop_count += 1;
        if loop_count >= 2 {
            break;
        }
    }

    let collection_total = 0;
    'items: for item in [1, 2, 3] {
        loop {
            collection_total += item;
            continue 'items;
        }
    }

    let range_total = 0;
    for i in ..3 {
        range_total += i;
    }
    for i in 3..5 {
        range_total += i;
    }

    {
        let scoped = "nested block";
        print(scoped);
    }

    print(label, sign, counter, loop_count, collection_total, range_total);
}
