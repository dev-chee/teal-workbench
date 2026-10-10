//! Teal 仓库维护入口；提供分发、ABI 覆盖率和 API 文档命令。

mod dist;
mod logical_workspaces;
mod rustdoc;
mod test_gate;

#[cfg(test)]
#[path = "dependency_boundaries/tests.rs"]
mod dependency_boundary_tests;

/// 返回维护命令用法，命令和参数名称保持稳定。
fn usage() -> &'static str {
    "Usage:\n  cargo xtask dist [--language-root PATH]\n  cargo xtask rustdoc audit\n  cargo xtask rustdoc files\n  cargo xtask rustdoc build [--output PATH]\n  cargo xtask rustdoc verify [--output PATH]\n  cargo xtask workspace verify\n  cargo xtask test-gate run --spec PATH [--force]"
}

/// 执行维护操作；错误统一写入标准错误并返回非零状态。
fn main() {
    if let Err(error) = run() {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}

/// 将命令行分派到文档工具，不接受未知命令。
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some("dist") => dist::run(arguments.collect()),
        Some("rustdoc") => {
            let command = arguments.next().ok_or_else(usage)?;
            rustdoc::run(&command, arguments.collect())
        }
        Some("workspace") => logical_workspaces::run(arguments.collect()),
        Some("test-gate") => test_gate::run(arguments.collect()),
        _ => Err(usage().into()),
    }
}
