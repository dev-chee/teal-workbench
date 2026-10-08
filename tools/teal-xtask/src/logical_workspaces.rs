//! 校验 `teal-lang` 与 `teal-workbench` 的逻辑拆仓清单。

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

type DynError = Box<dyn std::error::Error>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LogicalWorkspace {
    schema: u32,
    kind: String,
    name: String,
    repository_root: PathBuf,
    #[serde(default)]
    depends_on: Vec<String>,
    members: Vec<PathBuf>,
    #[serde(default)]
    package_roots: Vec<PathBuf>,
    catalog_manifest: PathBuf,
    validation_profiles: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidationProfile {
    schema: u32,
    name: String,
    workspace: String,
    purpose: String,
    inputs: Vec<PathBuf>,
    gates: Vec<ValidationGate>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidationGate {
    name: String,
    #[serde(default)]
    working_directory: PathBuf,
    command: Vec<String>,
}

/// 执行逻辑工作区清单校验。
pub fn run(arguments: Vec<String>) -> Result<(), DynError> {
    if arguments.as_slice() != ["verify"] {
        return Err("Usage: cargo xtask workspace verify".into());
    }
    let root = workspace_root();
    let lang = load_workspace(&root, "teal-lang")?;
    let workbench = load_workspace(&root, "teal-workbench")?;
    validate_workspace(&root, &lang)?;
    validate_workspace(&root, &workbench)?;
    validate_partition(&root, &lang, &workbench)?;
    let profiles = load_profiles(&root, &lang, &workbench)?;
    validate_profiles(&root, &profiles, &lang, &workbench)?;
    println!(
        "logical workspaces verified: {} language members, {} workbench members, {} profiles",
        lang.members.len(),
        workbench.members.len(),
        profiles.len()
    );
    Ok(())
}

fn load_workspace(root: &Path, name: &str) -> Result<LogicalWorkspace, DynError> {
    let path = workspace_directory(root, name).join("workspace.toml");
    let contents = fs::read_to_string(&path)?;
    let workspace: LogicalWorkspace = toml::from_str(&contents)?;
    if workspace.name != name {
        return Err(format!(
            "logical workspace manifest {} declares `{}`",
            path.display(),
            workspace.name
        )
        .into());
    }
    Ok(workspace)
}

fn validate_workspace(root: &Path, workspace: &LogicalWorkspace) -> Result<(), DynError> {
    if workspace.schema != 1 || workspace.kind != "logical-workspace" {
        return Err(format!(
            "logical workspace `{}` has an unsupported schema",
            workspace.name
        )
        .into());
    }
    let manifest_root = workspace_directory(root, &workspace.name).join(&workspace.repository_root);
    if manifest_root.canonicalize()? != root.canonicalize()? {
        return Err(format!(
            "logical workspace `{}` must resolve repository_root to the repository root",
            workspace.name
        )
        .into());
    }
    if workspace.members.is_empty() {
        return Err(format!("logical workspace `{}` has no members", workspace.name).into());
    }
    let mut members = BTreeSet::new();
    for member in &workspace.members {
        if !members.insert(member) {
            return Err(format!(
                "logical workspace `{}` repeats member `{}`",
                workspace.name,
                member.display()
            )
            .into());
        }
        let path = member_manifest_path(root, member);
        if !path.is_file() {
            return Err(format!(
                "logical workspace `{}` member `{}` has no Cargo.toml",
                workspace.name,
                member.display()
            )
            .into());
        }
        let manifest: toml::Value = toml::from_str(&fs::read_to_string(&path)?)?;
        let package_name = manifest
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
            .ok_or_else(|| format!("member `{}` has no package.name", member.display()))?;
        if package_name == "teal" && member != Path::new(".") {
            return Err("the root package `teal` must be listed as `.`".into());
        }
    }
    for package_root in &workspace.package_roots {
        let path = root.join(package_root);
        if !path.join("package.toml").is_file() {
            return Err(format!(
                "logical workspace `{}` package root `{}` has no package.toml",
                workspace.name,
                package_root.display()
            )
            .into());
        }
    }
    let catalog_manifest = root.join(&workspace.catalog_manifest);
    if !catalog_manifest.is_file() {
        return Err(format!(
            "logical workspace `{}` catalog manifest `{}` does not exist",
            workspace.name,
            workspace.catalog_manifest.display()
        )
        .into());
    }
    Ok(())
}

fn validate_partition(
    root: &Path,
    lang: &LogicalWorkspace,
    workbench: &LogicalWorkspace,
) -> Result<(), DynError> {
    if workbench.depends_on != ["teal-lang"] {
        return Err("teal-workbench must depend on teal-lang exactly once".into());
    }
    if !lang.depends_on.is_empty() {
        return Err("teal-lang must not depend on teal-workbench".into());
    }
    let overlap = lang
        .members
        .iter()
        .filter(|member| workbench.members.contains(member))
        .map(|member| member.display().to_string())
        .collect::<Vec<_>>();
    if !overlap.is_empty() {
        return Err(format!("logical workspace members overlap: {}", overlap.join(", ")).into());
    }

    for member in &lang.members {
        reject_workbench_dependencies(&member_manifest_path(root, member))?;
    }
    Ok(())
}

fn reject_workbench_dependencies(path: &Path) -> Result<(), DynError> {
    let manifest: toml::Value = toml::from_str(&fs::read_to_string(path)?)?;
    for section in ["dependencies", "build-dependencies"] {
        let Some(dependencies) = manifest.get(section).and_then(toml::Value::as_table) else {
            continue;
        };
        if let Some(name) = dependencies
            .keys()
            .find(|name| name.starts_with("teal-workbench"))
        {
            return Err(format!(
                "teal-lang member `{}` depends on workbench crate `{name}`",
                path.display()
            )
            .into());
        }
    }
    Ok(())
}

fn load_profiles(
    root: &Path,
    lang: &LogicalWorkspace,
    workbench: &LogicalWorkspace,
) -> Result<Vec<(PathBuf, ValidationProfile)>, DynError> {
    let names = lang
        .validation_profiles
        .iter()
        .chain(&workbench.validation_profiles)
        .cloned()
        .collect::<Vec<_>>();
    let mut profiles = Vec::with_capacity(names.len());
    for name in names {
        let path = root
            .join("validation/profiles")
            .join(format!("{name}.toml"));
        let profile: ValidationProfile = toml::from_str(&fs::read_to_string(&path)?)?;
        profiles.push((path, profile));
    }
    Ok(profiles)
}

fn validate_profiles(
    root: &Path,
    profiles: &[(PathBuf, ValidationProfile)],
    lang: &LogicalWorkspace,
    workbench: &LogicalWorkspace,
) -> Result<(), DynError> {
    let workspaces = [lang, workbench];
    let mut names = BTreeSet::new();
    for (path, profile) in profiles {
        if profile.schema != 1
            || profile.name.is_empty()
            || profile.purpose.trim().is_empty()
            || profile.inputs.is_empty()
            || profile.gates.is_empty()
        {
            return Err(format!("validation profile `{}` is incomplete", path.display()).into());
        }
        let expected_name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("invalid profile path `{}`", path.display()))?;
        if profile.name != expected_name || !names.insert(profile.name.as_str()) {
            return Err(format!(
                "validation profile `{}` has a duplicate or wrong name",
                path.display()
            )
            .into());
        }
        if !workspaces
            .iter()
            .any(|workspace| workspace.name == profile.workspace)
        {
            return Err(format!(
                "validation profile `{}` references unknown workspace `{}`",
                profile.name, profile.workspace
            )
            .into());
        }
        for input in &profile.inputs {
            let path = root.join(input);
            if !path.is_file() {
                return Err(format!(
                    "validation profile `{}` declares missing input `{}`",
                    profile.name,
                    input.display()
                )
                .into());
            }
        }
        for gate in &profile.gates {
            if gate.name.is_empty() || gate.command.is_empty() || gate.command[0].is_empty() {
                return Err(
                    format!("validation profile `{}` has an invalid gate", profile.name).into(),
                );
            }
            let cwd = root.join(&gate.working_directory);
            if !cwd.is_dir() {
                return Err(format!(
                    "validation profile `{}` gate `{}` has an invalid working directory",
                    profile.name, gate.name
                )
                .into());
            }
        }
    }
    Ok(())
}

fn member_manifest_path(root: &Path, member: &Path) -> PathBuf {
    if member == Path::new(".") {
        root.join("Cargo.toml")
    } else {
        root.join(member).join("Cargo.toml")
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask must be located in the tools directory of the workspace")
        .to_owned()
}

fn workspace_directory(root: &Path, name: &str) -> PathBuf {
    let sibling = root
        .parent()
        .map(|parent| parent.join(name))
        .unwrap_or_else(|| root.join(name));
    if sibling.join("workspace.toml").is_file() {
        sibling
    } else {
        root.join(name)
    }
}
