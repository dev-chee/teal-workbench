@edition("2026");

// Tier Core：基础、集合、联合、函数及限定名称类型。
// `none` 是可显式声明和存储的单值类型，与内部未初始化槽位不同。
import "modules/geometry.tl" as geo;
task main() {
    let integer: int = 1;
    let decimal: float = 1.5;
    let flag: bool = true;
    let text: string = "typed";
    let missing: none = none;
    let list: int[] = [1, 2];
    let matrix: float[][] = [[1.0], [2.0]];
    let lookup: {string: int} = {"one": 1};
    let choice: int | string;
    if flag {
        choice = integer;
    } else {
        choice = text;
    }
    let procedure: fn();
    let predicate: fn(int, string) -> bool;
    let imported: geo::Point;
    imported = geo::Point { x: 3, y: 4 };

    print(typeof(integer), typeof(decimal), typeof(flag), typeof(text));
    print(typeof(missing), typeof(list), typeof(matrix), typeof(lookup));
    print(typeof(choice), typeof(procedure), typeof(predicate), imported.x);
}
