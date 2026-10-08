@edition("2026");

// Tier Core：文件头构建标量指令；重复时后写覆盖前写，CLI 是最终一层。
@optimization("basic");
@compiler_debug(false);
@debug_info(true);

import "modules/compiler_directives.tl" as settings;

task main() {
    // 导入文件的同名指令只被校验并产生 ignored-compiler-option 警告。
    print(settings::value());
}
