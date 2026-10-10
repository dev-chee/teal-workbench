//! rustdoc 命令选项处理的回归测试。

use std::path::Path;

use super::{Options, PRODUCTS, validate_product_manifest, workspace_root};

#[test]
fn product_site_contains_current_product_libraries() {
    for package in [
        "teal-bench",
        "teal-browser",
        "teal-computer",
        "teal-dap",
        "teal-fmt",
        "teal-lsp",
        "teal-lsp-client",
        "teal-text",
        "teal-web",
        "teal-workbench-automation",
        "teal-workbench-core",
        "teal-workbench-services",
        "teal-workspace",
    ] {
        assert!(
            PRODUCTS.iter().any(|product| product.package == package),
            "{package}"
        );
    }
    assert!(
        PRODUCTS
            .iter()
            .all(|product| product.root.ends_with("src/lib.rs"))
    );
}

#[test]
fn product_manifest_matches_workspace_libraries_and_reports_omissions() {
    let workspace = workspace_root();
    validate_product_manifest(&workspace, PRODUCTS).expect("current library manifest");

    let omitted = PRODUCTS
        .iter()
        .copied()
        .filter(|product| product.package != "teal-text")
        .collect::<Vec<_>>();
    let error = validate_product_manifest(&workspace, &omitted).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("missing"), "{message}");
    assert!(message.contains("teal-text"), "{message}");
}

#[test]
fn platform_product_crates_use_current_names_and_exclude_retired_packages() {
    for (package, rustdoc_name, root) in [
        (
            "teal-browser",
            "teal_browser",
            "crates/teal-browser/src/lib.rs",
        ),
        (
            "teal-computer",
            "teal_computer",
            "crates/teal-computer/src/lib.rs",
        ),
    ] {
        let product = PRODUCTS
            .iter()
            .find(|product| product.package == package)
            .unwrap_or_else(|| panic!("missing product crate: {package}"));
        assert_eq!(product.rustdoc_name, rustdoc_name, "{package}");
        assert_eq!(product.root, root, "{package}");
    }

    for obsolete_package in [
        "teal-abi",
        "teal-agent",
        "teal-compiler",
        "teal-config",
        "teal-core",
        "teal-host",
        "teal-knowledge",
        "teal-model",
        "teal-packages",
        "teal-runtime",
        "teal-security",
        "teal-utils",
        "teal-vm",
    ] {
        assert!(
            PRODUCTS
                .iter()
                .all(|product| product.package != obsolete_package),
            "obsolete product crate should not remain: {obsolete_package}"
        );
    }
    assert!(
        PRODUCTS
            .iter()
            .all(|product| !product.package.ends_with("-wasm"))
    );
}

#[test]
fn relative_output_is_resolved_under_workspace() {
    let options = Options::parse(vec!["--output".into(), "target/custom-docs".into()])
        .expect("Options should be able to parse");
    assert_eq!(
        options.output(Path::new("C:/workspace")),
        Path::new("C:/workspace/target/custom-docs")
    );
}

#[test]
fn obsolete_sync_option_and_missing_output_are_rejected() {
    assert!(Options::parse(vec!["--base".into(), "HEAD".into()]).is_err());
    assert!(Options::parse(vec!["--output".into()]).is_err());
}
