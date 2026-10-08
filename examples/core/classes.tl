@edition("2026");

// Tier Core：class 构造、字段读取/修改、嵌套值与引用别名语义。
class Point {
    x: int,
    y: int,
}

class Segment {
    start: Point,
    end: Point,
}


task main() {
    let x = 3;
    let point = Point {
        x: x,
        y: x + 1,
    };
    let segment = Segment {
        start: point,
        end: Point { x: 8, y: 9 },
    };

    point.x = 10;
    segment.end.y = 12;

    print(point.x, point.y, point.x + point.y);
    print(segment.start.x, segment.end.y);
}
