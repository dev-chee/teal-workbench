//! 中文 Rust 文档解析与审计回归测试。

use std::{fs, path::PathBuf};

use super::{ProductCrate, collect, contains_chinese_prose, validate};

/// 创建独立源码样本，测试结束时自动移除临时文件。
struct Fixture(PathBuf);

impl Fixture {
    /// 使用测试名称与进程编号隔离样本。
    fn new(name: &str, source: &str) -> Self {
        let root = std::env::temp_dir().join(format!("teal-rustdoc-{name}-{}", std::process::id()));
        fs::create_dir_all(root.join("src"))
            .expect("It should be possible to create a sample directory");
        fs::write(root.join("src/lib.rs"), source)
            .expect("Should be able to write sample source code");
        Self(root)
    }

    /// 从样本入口收集公开文档。
    fn audit(&self) -> super::Audit {
        collect(
            &self.0,
            &[ProductCrate {
                package: "fixture",
                rustdoc_name: "fixture",
                root: "src/lib.rs",
            }],
        )
        .expect("The sample should be able to parse")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("It should be possible to remove the sample directory");
    }
}

#[test]
fn direct_chinese_docs_are_collected_without_language_configuration() {
    let fixture = Fixture::new(
        "direct",
        "//! \u{4e2d}\u{6587}\u{5e93}\u{8bf4}\u{660e}\u{3002}\n/// \u{4e2d}\u{6587}\u{51fd}\u{6570}\u{8bf4}\u{660e}\u{3002}\npub fn api() {}\n",
    );
    let audit = fixture.audit();
    validate(&audit).expect("Direct Chinese documents should pass audit");
    assert_eq!(audit.units.len(), 2);
    assert!(audit.units.iter().all(|unit| !unit.has_conditional_text));
}

#[test]
fn included_markdown_is_read_from_the_canonical_path() {
    let fixture = Fixture::new("include", "#![doc = include_str!(\"../README.md\")]\n");
    fs::write(
        fixture.0.join("README.md"),
        "# \u{5e93}\u{8bf4}\u{660e}\r\n\r\n\u{4e2d}\u{6587}\u{6b63}\u{6587}\u{3002}\r\n",
    )
    .unwrap();
    let audit = fixture.audit();
    validate(&audit).expect("Standard path Chinese documents should pass audit");
    assert!(!audit.units[0].markdown.contains('\r'));
}

#[test]
fn english_only_docs_are_rejected() {
    let fixture = Fixture::new(
        "english",
        "/// An English-only function.\npub fn api() {}\n",
    );
    let error = validate(&fixture.audit())
        .expect_err("Must fail when Chinese is missing")
        .to_string();
    assert!(error.contains("lacks a Chinese description"));
}

#[test]
fn conditional_documentation_is_rejected_even_when_chinese() {
    let fixture = Fixture::new(
        "conditional",
        "#[cfg_attr(doc, doc = \"\u{4e2d}\u{6587}\u{51fd}\u{6570}\u{3002}\")]\npub fn api() {}\n",
    );
    let error = validate(&fixture.audit())
        .expect_err("Conditional statement must fail")
        .to_string();
    assert!(error.contains("conditional document"));
}

#[test]
fn code_examples_do_not_substitute_for_chinese_prose() {
    assert!(!contains_chinese_prose(
        "Example.\n```rust,no_run\nlet text = \"\u{4e2d}\u{6587}\";\n```"
    ));
    assert!(!contains_chinese_prose(
        "~~~rust\nlet text = \"\u{4e2d}\u{6587}\";\n~~~"
    ));
    assert!(contains_chinese_prose(
        "\u{793a}\u{4f8b}\u{8bf4}\u{660e}\u{3002}\n```rust\nlet value = 1;\n```"
    ));
}

#[test]
fn hidden_test_modules_are_not_audited() {
    let fixture = Fixture::new(
        "test-module",
        "//! \u{4e2d}\u{6587}\u{5e93}\u{8bf4}\u{660e}\u{3002}\n#[cfg(test)]\nmod tests;\n",
    );
    validate(&fixture.audit()).expect("No attempt should be made to open the test module");
}
