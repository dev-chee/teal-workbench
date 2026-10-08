//! 中文 rustdoc 的源码审计、示例验证与站点组装。

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

mod site;
mod source;

use source::{Audit, ProductCrate};

/// API 站点包含的产品库及其源码入口。
pub const PRODUCTS: &[ProductCrate] = &[
    ProductCrate {
        package: "teal-security",
        rustdoc_name: "teal_security",
        root: "crates/teal-security/src/lib.rs",
    },
    ProductCrate {
        package: "teal-security",
        rustdoc_name: "teal_security",
        root: "crates/teal-security/src/lib.rs",
    },
    ProductCrate {
        package: "teal-core",
        rustdoc_name: "teal_core",
        root: "crates/teal-core/src/lib.rs",
    },
    ProductCrate {
        package: "teal-packages",
        rustdoc_name: "teal_packages",
        root: "crates/teal-packages/src/lib.rs",
    },
    ProductCrate {
        package: "teal-knowledge",
        rustdoc_name: "teal_knowledge",
        root: "crates/teal-knowledge/src/lib.rs",
    },
    ProductCrate {
        package: "teal-runtime",
        rustdoc_name: "teal_runtime",
        root: "crates/teal-runtime/src/lib.rs",
    },
    ProductCrate {
        package: "teal-bench",
        rustdoc_name: "teal_bench",
        root: "crates/teal-bench/src/lib.rs",
    },
    ProductCrate {
        package: "teal-workbench-automation",
        rustdoc_name: "teal_workbench_automation",
        root: "crates/teal-workbench-automation/src/lib.rs",
    },
    ProductCrate {
        package: "teal-workbench-core",
        rustdoc_name: "teal_workbench_core",
        root: "crates/teal-workbench-core/src/lib.rs",
    },
    ProductCrate {
        package: "teal-lsp-client",
        rustdoc_name: "teal_lsp_client",
        root: "crates/teal-lsp-client/src/lib.rs",
    },
    ProductCrate {
        package: "teal-workbench-services",
        rustdoc_name: "teal_workbench_services",
        root: "crates/teal-workbench-services/src/lib.rs",
    },
    ProductCrate {
        package: "teal-text",
        rustdoc_name: "teal_text",
        root: "crates/teal-text/src/lib.rs",
    },
    ProductCrate {
        package: "teal-utils",
        rustdoc_name: "teal_utils",
        root: "crates/teal-utils/src/lib.rs",
    },
    ProductCrate {
        package: "teal-web",
        rustdoc_name: "teal_web",
        root: "crates/teal-web/src/lib.rs",
    },
    ProductCrate {
        package: "teal-browser",
        rustdoc_name: "teal_browser",
        root: "crates/teal-browser/src/lib.rs",
    },
    ProductCrate {
        package: "teal-computer",
        rustdoc_name: "teal_computer",
        root: "crates/teal-computer/src/lib.rs",
    },
    ProductCrate {
        package: "teal-agent",
        rustdoc_name: "teal_agent",
        root: "crates/teal-agent/src/lib.rs",
    },
    ProductCrate {
        package: "teal-model",
        rustdoc_name: "teal_model",
        root: "crates/teal-model/src/lib.rs",
    },
    ProductCrate {
        package: "teal-host",
        rustdoc_name: "teal_host",
        root: "crates/teal-host/src/lib.rs",
    },
    ProductCrate {
        package: "teal-workspace",
        rustdoc_name: "teal_workspace",
        root: "crates/teal-workspace/src/lib.rs",
    },
    ProductCrate {
        package: "teal-config",
        rustdoc_name: "teal_config",
        root: "crates/teal-config/src/lib.rs",
    },
    ProductCrate {
        package: "teal-abi",
        rustdoc_name: "teal_abi",
        root: "crates/teal-abi/src/lib.rs",
    },
    ProductCrate {
        package: "teal-compiler",
        rustdoc_name: "teal_compiler",
        root: "crates/teal-compiler/src/lib.rs",
    },
    ProductCrate {
        package: "teal-fmt",
        rustdoc_name: "teal_fmt",
        root: "crates/teal-fmt/src/lib.rs",
    },
    ProductCrate {
        package: "teal-vm",
        rustdoc_name: "teal_vm",
        root: "crates/teal-vm/src/lib.rs",
    },
    ProductCrate {
        package: "teal-lsp",
        rustdoc_name: "teal_lsp",
        root: "crates/teal-lsp/src/lib.rs",
    },
    ProductCrate {
        package: "teal-dap",
        rustdoc_name: "teal_dap",
        root: "crates/teal-dap/src/lib.rs",
    },
];

/// 执行文档维护命令；构建与验证前均检查中文说明。
pub fn run(command: &str, arguments: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let workspace = workspace_root();
    match command {
        "audit" => {
            reject_arguments(&arguments)?;
            validate_product_manifest(&workspace, PRODUCTS)?;
            let audit = source::collect(&workspace, PRODUCTS)?;
            print_audit(&audit);
            source::validate(&audit)
        }
        "files" => {
            reject_arguments(&arguments)?;
            validate_product_manifest(&workspace, PRODUCTS)?;
            let audit = source::collect(&workspace, PRODUCTS)?;
            for file in audit.files {
                println!(
                    "{}",
                    file.strip_prefix(&workspace).unwrap_or(&file).display()
                );
            }
            Ok(())
        }
        "build" => {
            let options = Options::parse(arguments)?;
            collect_validated_audit(&workspace)?;
            site::build(&workspace, &options.output(&workspace))
        }
        "verify" => {
            let options = Options::parse(arguments)?;
            collect_validated_audit(&workspace)?;
            site::build(&workspace, &options.output(&workspace))?;
            site::test_doctests(&workspace)
        }
        _ => Err(format!("Unknown rustdoc command: {command:?}").into()),
    }
}

/// 收集并验证公开文档，同时单列源码审计耗时。
fn collect_validated_audit(workspace: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    validate_product_manifest(workspace, PRODUCTS)?;
    let audit = source::collect(workspace, PRODUCTS)?;
    print_audit(&audit);
    source::validate(&audit)?;
    println!(
        "rustdoc stage source-audit: {:.3}s",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// 用 Cargo 的实际工作区库 target 核对文档清单，防止新增公开库静默漏检。
fn validate_product_manifest(
    workspace: &Path,
    products: &[ProductCrate],
) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("cargo")
        .args(["metadata", "--locked", "--no-deps", "--format-version", "1"])
        .current_dir(workspace)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed while checking rustdoc products: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let members = metadata["workspace_members"]
        .as_array()
        .ok_or("cargo metadata has no workspace members")?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<BTreeSet<_>>();
    let workspace = workspace.canonicalize()?;
    let mut actual = BTreeSet::new();
    for package in metadata["packages"]
        .as_array()
        .ok_or("cargo metadata has no packages")?
    {
        let Some(id) = package["id"].as_str() else {
            continue;
        };
        if !members.contains(id) {
            continue;
        }
        let package_name = package["name"].as_str().ok_or("package name is missing")?;
        for target in package["targets"]
            .as_array()
            .ok_or("package targets are missing")?
        {
            let is_library = target["kind"].as_array().is_some_and(|kinds| {
                kinds
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .any(|kind| {
                        matches!(kind, "lib" | "rlib" | "cdylib" | "staticlib" | "proc-macro")
                    })
            });
            if !is_library {
                continue;
            }
            let name = target["name"]
                .as_str()
                .ok_or("library target name is missing")?;
            let source = target["src_path"]
                .as_str()
                .ok_or("library target source is missing")?;
            let root = Path::new(source)
                .canonicalize()?
                .strip_prefix(&workspace)?
                .to_string_lossy()
                .replace('\\', "/");
            // tools 下的测试辅助库不是公开产品；产品库统一位于 crates。
            if !root.starts_with("crates/") {
                continue;
            }
            actual.insert((package_name.to_owned(), name.to_owned(), root));
        }
    }
    let documented = products
        .iter()
        .map(|product| {
            (
                product.package.to_owned(),
                product.rustdoc_name.to_owned(),
                product.root.to_owned(),
            )
        })
        .collect::<BTreeSet<_>>();
    if actual != documented {
        let missing = actual.difference(&documented).collect::<Vec<_>>();
        let stale = documented.difference(&actual).collect::<Vec<_>>();
        return Err(format!(
            "rustdoc product manifest does not match workspace library targets; missing: {missing:?}; stale: {stale:?}"
        )
        .into());
    }
    Ok(())
}

/// 根据工具所在目录确定工作区根目录，不依赖调用者的当前目录。
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask must be located in the tools directory of the workspace")
        .to_owned()
}

/// 拒绝无参数命令收到的额外输入。
fn reject_arguments(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.is_empty() {
        Ok(())
    } else {
        Err(format!("Unexpected arguments: {}", arguments.join(" ")).into())
    }
}

/// 输出审计覆盖范围；具体文档约定由源码审计器检查。
fn print_audit(audit: &Audit) {
    println!(
        "rustdoc audit: {} source code files, {} public units with documentation",
        audit.files.len(),
        audit.units.len()
    );
}

/// 文档构建选项；只允许指定产物目录。
#[derive(Default)]
struct Options {
    /// 未指定时使用工作区下的 `target/rustdoc-site`。
    output: Option<PathBuf>,
}

impl Options {
    /// 解析参数并拒绝缺失值或未知选项。
    fn parse(arguments: Vec<String>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut options = Self::default();
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--output" => {
                    options.output = Some(PathBuf::from(
                        arguments.next().ok_or("--output requires directory path")?,
                    ))
                }
                _ => return Err(format!("Unknown option: {argument:?}").into()),
            }
        }
        Ok(options)
    }

    /// 将相对输出路径解释为工作区相对路径；安全范围在构建前检查。
    fn output(&self, workspace: &Path) -> PathBuf {
        self.output
            .as_ref()
            .map(|path| {
                if path.is_absolute() {
                    path.clone()
                } else {
                    workspace.join(path)
                }
            })
            .unwrap_or_else(|| workspace.join("target/rustdoc-site"))
    }
}

#[cfg(test)]
mod tests;
