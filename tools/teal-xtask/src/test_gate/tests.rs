//! 串行 test-gate 规范、缓存与失效语义的端到端回归测试。

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::run_spec;

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

/// 创建互不共享输入、缓存和日志的临时工作区。
fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "teal-test-gate-{name}-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("work")).expect("The gate fixture workspace should be creatable");
    fs::write(root.join("work/input.txt"), "input-v1").unwrap();
    root
}

/// 以 TOML 基本字符串格式编码路径，兼容 Windows 反斜杠。
fn toml_string(value: impl AsRef<str>) -> String {
    format!("{:?}", value.as_ref())
}

/// 写入使用当前测试二进制作为跨平台 gate 子进程的最小规范。
fn write_spec(root: &Path, task_id: &str, gates: &[(&str, &str)]) -> PathBuf {
    let executable = std::env::current_exe().expect("The test executable path should be known");
    let mut spec = format!(
        "task_id = {}\ninputs = [\"work/input.txt\"]\n",
        toml_string(task_id)
    );
    for (name, child_test) in gates {
        spec.push_str(&format!(
            "\n[[gates]]\nname = {}\ncwd = \"work\"\ncommand = [{}, {}, \"--exact\", \"--nocapture\"]\n",
            toml_string(name),
            toml_string(executable.to_string_lossy()),
            toml_string(format!("test_gate::tests::{child_test}")),
        ));
    }
    let directory = root.join("target/test-gate").join(task_id);
    fs::create_dir_all(&directory).expect("The declared test-gate output directory should exist");
    let path = directory.join("gate.toml");
    fs::write(&path, spec).expect("The gate spec should be writable");
    path
}

fn marker(root: &Path) -> PathBuf {
    root.join("work/child-marker.txt")
}

fn marker_count(root: &Path) -> usize {
    fs::read_to_string(marker(root))
        .unwrap_or_default()
        .lines()
        .count()
}

fn run_summary(root: &Path, task_id: &str) -> String {
    fs::read_to_string(
        root.join("target/test-gate")
            .join(task_id)
            .join("run-summary.md"),
    )
    .expect("Every completed gate attempt should have a machine-generated summary")
}

fn enable_child(root: &Path) {
    fs::write(root.join("work/.teal-test-gate-child"), "enabled").unwrap();
}

/// 这是由规范 argv 调起的成功子测试；正常测试进程不会创建 sentinel，故不会产生副作用。
#[test]
fn test_gate_command_child() {
    let working_directory = std::env::current_dir().unwrap();
    if !working_directory.join(".teal-test-gate-child").is_file() {
        return;
    }

    let mut output = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(working_directory.join("child-marker.txt"))
        .unwrap();
    writeln!(output, "invoked").unwrap();

    if working_directory
        .join(".teal-test-gate-mutate-input")
        .is_file()
    {
        fs::write(
            working_directory.join("input.txt"),
            "input-mutated-during-gate",
        )
        .unwrap();
    }
    let expected_argument = working_directory.join(".teal-test-gate-expected-argument");
    if expected_argument.is_file() {
        let expected = fs::read_to_string(expected_argument).unwrap();
        assert!(
            std::env::args().any(|argument| argument == expected),
            "The gate argv must preserve an opaque argument without shell interpretation"
        );
        fs::write(working_directory.join("opaque-argument-ok.txt"), "ok").unwrap();
    }
    assert!(
        !working_directory.join(".teal-test-gate-fail").is_file(),
        "The controlled first gate should fail"
    );
}

/// 这是 fail-fast 断言中的第二个 gate；若被执行就会留下可见证据。
#[test]
fn test_gate_command_second_child() {
    let working_directory = std::env::current_dir().unwrap();
    if working_directory.join(".teal-test-gate-child").is_file() {
        fs::write(working_directory.join("second-gate-ran.txt"), "unexpected").unwrap();
    }
}

#[test]
fn passing_gate_is_reused_by_identical_input_and_force_executes_it_again() {
    let root = fixture("cache-reuse");
    enable_child(&root);
    let spec = write_spec(
        &root,
        "cache-reuse",
        &[("marker", "test_gate_command_child")],
    );

    run_spec(&root, &spec, false).expect("The initial gate should pass");
    assert_eq!(marker_count(&root), 1);
    assert!(run_summary(&root, "cache-reuse").contains("Cache status: COLD"));
    run_spec(&root, &spec, false).expect("An unchanged passing gate should be reusable");
    assert_eq!(
        marker_count(&root),
        1,
        "A matching PASS cache key must not execute the gate process again"
    );
    assert!(run_summary(&root, "cache-reuse").contains("Cache status: HOT"));
    run_spec(&root, &spec, true).expect("--force should still permit a passing gate");
    assert_eq!(
        marker_count(&root),
        2,
        "--force must bypass an otherwise reusable PASS result"
    );
    assert!(run_summary(&root, "cache-reuse").contains("Cache status: COLD"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn changed_input_or_spec_cannot_reuse_a_previous_passing_gate() {
    let root = fixture("cache-key");
    enable_child(&root);
    let spec = write_spec(&root, "cache-key", &[("marker", "test_gate_command_child")]);

    run_spec(&root, &spec, false).unwrap();
    fs::write(root.join("work/input.txt"), "input-v2").unwrap();
    run_spec(&root, &spec, false).expect("Changed input must invalidate the PASS cache");
    assert_eq!(marker_count(&root), 2);

    let changed_spec = write_spec(
        &root,
        "cache-key",
        &[("renamed-marker", "test_gate_command_child")],
    );
    run_spec(&root, &changed_spec, false)
        .expect("Changed specification content must invalidate the PASS cache");
    assert_eq!(marker_count(&root), 3);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failure_stops_later_gates_without_claiming_a_passing_cache_entry() {
    let root = fixture("fail-fast");
    enable_child(&root);
    fs::write(root.join("work/.teal-test-gate-fail"), "fail").unwrap();
    let spec = write_spec(
        &root,
        "fail-fast",
        &[
            ("failing", "test_gate_command_child"),
            ("must-not-run", "test_gate_command_second_child"),
        ],
    );

    assert!(
        run_spec(&root, &spec, false).is_err(),
        "A process failure must make the final gate result fail"
    );
    assert_eq!(
        marker_count(&root),
        1,
        "The first gate should have started once"
    );
    assert!(
        !root.join("work/second-gate-ran.txt").exists(),
        "A failing gate must prevent later gates from running"
    );
    let output = root.join("target/test-gate/fail-fast");
    assert_eq!(
        fs::read_to_string(output.join("02-must-not-run.log")).unwrap(),
        "BLOCKED: an earlier required gate failed; this command was not started.\n"
    );
    let summary: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("run.json")).unwrap()).unwrap();
    let gates = summary["gates"].as_array().unwrap();
    assert_eq!(gates[0]["status"], "FAIL");
    assert_eq!(gates[1]["status"], "BLOCKED");
    assert_eq!(gates[1]["log"], "02-must-not-run.log");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn input_mutation_stops_later_gates_and_cannot_seed_a_passing_cache() {
    let root = fixture("stale");
    enable_child(&root);
    fs::write(root.join("work/.teal-test-gate-mutate-input"), "mutate").unwrap();
    let spec = write_spec(
        &root,
        "stale",
        &[
            ("mutator", "test_gate_command_child"),
            ("must-not-run", "test_gate_command_second_child"),
        ],
    );

    assert!(
        run_spec(&root, &spec, false).is_err(),
        "A stale input must make the final gate result stale rather than pass"
    );
    let report = fs::read_to_string(root.join("target/test-gate/stale/run-summary.md"))
        .expect("Even a stale run must leave an auditable report");
    assert!(
        report.contains("STALE"),
        "Changing a declared input during a gate cannot be accepted as PASS: {report}"
    );
    assert!(
        !root.join("work/second-gate-ran.txt").exists(),
        "Input drift after one gate must stop every later gate"
    );

    fs::write(root.join("work/input.txt"), "input-v1").unwrap();
    fs::remove_file(root.join("work/.teal-test-gate-mutate-input")).unwrap();
    run_spec(&root, &spec, false).expect("Restoring the original input should allow a fresh run");
    assert_eq!(
        marker_count(&root),
        2,
        "A PASS from the stale A-to-B run must not be reused after restoring input A"
    );
    assert!(
        root.join("work/second-gate-ran.txt").exists(),
        "The stable retry should execute the later gate"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unsafe_task_ids_and_specs_outside_the_declared_output_directory_are_rejected() {
    let root = fixture("unsafe-spec");
    enable_child(&root);
    let unsafe_spec = write_spec(
        &root,
        "../unsafe-task",
        &[("marker", "test_gate_command_child")],
    );
    let unsafe_error = match run_spec(&root, &unsafe_spec, false) {
        Ok(_) => panic!("Unsafe task ids must not produce a gate summary"),
        Err(error) => error,
    };
    assert!(
        unsafe_error
            .to_string()
            .contains("Unsafe test-gate task id")
    );
    assert_eq!(marker_count(&root), 0);

    let declared_spec = write_spec(
        &root,
        "outside-spec",
        &[("marker", "test_gate_command_child")],
    );
    let outside = root.join("outside/gate.toml");
    fs::create_dir_all(outside.parent().unwrap()).unwrap();
    fs::copy(&declared_spec, &outside).unwrap();
    let outside_error = match run_spec(&root, &outside, false) {
        Ok(_) => panic!("A spec outside its declared output directory must be rejected"),
        Err(error) => error,
    };
    assert!(outside_error.to_string().contains("Spec must be located"));
    assert_eq!(marker_count(&root), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parent_directory_gate_cwd_is_rejected_before_command_execution() {
    let root = fixture("unsafe-cwd");
    enable_child(&root);
    let spec = write_spec(
        &root,
        "unsafe-cwd",
        &[("marker", "test_gate_command_child")],
    );
    let contents = fs::read_to_string(&spec)
        .unwrap()
        .replace("cwd = \"work\"", "cwd = \"..\"");
    fs::write(&spec, contents).unwrap();

    let error = match run_spec(&root, &spec, false) {
        Ok(_) => panic!("Parent-directory gate cwd values must be rejected"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("gate cwd must be workspace-relative")
    );
    assert_eq!(marker_count(&root), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn argv_values_with_spaces_and_shell_metacharacters_stay_single_opaque_arguments() {
    let root = fixture("opaque-argv");
    enable_child(&root);
    let opaque = "spaces ; & $not_a_shell (argument) [literal]";
    fs::write(root.join("work/.teal-test-gate-expected-argument"), opaque).unwrap();
    let spec = write_spec(
        &root,
        "opaque-argv",
        &[("marker", "test_gate_command_child")],
    );
    let contents = fs::read_to_string(&spec).unwrap().replace(
        "\"--nocapture\"]",
        &format!("\"--nocapture\", \"--skip\", {}]", toml_string(opaque)),
    );
    fs::write(&spec, contents).unwrap();

    run_spec(&root, &spec, false).expect("Opaque argv values must not require a shell");
    assert!(root.join("work/opaque-argument-ok.txt").is_file());
    fs::remove_dir_all(root).unwrap();
}
