@edition("2026");

// Tier Core：用户实现的 Iterator（含 task 成员）共用同一套适配器与终结方法。
class Steps {
    remaining: int,
}

impl Iterator for Steps {
    fn next() -> int | none {
        if self.remaining <= 0 {
            return none;
        }
        self.remaining -= 1;
        return self.remaining + 1;
    }
}

class Slow {
    remaining: int,
}

impl Iterator for Slow {
    task next() -> int | none {
        if self.remaining <= 0 {
            return none;
        }
        self.remaining -= 1;
        print("step", self.remaining + 1);
        return self.remaining + 1;
    }
}

fn total(total: int, value: int) -> int {
    return total + value;
}

task main() {
    // 同步实现：适配器、终结方法与内建来源完全一致。
    print(Steps { remaining: 5 }.skip(2).collect());
    print(Steps { remaining: 5 }.take(2).collect());
    print(Steps { remaining: 5 }.take_while(fn(value: int) { return value > 2; }).collect());
    print(Steps { remaining: 5 }.skip_while(fn(value: int) { return value > 2; }).collect());
    print(Steps { remaining: 3 }.chain(Steps { remaining: 2 }).collect());
    print(Steps { remaining: 4 }.fold(0, total), Steps { remaining: 5 }.count());
    print(Steps { remaining: 5 }.any(fn(value: int) { return value == 3; }), Steps { remaining: 5 }.all(fn(value: int) { return value > 0; }));
    print(Steps { remaining: 5 }.find(fn(value: int) { return value == 3; }));
    print(Steps { remaining: 5 }.map(fn(value: int) { return value * 10; }).skip(1).take(2).collect());

    // task 成员：每一步都完成一次 next 任务后才交付元素，顺序可观察。
    print(Slow { remaining: 2 }.collect());
}
