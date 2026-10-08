@edition("2026");

// Tier Core：自定义 Attribute 的目标类别与 reflect::item 投影选择器。
// 每次属性应用构造一个隐藏全局实例，同一 VM 内的多次投影共享它的对象身份。

@attribute
class Tag {
    label: string,
}

@Tag { label: "function" }
fn helper() -> int {
    return 1;
}

@Tag { label: "static" }
static counter = 5;

class Point {
    value: int,
}

impl Point {
    @Tag { label: "method" }
    fn describe() -> string {
        return "point";
    }

    @Tag { label: "associated" }
    static fn origin() -> Point {
        return Point { value: 0 };
    }
}

@Tag { label: "class" }
class Model {
    id: int,
}

@Tag { label: "enum" }
enum State {
    idle,
    busy,
}

@Tag { label: "class-agent" }
class Worker {
    runs: int,
}

task main() {
    let function_label = reflect::item(fn helper).@Tag;
    let static_label = reflect::item(static counter).@Tag;
    let method_label = reflect::item(Point::describe).@Tag;
    let associated_label = reflect::item(Point::origin).@Tag;
    let class_label = reflect::item(class Model).@Tag;
    let enum_label = reflect::item(enum State).@Tag;
    let agent_label = reflect::item(class Worker).@Tag;

    print(function_label.label, static_label.label, method_label.label, associated_label.label);
    print(class_label.label, enum_label.label, agent_label.label);
    // 投影只解析元数据，被标注的声明仍按普通规则调用。
    print(helper(), Point::origin().describe());
}
