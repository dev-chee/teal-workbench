@edition("2026");

// Cross-tier Showcase：最小工作台 Host 调用；它不定义新的语言 Tier。
host "std.bench.editor" as editor { fn message(text: string); }
fn main() { editor::message("Hello from Teal Bench"); }
