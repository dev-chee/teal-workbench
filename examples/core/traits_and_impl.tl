@edition("2026");

// Tier Core：固有 impl、Trait、关联函数、Self、UFCS 与 impl Trait 返回。
trait Named {
    fn name() -> string;
}

trait Active {
    fn active() -> bool;
}

trait Label {
    fn name() -> string;
}

class Counter {
    value: int,
}

impl Counter {
    fn increment() -> int {
        self.value += 1;
        return self.value;
    }

    static fn zero() -> Self {
        return Self { value: 0 };
    }
}

enum State {
    ready,
    finished,
}

impl Named for State {
    fn name() -> string {
        match self {
            State.ready => { return "ready"; }
            State.finished => { return "finished"; }
        }
    }
}

impl Active for State {
    fn active() -> bool {
        match self {
            State.ready => { return true; }
            State.finished => { return false; }
        }
    }
}

impl Label for State {
    fn name() -> string { return "state label"; }
}

fn describe(value: impl Named + Active) -> string {
    if value.active() {
        return value.name();
    }
    return "inactive";
}

fn initial_state() -> impl Named + Active {
    return State.ready;
}


task main() {
    let counter = Counter::zero();
    print(counter.increment());
    print(describe(initial_state()));
    // 同名特征方法使用 UFCS 明确选择实现。
    print(<State as Named>::name(State.ready));
    print(<State as Label>::name(State.ready));
}
