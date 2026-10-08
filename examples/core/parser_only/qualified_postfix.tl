@edition("2026");

// Tier Core 解析边界：限定名称后的字段、索引与调用链目前只保证可解析。

fn main() {
    let pi = math::constants::pi;
    let point = math::geometry::make(1, 2,);
    let constructed = math::geometry::Point {
        x: 3,
        y: 4,
    };

    module::value = 5;
    module::items[0] = 6;
    point.translate(1, 2);
    constructed.geometry::translate(3);
}
