@edition("2026");

// Tier Core 辅助模块：选择性重导出并重命名另一模块的公开声明。

import "geometry.tl" as geometry;

pub geometry::Point as PublicPoint;
pub geometry::Axis;
pub geometry::origin_x as shared_origin_x;
pub geometry::add as sum;

pub fn local_sum(left: int, right: int) -> int {
    return sum(left, right);
}
