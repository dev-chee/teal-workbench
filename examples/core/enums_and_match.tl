@edition("2026");

// Tier Core：枚举判别式、带字段变体、穷尽 match、解构与通配臂。
enum Direction {
    north = -1,
    south,
    east,
    west = 10,
}

enum Action {
    reply { to: string, tone: string },
    count { value: int },
    ratio { value: float },
    archive = 20,
}


task main() {
    let direction = Direction.east;
    let result = "";

    match direction {
        Direction.north => { result = "north"; }
        Direction.south => { result = "south"; }
        Direction.east => { result = "east"; }
        Direction.west => { result = "west"; }
    }

    let reply = Action.reply {
        to: "team",
        tone: "warm",
    };
    let count = Action.count { value: 3 };
    let ratio = Action.ratio { value: .5 };
    let archive = Action.archive;

    match reply {
        Action.reply { to, tone: style } => { print(to, style); }
        _ => { print("not a reply"); }
    }

    print(result);
    print(int(Direction.north), int(Direction.east), int(Direction.west));
    print("archive tag=${archive}");
    print(typeof(reply), typeof(count), typeof(ratio), typeof(archive));
}
