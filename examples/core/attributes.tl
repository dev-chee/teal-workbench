@edition("2026");

// Tier Core：class/enum Attribute 实例、隐藏全局状态、嵌套类型和类型投影。
class Metadata {
    source: string,
}

@attribute
class Route {
    method: string,
    path: string,
    metadata: Metadata,
}

impl Route {
    fn describe() -> string {
        return self.method + " " + self.path + " from " + self.metadata.source;
    }
}

@attribute
enum Mode {
    fast,
    safe { level: int },
}

fn metadata(source: string) -> Metadata {
    return Metadata { source: source };
}

@Route {
    method: "GET",
    path: "/status",
    metadata: metadata("example"),
}
@Mode.safe { level: 2 }
task main() {
    let route = reflect::item(main).@Route;
    let mode = reflect::item(main).@Mode;
    print(route.describe());
    route.path = "/health";
    print(reflect::item(main).@Route.path);
    match mode {
        Mode.fast => { print("fast"); }
        Mode.safe { level } => { print(level); }
    }
}
