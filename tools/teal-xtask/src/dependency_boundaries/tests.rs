//! 执行体系生产依赖方向回归。

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
    process::Command,
};

use serde_json::Value;

struct ProductionGraph {
    names: BTreeMap<String, String>,
    labels: BTreeMap<String, String>,
    workspace_members: BTreeSet<String>,
    edges: BTreeMap<String, BTreeSet<String>>,
}

impl ProductionGraph {
    fn workspace_package(&self, name: &str) -> Option<&str> {
        self.workspace_members
            .iter()
            .find(|id| self.names.get(*id).is_some_and(|found| found == name))
            .map(String::as_str)
    }
}

fn production_graph(metadata: &Value) -> Result<ProductionGraph, String> {
    let packages = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata is missing packages")?;
    let identities = packages
        .iter()
        .map(|package| {
            let id = package["id"]
                .as_str()
                .ok_or("a package is missing its id")?
                .to_owned();
            let name = package["name"]
                .as_str()
                .ok_or("a package is missing its name")?
                .to_owned();
            let version = package["version"].as_str().unwrap_or("unknown");
            Ok((id, (name.clone(), format!("{name}@{version}"))))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let names = identities
        .iter()
        .map(|(id, (name, _))| (id.clone(), name.clone()))
        .collect::<BTreeMap<_, _>>();
    let labels = identities
        .into_iter()
        .map(|(id, (_, label))| (id, label))
        .collect::<BTreeMap<_, _>>();
    let workspace_members = metadata["workspace_members"]
        .as_array()
        .ok_or("cargo metadata is missing workspace_members")?
        .iter()
        .map(|member| {
            member
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "a workspace member is not a package id".to_owned())
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .ok_or("cargo metadata is missing resolve nodes")?;
    let mut edges_by_id = BTreeMap::<String, BTreeSet<String>>::new();
    for node in nodes {
        let package_id = node["id"]
            .as_str()
            .ok_or("a resolve node is missing its id")?;
        if !names.contains_key(package_id) {
            return Err("a resolve node refers to an unknown package".into());
        }
        let edges = edges_by_id.entry(package_id.to_owned()).or_default();
        for dependency in node["deps"]
            .as_array()
            .ok_or("a resolve node is missing deps")?
        {
            let is_production = dependency["dep_kinds"]
                .as_array()
                .ok_or("a resolved dependency is missing dep_kinds")?
                .iter()
                .any(|kind| kind["kind"].is_null() || kind["kind"] == "build");
            if is_production {
                let dependency_id = dependency["pkg"]
                    .as_str()
                    .ok_or("a resolved dependency is missing its package id")?;
                if !names.contains_key(dependency_id) {
                    return Err("a dependency refers to an unknown package".into());
                }
                edges.insert(dependency_id.to_owned());
            }
        }
    }
    Ok(ProductionGraph {
        names,
        labels,
        workspace_members,
        edges: edges_by_id,
    })
}

fn dependency_path(
    graph: &ProductionGraph,
    source_id: &str,
    forbidden: &BTreeSet<&str>,
) -> Option<Vec<String>> {
    let mut queue = VecDeque::from([vec![source_id.to_owned()]]);
    let mut visited = BTreeSet::from([source_id.to_owned()]);
    while let Some(path) = queue.pop_front() {
        let current = path.last()?;
        for dependency in graph.edges.get(current).into_iter().flatten() {
            let mut next = path.clone();
            next.push(dependency.clone());
            if graph
                .names
                .get(dependency)
                .is_some_and(|name| forbidden.contains(name.as_str()))
            {
                return Some(
                    next.iter()
                        .map(|id| graph.labels.get(id).cloned().unwrap_or_else(|| id.clone()))
                        .collect(),
                );
            }
            if visited.insert(dependency.clone()) {
                queue.push_back(next);
            }
        }
    }
    None
}

fn cargo_metadata(workspace: &Path) -> Value {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--all-features"])
        .current_dir(workspace)
        .output()
        .expect("cargo metadata should start");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("cargo metadata should return JSON")
}

fn cargo_tree_no_default_features(workspace: &Path, package: &str) -> String {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            package,
            "--no-default-features",
            "-e",
            "normal",
        ])
        .current_dir(workspace)
        .output()
        .expect("cargo tree should start");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("cargo tree should return UTF-8")
}

fn cargo_tree_embedded_core(workspace: &Path) -> String {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "teal-core",
            "--no-default-features",
            "--features",
            "embedded",
            "-e",
            "normal",
        ])
        .current_dir(workspace)
        .output()
        .expect("cargo tree should start");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("cargo tree should return UTF-8")
}

#[test]
fn execution_crates_keep_their_production_dependency_boundaries() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .ancestors()
        .nth(2)
        .expect("teal-xtask should be nested under the workspace root");
    let graph = production_graph(&cargo_metadata(workspace)).expect("metadata should be complete");
    let boundaries = [
        (
            "teal-vm",
            BTreeSet::from([
                "teal-compiler",
                "teal-packages",
                "teal-host",
                "teal-agent",
                "teal-runtime",
                "teal-model",
            ]),
        ),
        (
            "teal-core",
            BTreeSet::from([
                "teal-host",
                "teal-agent",
                "teal-runtime",
                "tokio",
                "reqwest",
                "rusqlite",
            ]),
        ),
        ("teal-host", BTreeSet::from(["teal-agent", "teal-runtime"])),
        (
            "teal-agent",
            BTreeSet::from(["teal-runtime", "teal-bench", "teal-workbench-core"]),
        ),
        (
            "teal-model",
            BTreeSet::from([
                "teal-agent",
                "teal-runtime",
                "teal-bench",
                "teal-workbench-services",
            ]),
        ),
    ];
    let violations = boundaries
        .iter()
        .filter_map(|(source, forbidden)| {
            let source_id = graph.workspace_package(source).unwrap_or_else(|| {
                panic!("constrained workspace package `{source}` is missing from cargo metadata")
            });
            dependency_path(&graph, source_id, forbidden)
        })
        .map(|path| path.join(" -> "))
        .collect::<Vec<_>>();

    assert!(
        violations.is_empty(),
        "execution crate dependency boundaries were violated:\n{}",
        violations.join("\n")
    );
}

#[test]
fn teal_core_embedded_runtime_excludes_compiler_and_packages() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .ancestors()
        .nth(2)
        .expect("teal-xtask should be nested under the workspace root");
    let tree = cargo_tree_embedded_core(workspace);
    for forbidden in ["teal-compiler", "teal-packages"] {
        assert!(
            !tree
                .lines()
                .any(|line| line.trim_start().starts_with(forbidden)),
            "Embedded Runtime teal-core unexpectedly includes {forbidden}:\n{tree}"
        );
    }
}

#[test]
fn workbench_core_does_not_pull_native_security_implementations() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .ancestors()
        .nth(2)
        .expect("teal-xtask should be nested under the workspace root");
    let tree = cargo_tree_no_default_features(workspace, "teal-workbench-core");
    for forbidden in ["teal-vm", "teal-host", "reqwest"] {
        assert!(
            !tree
                .lines()
                .any(|line| line.trim_start().starts_with(forbidden)),
            "Workbench core unexpectedly includes {forbidden}:\n{tree}"
        );
    }
}

#[test]
fn workbench_core_syntax_feature_stays_in_the_editor_layer() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .ancestors()
        .nth(2)
        .expect("teal-xtask should be nested under the workspace root");
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "teal-workbench-core",
            "--no-default-features",
            "--features",
            "syntax",
            "-e",
            "normal",
        ])
        .current_dir(workspace)
        .output()
        .expect("cargo tree should start");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tree = String::from_utf8(output.stdout).expect("cargo tree should return UTF-8");
    for forbidden in [
        "teal-vm",
        "teal-host",
        "teal-agent",
        "teal-workbench-services",
        "reqwest",
    ] {
        assert!(
            !tree
                .lines()
                .any(|line| line.trim_start().starts_with(forbidden)),
            "Workbench core syntax unexpectedly includes {forbidden}:\n{tree}"
        );
    }
}

#[test]
fn dependency_graph_includes_build_edges_and_excludes_dev_edges() {
    let metadata = serde_json::json!({
        "packages": [
            {"id": "a", "name": "a", "version": "1.0.0"},
            {"id": "build", "name": "build-helper", "version": "1.0.0"},
            {"id": "forbidden", "name": "forbidden", "version": "1.0.0"},
            {"id": "dev", "name": "dev-helper", "version": "1.0.0"}
        ],
        "workspace_members": ["a"],
        "resolve": {"nodes": [
            {"id": "a", "deps": [
                {"pkg": "build", "dep_kinds": [{"kind": "build"}]},
                {"pkg": "dev", "dep_kinds": [{"kind": "dev"}]}
            ]},
            {"id": "build", "deps": [
                {"pkg": "forbidden", "dep_kinds": [{"kind": null}]}
            ]},
            {"id": "forbidden", "deps": []},
            {"id": "dev", "deps": [
                {"pkg": "forbidden", "dep_kinds": [{"kind": null}]}
            ]}
        ]}
    });
    let graph = production_graph(&metadata).unwrap();

    assert_eq!(
        dependency_path(&graph, "a", &BTreeSet::from(["forbidden"])),
        Some(vec![
            "a@1.0.0".into(),
            "build-helper@1.0.0".into(),
            "forbidden@1.0.0".into()
        ])
    );
    assert!(!graph.edges["a"].contains("dev"));
}

#[test]
fn dependency_graph_keeps_same_name_versions_isolated() {
    let metadata = serde_json::json!({
        "packages": [
            {"id": "a", "name": "a", "version": "1.0.0"},
            {"id": "x", "name": "x", "version": "1.0.0"},
            {"id": "helper-1", "name": "helper", "version": "1.0.0"},
            {"id": "helper-2", "name": "helper", "version": "2.0.0"},
            {"id": "forbidden", "name": "forbidden", "version": "1.0.0"}
        ],
        "workspace_members": ["a", "x"],
        "resolve": {"nodes": [
            {"id": "a", "deps": [
                {"pkg": "helper-1", "dep_kinds": [{"kind": null}]}
            ]},
            {"id": "x", "deps": [
                {"pkg": "helper-2", "dep_kinds": [{"kind": null}]}
            ]},
            {"id": "helper-1", "deps": []},
            {"id": "helper-2", "deps": [
                {"pkg": "forbidden", "dep_kinds": [{"kind": null}]}
            ]},
            {"id": "forbidden", "deps": []}
        ]}
    });
    let graph = production_graph(&metadata).unwrap();

    assert_eq!(
        dependency_path(&graph, "a", &BTreeSet::from(["forbidden"])),
        None
    );
    assert_eq!(
        dependency_path(&graph, "x", &BTreeSet::from(["forbidden"])),
        Some(vec![
            "x@1.0.0".into(),
            "helper@2.0.0".into(),
            "forbidden@1.0.0".into()
        ])
    );
}
