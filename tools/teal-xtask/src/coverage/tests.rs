//! ABI/VM 覆盖命令参数与目标解析的回归测试。

use super::{
    ABI_MINIMUM_COVERAGE, HOST_CRITICAL_MINIMUM_LINES, HOST_CRITICAL_MODULES,
    HOST_MINIMUM_COVERAGE, TEST_SOURCE_REGEX, VM_CRITICAL_MINIMUM_LINES, VM_CRITICAL_MODULES,
    VM_MINIMUM_COVERAGE, abi_arguments, enforce_summary, host_criteria, is_test_source,
    preflight_modules, run, vm_arguments, vm_criteria, CriticalSummary, TargetSummary,
};

#[test]
fn abi_coverage_excludes_tests_and_enforces_both_thresholds() {
    let arguments = abi_arguments();
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["--fail-under-lines", ABI_MINIMUM_COVERAGE])
    );
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["--fail-under-regions", ABI_MINIMUM_COVERAGE])
    );
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["--ignore-filename-regex", TEST_SOURCE_REGEX])
    );
    assert!(arguments.contains(&"--locked"));
}

#[test]
fn vm_coverage_uses_its_own_line_and_region_threshold() {
    let arguments = vm_arguments();
    assert!(arguments.contains(&"--all-targets"));
    assert!(arguments.contains(&"--json"));
    assert!(super::VM_COVERAGE_PACKAGES.contains(&"teal-agent"));
    assert!(super::VM_COVERAGE_PACKAGES.contains(&"teal-dap"));
    assert!(
        super::VM_COVERAGE_PACKAGES
            .iter()
            .all(|package| !package.ends_with("-wasm"))
    );
    assert_eq!(VM_MINIMUM_COVERAGE, "60");
    assert_eq!(VM_CRITICAL_MINIMUM_LINES, "80");
    assert!(VM_CRITICAL_MODULES.contains(&("external dispatch", "crates/teal-vm/src/external.rs")));
    assert!(
        VM_CRITICAL_MODULES.contains(&("debug session", "crates/teal-vm/src/debug/session.rs"))
    );
    for module in [
        ("snapshot facade", "crates/teal-vm/src/snapshot.rs"),
        ("snapshot capture", "crates/teal-vm/src/snapshot/capture.rs"),
        ("snapshot codec", "crates/teal-vm/src/snapshot/codec.rs"),
        ("snapshot context", "crates/teal-vm/src/snapshot/context.rs"),
        ("snapshot restore", "crates/teal-vm/src/snapshot/restore.rs"),
        (
            "snapshot validation",
            "crates/teal-vm/src/snapshot/validation.rs",
        ),
        ("debug evaluation", "crates/teal-vm/src/debug/evaluate.rs"),
    ] {
        assert!(VM_CRITICAL_MODULES.contains(&module));
    }
    assert_eq!(VM_CRITICAL_MODULES.len(), 12);
}

#[test]
fn host_coverage_has_global_and_security_critical_thresholds() {
    assert_eq!(HOST_MINIMUM_COVERAGE, "80");
    assert_eq!(HOST_CRITICAL_MINIMUM_LINES, "90");
    assert!(
        HOST_CRITICAL_MODULES.contains(&("dylib policy", "crates/teal-host/src/ffi/policy.rs"))
    );
    assert!(HOST_CRITICAL_MODULES.contains(&(
        "native filesystem",
        "crates/teal-host/src/native/file_system.rs"
    )));
    assert!(HOST_CRITICAL_MODULES.contains(&(
        "process spec",
        "crates/teal-host/src/standard/process/spec.rs"
    )));
    assert_eq!(HOST_CRITICAL_MODULES.len(), 7);
}

#[test]
fn coverage_rejects_missing_or_unknown_targets_before_spawning_cargo() {
    assert!(run(Vec::new()).is_err());
    assert!(run(vec!["unknown".into()]).is_err());
    assert!(run(vec!["abi".into(), "extra".into()]).is_err());
}

#[test]
fn test_source_predicate_only_matches_tests_entries() {
    use std::path::Path;

    // 普通文件名即使以 `test` 开头也不算测试源码。
    assert!(!is_test_source(Path::new("crates/teal-abi/src/latest.rs")));
    assert!(!is_test_source(Path::new(
        "crates/teal-vm/src/execution.rs"
    )));
    // `tests` 目录与 `tests.rs` 仍然被排除。
    assert!(is_test_source(Path::new(
        "crates/teal-abi/src/program/tests/support.rs"
    )));
    assert!(is_test_source(Path::new(
        "crates/teal-abi/src/numeric/tests.rs"
    )));
}

#[test]
fn every_declared_critical_module_exists_in_the_workspace() {
    // 前置校验必须在真实工作区上通过；缺一即说明清单与实际布局脱节。
    preflight_modules(&[vm_criteria()]).expect("VM critical modules must exist");
    preflight_modules(&[host_criteria()]).expect("Host critical modules must exist");
    preflight_modules(&[vm_criteria(), host_criteria()]).expect("`all` criteria must exist");
}

fn summary(
    package: &'static str,
    minimum: &'static str,
    critical_minimum: &'static str,
    covered: u64,
    count: u64,
) -> TargetSummary {
    let lines = super::CoverageCount { count, covered };
    TargetSummary {
        package,
        minimum,
        critical_minimum,
        lines,
        regions: super::CoverageCount { count, covered },
        critical: vec![CriticalSummary {
            name: "module",
            lines: super::CoverageCount { count, covered },
            regions: super::CoverageCount { count, covered },
        }],
    }
}

#[test]
fn thresholds_are_enforced_on_global_and_critical_lines() {
    // 达到阈值：通过。
    let passing = summary("teal-vm", "60", "80", 80, 100);
    assert!(enforce_summary(&passing).is_ok());

    // 全局行/区域低于阈值：报出全局原因。
    let failing_global = summary("teal-host", "80", "90", 70, 100);
    let reason = enforce_summary(&failing_global).expect_err("global coverage must fail");
    assert!(
        reason.contains("teal-host") && reason.contains("80%"),
        "{reason}"
    );

    // 全局达标但关键模块不足：报出模块原因。
    let failing_critical = TargetSummary {
        package: "teal-host",
        minimum: "80",
        critical_minimum: "90",
        lines: super::CoverageCount {
            count: 100,
            covered: 95,
        },
        regions: super::CoverageCount {
            count: 100,
            covered: 95,
        },
        critical: vec![CriticalSummary {
            name: "authorization",
            lines: super::CoverageCount {
                count: 100,
                covered: 50,
            },
            regions: super::CoverageCount {
                count: 100,
                covered: 50,
            },
        }],
    };
    let reason = enforce_summary(&failing_critical).expect_err("critical coverage must fail");
    assert!(
        reason.contains("authorization") && reason.contains("90%"),
        "{reason}"
    );

    // 空计数按 0% 处理。
    let empty = summary("teal-vm", "60", "80", 0, 0);
    assert!(enforce_summary(&empty).is_err());
}
