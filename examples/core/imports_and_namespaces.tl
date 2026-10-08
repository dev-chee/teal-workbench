@edition("2026");

// Tier Core：相对导入、别名、限定名称、直接导出与选择性重导出。
// 被导入文件的私有 main 不会执行，也不能从其他源码调用。
import "modules/geometry.tl";
import "modules/geometry.tl" as geo;
import "modules/geometry_facade.tl" as facade;

class Scene {
    focus: geometry::Point,
}


task main() {
    let sum = geometry::add(2, 3);
    let point = geo::Point { x: 4, y: 5 };
    let scene = Scene { focus: point };
    let axis = geometry::Axis.horizontal;

    geometry::origin_x = 1;
    geometry::samples[0] = 11;

    let shifted = geo::shift_x(point, 6);
    let public_point: facade::PublicPoint = facade::PublicPoint { x: facade::sum(3, 4), y: 8 };
    let public_axis = facade::Axis.vertical;
    facade::shared_origin_x = 2;

    print(sum, scene.focus.x, shifted.x, public_point.x, facade::local_sum(4, 5));
    print(geometry::origin_x, geometry::samples[0], typeof(axis), int(public_axis));
}
