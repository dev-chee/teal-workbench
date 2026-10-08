//! 中文单站点布局、页面语言标记和输出范围的回归测试。

use std::{fs, io, path::PathBuf};

use super::{
    PRODUCTS, checked_target_path, decorate_page, fresh_build_root, is_permission_denied,
    landing_page, recreate_directory,
};

/// 创建具有独立名称的临时工作区。
fn fixture(name: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("teal-rustdoc-site-{name}-{}", std::process::id()));
    fs::create_dir_all(root.join("target"))
        .expect("It should be possible to create a sample directory");
    root
}

#[test]
fn landing_page_links_all_libraries_without_language_routing() {
    let page = landing_page();
    assert!(page.contains("lang=\"zh-CN\""));
    assert!(page.contains("Teal API Documentation"));
    for product in PRODUCTS {
        assert!(page.contains(&format!("href=\"{}/index.html\"", product.rustdoc_name)));
    }
    assert!(!page.contains("<script"));
    assert!(!page.contains("href=\"en/"));
    assert!(!page.contains("href=\"zh-CN/"));
}

#[test]
fn chinese_metadata_preserves_page_content_without_counterpart_links() {
    let root = fixture("metadata");
    let page = root.join("page.html");
    fs::write(
        &page,
        "<!doctype html><html lang=\"en\"><head></head><body><code>Value</code>Chinese description.</body></html>",
    )
    .unwrap();
    decorate_page(&page).expect("Should be able to mark page language");
    decorate_page(&page).expect("Repeat processing should be consistent");
    let html = fs::read_to_string(&page).unwrap();
    assert!(html.contains("<html lang=\"zh-CN\">"));
    assert!(html.contains("<code>Value</code>Chinese description."));
    assert!(!html.contains("hreflang"));
    assert!(!html.contains("teal-doc-language"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_page_is_rejected_without_overwriting_it() {
    let root = fixture("malformed");
    let page = root.join("page.html");
    fs::write(&page, "<html></html>").unwrap();
    assert!(decorate_page(&page).is_err());
    assert_eq!(fs::read_to_string(&page).unwrap(), "<html></html>");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn output_must_be_a_strict_target_descendant() {
    let root = fixture("boundaries");
    for path in [
        &root,
        &root.join("target"),
        &root.join("target/../outside"),
        &root.join("outside"),
    ] {
        assert!(
            checked_target_path(&root, path).is_err(),
            "{} should not be accepted",
            path.display()
        );
    }
    assert!(checked_target_path(&root, &root.join("target/nested/site")).is_ok());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn recreating_site_removes_previous_language_trees_only() {
    let root = fixture("rebuild");
    let site = root.join("target/site");
    fs::create_dir_all(site.join("en")).unwrap();
    fs::create_dir_all(site.join("zh-CN")).unwrap();
    fs::write(root.join("target/keep"), "Keep compilation cache").unwrap();
    recreate_directory(&root, &site).unwrap();
    assert!(site.is_dir());
    assert_eq!(fs::read_dir(&site).unwrap().count(), 0);
    assert!(root.join("target/keep").is_file());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn build_rejects_output_overlapping_its_cache_before_running_cargo() {
    let root = fixture("overlap");
    for output in [
        root.join("target/rustdoc-build"),
        root.join("target/rustdoc-build/nested"),
    ] {
        let error =
            super::build(&root, &output).expect_err("Cannot overwrite internal build cache");
        assert!(error.to_string().contains("overlap"));
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn isolated_build_root_is_a_fresh_target_descendant() {
    let root = fixture("isolated-build");
    let build_root = fresh_build_root(&root).expect("Should allocate an isolated build root");
    assert!(checked_target_path(&root, &build_root).is_ok());
    assert_eq!(
        build_root.parent().and_then(|path| path.file_name()),
        Some(std::ffi::OsStr::new("target"))
    );
    assert!(!build_root.to_string_lossy().starts_with("\\\\?\\"));
    assert!(!build_root.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn only_permission_denied_cleanup_errors_trigger_fallback() {
    let denied = io::Error::new(io::ErrorKind::PermissionDenied, "locked documentation file");
    let other = io::Error::new(io::ErrorKind::NotFound, "missing documentation file");
    assert!(is_permission_denied(&denied));
    assert!(!is_permission_denied(&other));
}
