use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let target = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("target"));
    let target = if target.is_absolute() {
        target
    } else {
        manifest.join(target)
    };
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let directory = target.join(profile);
    let suffix = env::consts::EXE_SUFFIX;
    for name in [
        "teal-c",
        "teal-run",
        "teal-test",
        "teal-auth",
        "teal-model",
        "teal-perm",
        "teal-config",
        "teal-knowledge",
        "teal-bench",
        "teal-fmt",
        "teal-lsp",
        "teal-dap",
        "teal-audit",
    ] {
        println!(
            "cargo:rustc-env=CARGO_BIN_EXE_{name}={}",
            directory.join(format!("{name}{suffix}")).display()
        );
    }
}
