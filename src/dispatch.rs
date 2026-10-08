//! 根命令到同版本 sibling binary 的无损派发。

use std::{ffi::OsString, path::PathBuf, process::Command};

/// 派发一次根命令并返回目标进程退出码。
#[must_use]
pub fn run<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    let Some(command) = args.next() else {
        print_help();
        return 0;
    };
    if command == "--help" || command == "-h" {
        print_help();
        return 0;
    }
    if command == "--version" || command == "-V" {
        println!("teal {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    let Some(binary) = target(&command) else {
        eprintln!("unknown Teal command `{}`", command.to_string_lossy());
        eprintln!("Run `teal --help` to list available commands.");
        return 2;
    };
    let executable = match sibling(binary) {
        Ok(executable) => executable,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    match Command::new(&executable).args(args).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("cannot start `{}`: {error}", executable.display());
            1
        }
    }
}

fn print_help() {
    println!("Teal tool dispatcher");
    print!("\nUsage: teal <COMMAND> [ARGS...]\n");
    print!("\nCommands:\n");
    println!("    compile    Compile and check Teal source (teal-c)");
    println!("    run        Execute Teal source (teal-run)");
    println!("    test       Run registered Teal tests (teal-test)");
    println!("    auth       Manage authentication profiles (teal-auth)");
    println!("    model      List and select model profiles (teal-model)");
    println!("    perm       Manage workspace permissions (teal-perm)");
    println!("    config     Inspect and edit configuration (teal-config)");
    println!("    knowledge  Manage versioned knowledge (teal-knowledge)");
    println!("    bench      Start the workbench (teal-bench)");
    println!("    fmt        Format Teal source (teal-fmt)");
    println!("    lsp        Start the language server (teal-lsp)");
    println!("    dap        Start the debug adapter (teal-dap)");
    println!("    audit      Query and verify the local audit ledger (teal-audit)");
    print!("\nOptions:\n");
    println!("  -h, --help     Print help");
    println!("  -V, --version  Print version");
}

fn target(command: &std::ffi::OsStr) -> Option<&'static str> {
    let value = command.to_str()?;
    match value {
        "compile" => Some("teal-c"),
        "run" => Some("teal-run"),
        "test" => Some("teal-test"),
        "auth" => Some("teal-auth"),
        "model" => Some("teal-model"),
        "perm" => Some("teal-perm"),
        "config" => Some("teal-config"),
        "knowledge" => Some("teal-knowledge"),
        "bench" => Some("teal-bench"),
        "fmt" => Some("teal-fmt"),
        "lsp" => Some("teal-lsp"),
        "dap" => Some("teal-dap"),
        "audit" => Some("teal-audit"),
        value if value.starts_with('-') || value.ends_with(".tl") => Some("teal-run"),
        _ => None,
    }
}

fn sibling(name: &str) -> Result<PathBuf, String> {
    let current = std::env::current_exe()
        .map_err(|error| format!("cannot resolve the current Teal executable: {error}"))?;
    let directory = current
        .parent()
        .ok_or_else(|| "the current Teal executable has no parent directory".to_owned())?;
    let file_name = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let mut candidates = Vec::new();
    if directory.file_name().is_some_and(|name| name == "deps")
        && let Some(parent) = directory.parent()
    {
        candidates.push(parent.join(&file_name));
    }
    candidates.push(directory.join(&file_name));
    if let Some(candidate) = candidates.into_iter().find(|path| path.is_file()) {
        Ok(candidate)
    } else {
        let candidate = directory.join(file_name);
        Err(format!(
            "required sibling binary `{}` is missing; reinstall the complete Teal distribution",
            candidate.display()
        ))
    }
}
