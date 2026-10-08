@edition("2026");

// Tier Core：用户自定义迭代协议（Iterable/Iterator）驱动 for 循环。
class Countdown {
    remaining: int,
}

impl Iterable for Countdown {
    fn iter() -> Countdown {
        return self;
    }
}

impl Iterator for Countdown {
    fn next() -> int | none {
        if self.remaining <= 0 {
            return none;
        }
        self.remaining -= 1;
        return self.remaining + 1;
    }
}

class Stack {
    items: int[],
}

class StackIter {
    items: int[],
    cursor: int,
}

impl Iterable for Stack {
    fn iter() -> StackIter {
        return StackIter { items: self.items, cursor: 0 };
    }
}

impl Iterator for StackIter {
    fn next() -> int | none {
        if self.cursor >= self.items.len() {
            return none;
        }
        let index = self.cursor;
        self.cursor += 1;
        return self.items[index];
    }
}


task main() {
    let countdown = Countdown { remaining: 3 };
    let ascending = 0;
    for value in countdown {
        ascending = ascending * 10 + value;
    }
    print(ascending);

    let stack = Stack { items: [4, 5, 6] };
    let total = 0;
    for value in stack {
        if value == 5 {
            continue;
        }
        total += value;
    }
    print(total);
}
