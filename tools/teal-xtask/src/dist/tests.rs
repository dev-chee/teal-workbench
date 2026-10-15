//! 分发目录、资源清单、安全边界和平台归档的回归测试。

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    EntryKind, MANIFEST_FORMAT_VERSION, RuntimeManifest, assemble_distribution,
    checked_target_path, classify_entry, create_platform_archive, create_tar_archive,
    create_zip_archive, executable_name, run, sha256_file, validate_bundle_name,
};
/// 为每个测试创建并清理独立目录。
struct Fixture {
    /// 样本根目录。
    root: PathBuf,
}

impl Fixture {
    /// 创建包含运行时资源和工作台伪 release 二进制的样本。
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("teal-dist-{name}-{nonce}-{}", std::process::id()));
        let fixture = Self { root };
        fixture.write(
            "runtime/README.md",
            b"# Teal \xe8\xbf\x90\xe8\xa1\x8c\xe6\x97\xb6\xe8\xb5\x84\xe6\xba\x90\n",
        );
        fixture.write(
            "runtime/packages/std/ui.tl",
            b"pub enum Component { text }\n",
        );
        fixture.write("runtime/config/default.json", br#"{"theme":"default"}"#);
        fixture.write("runtime/assets/logo.bin", &[0, 1, 2, 255]);
        for binary in [
            "teal",
            "teal-run",
            "teal-perm",
            "teal-config",
            "teal-bench",
            "teal-knowledge",
        ] {
            let path = Path::new("binaries").join(executable_name(binary));
            fixture.write(path, binary.as_bytes());
        }
        fixture
    }

    /// 写入相对样本路径并创建父目录。
    fn write(&self, relative: impl AsRef<Path>, contents: &[u8]) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn assembly_replaces_stale_output_and_writes_sorted_hashes() {
    let fixture = Fixture::new("assembly");
    fixture.write("dist/teal-test/stale.txt", b"stale");
    let bundle = assemble_distribution(
        &fixture.root.join("runtime"),
        &fixture.root.join("binaries"),
        &fixture.root.join("dist"),
        "teal-test",
    )
    .unwrap();

    assert!(!bundle.join("stale.txt").exists());
    assert_eq!(
        fs::read(bundle.join(executable_name("teal"))).unwrap(),
        b"teal"
    );
    for binary in [
        "teal-run",
        "teal-perm",
        "teal-config",
        "teal-bench",
        "teal-knowledge",
    ] {
        assert_eq!(
            fs::read(bundle.join(executable_name(binary))).unwrap(),
            binary.as_bytes()
        );
    }

    let manifest: RuntimeManifest =
        serde_json::from_slice(&fs::read(bundle.join("runtime/manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest.format_version, MANIFEST_FORMAT_VERSION);
    assert_eq!(
        manifest
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        [
            "README.md",
            "assets/logo.bin",
            "config/default.json",
            "packages/std/ui.tl"
        ]
    );
    assert!(
        manifest
            .files
            .iter()
            .all(|file| !file.path.starts_with("knowledge")),
        "official knowledge must stay embedded in the binaries: {:?}",
        manifest.files
    );
    for file in &manifest.files {
        let path = bundle.join("runtime").join(&file.path);
        assert_eq!(file.size, fs::metadata(&path).unwrap().len());
        assert_eq!(file.sha256, sha256_file(&path).unwrap());
        assert_eq!(file.sha256.len(), 64);
        assert!(
            file.sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
    }
}

#[test]
fn both_archives_contain_one_complete_top_level_bundle() {
    let fixture = Fixture::new("archive");
    let bundle = assemble_distribution(
        &fixture.root.join("runtime"),
        &fixture.root.join("binaries"),
        &fixture.root.join("dist"),
        "teal-test",
    )
    .unwrap();
    let zip = create_zip_archive(&fixture.root.join("dist"), &bundle, "teal-test").unwrap();
    let tar = create_tar_archive(&fixture.root.join("dist"), &bundle, "teal-test").unwrap();
    let platform =
        create_platform_archive(&fixture.root.join("dist"), &bundle, "teal-test").unwrap();
    assert_eq!(
        platform.extension().and_then(std::ffi::OsStr::to_str),
        Some(if cfg!(windows) { "zip" } else { "gz" })
    );
    let zip_names = zip_archive_names(&zip);
    let tar_names = tar_archive_names(&tar);
    assert_eq!(zip_names, tar_names);
    let suffix = std::env::consts::EXE_SUFFIX;
    let mut expected = vec![
        "teal-test/".to_owned(),
        format!("teal-test/teal{suffix}"),
        "teal-test/runtime/manifest.json".to_owned(),
        "teal-test/runtime/packages/std/ui.tl".to_owned(),
        "teal-test/runtime/config/default.json".to_owned(),
        "teal-test/runtime/assets/logo.bin".to_owned(),
    ];
    expected.extend(
        [
            "teal-run",
            "teal-perm",
            "teal-config",
            "teal-bench",
            "teal-knowledge",
        ]
        .map(|binary| format!("teal-test/{binary}{suffix}")),
    );
    for expected in expected {
        assert!(
            zip_names.contains(&expected),
            "Archive missing {expected:?}: {zip_names:?}"
        );
    }
    assert!(
        zip_names
            .iter()
            .all(|name| name == "teal-test/" || name.starts_with("teal-test/"))
    );
    assert!(
        zip_names.iter().all(|name| !name.contains("teal-ide")),
        "the breaking rename must not ship an IDE binary: {zip_names:?}"
    );
    assert!(
        zip_names
            .iter()
            .all(|name| !name.contains("runtime/knowledge")),
        "knowledge text form must not return to the distribution: {zip_names:?}"
    );
}

#[test]
fn reserved_manifest_and_unsafe_entry_types_are_rejected() {
    let fixture = Fixture::new("boundaries");
    fixture.write("runtime/manifest.json", b"{}");
    let error = assemble_distribution(
        &fixture.root.join("runtime"),
        &fixture.root.join("binaries"),
        &fixture.root.join("dist"),
        "teal-test",
    )
    .unwrap_err();
    assert!(error.to_string().contains("manifest.json"));

    assert_eq!(
        classify_entry(false, true, false).unwrap(),
        EntryKind::Directory
    );
    assert_eq!(classify_entry(false, false, true).unwrap(), EntryKind::File);
    assert!(
        classify_entry(true, false, true)
            .unwrap_err()
            .contains("symbolic link")
    );
    assert!(
        classify_entry(false, false, false)
            .unwrap_err()
            .contains("ordinary files")
    );
}

#[test]
fn bundle_names_cannot_escape_the_distribution_root() {
    assert!(validate_bundle_name("teal-test").is_ok());
    for name in ["", "../escape", "nested/name", "/absolute"] {
        assert!(
            validate_bundle_name(name).is_err(),
            "Name should be rejected: {name:?}"
        );
    }
}

#[test]
fn dist_command_rejects_unknown_arguments() {
    let error = run(vec!["--output".to_owned(), "elsewhere".to_owned()]).unwrap_err();
    assert!(error.to_string().contains("Usage:"));
}

#[test]
fn distribution_outputs_must_remain_below_target() {
    let fixture = Fixture::new("output-scope");
    fs::create_dir_all(fixture.root.join("target")).unwrap();
    assert!(
        checked_target_path(&fixture.root, &fixture.root.join("target/dist"))
            .unwrap()
            .ends_with("target/dist")
    );
    assert!(checked_target_path(&fixture.root, &fixture.root.join("target")).is_err());
    assert!(checked_target_path(&fixture.root, &fixture.root.join("outside")).is_err());
    assert!(checked_target_path(&fixture.root, &fixture.root.join("target/../outside")).is_err());
}

/// 返回 zip 归档中的全部规范路径。
fn zip_archive_names(path: &Path) -> Vec<String> {
    let mut archive = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    let mut names = (0..archive.len())
        .map(|index| archive.by_index(index).unwrap().name().to_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// 返回 tar.gz 归档中的全部规范路径。
fn tar_archive_names(path: &Path) -> Vec<String> {
    let decoder = flate2::read::GzDecoder::new(fs::File::open(path).unwrap());
    let mut archive = tar::Archive::new(decoder);
    let mut names = archive
        .entries()
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let is_directory = entry.header().entry_type().is_dir();
            let mut path = entry.path().unwrap().to_string_lossy().replace('\\', "/");
            if is_directory && !path.ends_with('/') {
                path.push('/');
            }
            path
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn language_binaries_have_one_source_and_never_replace_the_product_entry() {
    let installer = include_str!("../../../../runtime/installs/install.ps1");
    for (_, binary) in super::PRODUCTS.iter().chain(super::LANGUAGE_PRODUCTS) {
        assert!(
            installer.contains(&format!("'{binary}.exe'")),
            "installer missing {binary}"
        );
    }
    assert!(installer.contains("'--language-root', $languageRoot"));
    for name in ["teal-fmt", "teal-lsp", "teal-dap"] {
        assert!(super::LANGUAGE_PRODUCTS.contains(&("teal-lang", name)));
        assert!(!super::PRODUCTS.iter().any(|(_, binary)| *binary == name));
    }
    assert!(super::PRODUCTS.contains(&("teal", "teal")));
    assert!(
        !super::LANGUAGE_PRODUCTS
            .iter()
            .any(|(_, binary)| *binary == "teal")
    );
}

#[test]
fn runtime_merge_takes_std_from_language_and_keeps_workbench_extensions() {
    let fixture = Fixture::new("runtime-merge");
    fixture.write(
        "language/runtime/packages/catalog.toml",
        b"format = 6\npackages = [\"std\"]\n",
    );
    fixture.write(
        "language/runtime/packages/std/package.toml",
        b"format = 7\n",
    );
    fixture.write(
        "runtime/packages/catalog-workbench.toml",
        b"format = 6\npackages = [\"bench\"]\n",
    );
    fixture.write("runtime/packages/bench/package.toml", b"format = 7\n");
    let merged = super::merge_runtime(
        &fixture.root.join("runtime"),
        &fixture.root.join("language"),
        &fixture.root.join("build"),
    )
    .unwrap();
    let catalog: toml::Value =
        toml::from_str(&fs::read_to_string(merged.join("packages/catalog.toml")).unwrap()).unwrap();
    assert_eq!(catalog["format"].as_integer(), Some(6));
    assert_eq!(
        catalog["packages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>(),
        ["bench", "std"]
    );
    assert!(merged.join("packages/std/package.toml").is_file());
    assert!(merged.join("packages/bench/package.toml").is_file());
}
