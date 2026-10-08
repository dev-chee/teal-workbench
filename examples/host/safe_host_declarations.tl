@edition("2026");

// Tier Host：安全 Host 声明只描述模块、名称和精确签名，不包含实现或授权。
// 同目录 host_registry.rs 演示默认拒绝及显式 capability 授权后的执行。
host "rein.editor" as editor {
    fn read(arg0: int) -> string;
    fn apply(arg0: string, arg1: bool);
}


fn main() {
    let source = editor::read(0);
    editor::apply(source, true);
}
