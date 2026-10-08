//! 标准库 Builtin 接口制品的生成与一致性校验。

use std::{fs, path::PathBuf};

/// 执行 `cargo xtask builtins generate|verify`。
///
/// 权威声明在 `teal-compiler` 的 Rust 声明表里，`runtime/packages/std/builtins.tli` 是由它
/// 渲染出的工具制品；`generate` 写回规范文本，`verify` 逐字节比对。
pub(crate) fn run(command: &str, arguments: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    if !arguments.is_empty() {
        return Err("builtin interface commands do not accept arguments".into());
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = root.join("runtime/packages/std/builtins.tli");
    let expected = teal_packages::BuiltinInterface::from_catalog(
        teal_compiler::builtin_catalog(),
        &teal_compiler::builtin_documentation(),
    )?;
    match command {
        "generate" => {
            fs::write(&path, expected.source())?;
            Ok(())
        }
        "verify" => {
            let current = fs::read_to_string(&path)?;
            if current != expected.source() {
                return Err(
                    "builtin interface is stale; run `cargo xtask builtins generate`".into(),
                );
            }
            if !expected.is_canonical() {
                return Err("generated builtin interface is not canonical".into());
            }
            Ok(())
        }
        _ => Err("Usage: cargo xtask builtins <generate|verify>".into()),
    }
}

#[cfg(test)]
#[path = "builtins/tests.rs"]
mod tests;
