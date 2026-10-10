//! 校验当前 `teal-workbench` workspace 与其拆仓依赖清单。

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// 执行逻辑工作区校验；不再读取拆仓前遗留的 sibling `workspace.toml`。
pub fn run(arguments: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.as_slice() != ["verify"] {
        return Err("Usage: cargo xtask workspace verify".into());
    }
    let root = workspace_root();
    let metadata = cargo_metadata(&root)?;
    let members = metadata["workspace_members"]
        .as_array()
        .ok_or("cargo metadata has no workspace members")?;
    if members.is_empty() {
        return Err("workbench workspace has no members".into());
    }
    let package_names = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata has no packages")?
        .iter()
        .filter_map(|package| package["name"].as_str())
        .collect::<BTreeSet<_>>();
    for required in [
        "teal",
        "teal-bench",
        "teal-workbench-core",
        "teal-workbench-services",
        "teal-xtask",
    ] {
        if !package_names.contains(required) {
            return Err(format!("workbench package `{required}` is missing").into());
        }
    }
    let catalog = root.join("runtime/packages/catalog-workbench.toml");
    let value: toml::Value = toml::from_str(&fs::read_to_string(&catalog)?)?;
    let packages = value["packages"]
        .as_array()
        .ok_or("catalog has no packages")?;
    if packages.is_empty() {
        return Err("workbench catalog has no packages".into());
    }
    for package in packages.iter().filter_map(toml::Value::as_str) {
        let path = root
            .join("runtime/packages")
            .join(package)
            .join("package.toml");
        if !path.is_file() {
            return Err(format!("catalog package `{package}` has no package.toml").into());
        }
    }
    println!(
        "workbench workspace verified: {} Cargo members, {} runtime packages",
        members.len(),
        packages.len()
    );
    Ok(())
}

fn cargo_metadata(root: &Path) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let output = Command::new("cargo")
        .args(["metadata", "--locked", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask must be located in the tools directory of the workspace")
        .to_owned()
}
