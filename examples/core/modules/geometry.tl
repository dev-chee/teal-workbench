@edition("2026");

// Tier Core 辅助模块：由 imports_and_namespaces.tl 与 type_syntax.tl 导入。

pub class Point {
    pub x: int,
    pub y: int,
}

pub enum Axis {
    horizontal,
    vertical,
}

pub static origin_x = 0;
pub static samples = [10, 20, 30];

pub fn add(left: int, right: int) -> int {
    return left + right;
}

pub fn make_point(x: int, y: int) -> Point {
    return Point { x: x, y: y };
}

pub fn shift_x(point: Point, amount: int) -> Point {
    point.x += amount;
    return point;
}
