use super::*;

#[test]
fn lock_round_trip_and_cache_require_exact_hashes() {
    let directory = tempfile::tempdir().unwrap();
    let bytes = b"extension";
    let packages = vec![
        RuntimePackage {
            name: "bench".into(),
            version: "1.0.0".into(),
            digest: sha256(b"bench"),
        },
        RuntimePackage {
            name: "std".into(),
            version: "11.0.0".into(),
            digest: sha256(b"std"),
        },
    ];
    let artifact = RuntimeArtifact {
        name: "debug-adapter".into(),
        version: "1".into(),
        sha256: sha256(bytes),
    };
    let lock = RuntimeLock::new(
        "0.1.0",
        packages.clone().into_iter().rev().collect(),
        vec![artifact.clone()],
        "native",
    )
    .unwrap();
    let path = directory.path().join(".teal/runtime.lock");
    lock.save(&path).unwrap();
    let loaded = RuntimeLock::load(&path).unwrap();
    let cache = RuntimeCache::new(directory.path().join("cache"));
    assert_eq!(loaded.packages, packages);
    assert_eq!(loaded.core_catalog_digest.len(), 64);
    assert_eq!(loaded.extension_catalog_digest, None);
    assert_eq!(loaded.package_format, PACKAGE_FORMAT_VERSION);
    assert_eq!(
        loaded.status("0.1.0", &packages, &cache),
        RuntimeLockStatus::MissingArtifacts(vec!["debug-adapter".into()])
    );
    assert!(cache.install(&artifact, b"wrong").is_err());
    cache.install(&artifact, bytes).unwrap();
    assert_eq!(
        loaded.status("0.1.0", &packages, &cache),
        RuntimeLockStatus::Ready
    );

    let mut changed = packages.clone();
    changed[1].digest = sha256(b"changed-std");
    assert_eq!(
        loaded.status("0.1.0", &changed, &cache),
        RuntimeLockStatus::PackageMismatch(vec!["std".into()]),
        "a version or digest change must identify the affected package"
    );

    let mut changed_catalog = loaded.clone();
    changed_catalog.core_catalog_digest = sha256(b"different-catalog");
    assert_eq!(
        changed_catalog.status("0.1.0", &packages, &cache),
        RuntimeLockStatus::PackageMismatch(vec!["catalog".into()]),
        "catalog identity drift must invalidate an otherwise equal package list"
    );
}

#[test]
fn lock_can_bind_core_and_extension_catalog_identities() {
    let lock = RuntimeLock::new("0.1.0", Vec::new(), Vec::new(), "native")
        .unwrap()
        .with_catalog_identity("a".repeat(64), Some("b".repeat(64)), PACKAGE_FORMAT_VERSION);
    assert_eq!(lock.core_catalog_digest, "a".repeat(64));
    assert_eq!(lock.extension_catalog_digest, Some("b".repeat(64)));
    lock.validate().unwrap();
}

#[test]
fn incompatible_lock_is_rejected_without_rewrite() {
    let directory = tempfile::tempdir().unwrap();
    // 未来版本用当前版本号加一表示，避免测试在 LOCK_VERSION 提升后误当受支持版本。
    let future = format!(
        r#"{{"version":{},"teal_version":"x","packages":[],"artifacts":[],"runtime_profile":"native"}}"#,
        LOCK_VERSION + 1
    );
    for (name, contents) in [
        (
            "legacy.lock",
            r#"{"version":1,"teal_version":"x","runtime_manifest_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","artifacts":[],"requested_features":[]}"#,
        ),
        (
            "extension.lock",
            r#"{"version":2,"teal_version":"x","runtime_manifest_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","artifacts":[],"requested_extensions":[]}"#,
        ),
        (
            "profile-v3.lock",
            r#"{"version":3,"teal_version":"x","runtime_manifest_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","artifacts":[],"runtime_profile":"native"}"#,
        ),
        (
            "profile-v4.lock",
            r#"{"version":4,"teal_version":"x","runtime_manifest_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","artifacts":[],"runtime_profile":"native"}"#,
        ),
        (
            "packages-v5.lock",
            r#"{"version":5,"teal_version":"x","packages":[],"artifacts":[],"runtime_profile":"native"}"#,
        ),
        ("future.lock", future.as_str()),
    ] {
        let path = directory.path().join(name);
        std::fs::write(&path, contents).unwrap();
        assert!(
            RuntimeLock::load(&path)
                .unwrap_err()
                .to_string()
                .contains(&format!("this build uses {LOCK_VERSION}"))
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), contents);
    }
}
