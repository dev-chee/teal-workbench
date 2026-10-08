//! Builtin 接口维护命令的回归测试。

use super::run;

#[test]
fn verify_accepts_the_generated_interface() {
    run("verify", Vec::new()).expect("the checked-in builtin interface should be current");
}

#[test]
fn commands_reject_unknown_subcommands_and_extra_arguments() {
    let unknown = run("unknown", Vec::new()).expect_err("unknown commands must fail");
    assert!(
        unknown
            .to_string()
            .contains("Usage: cargo xtask builtins <generate|verify>")
    );

    let extra = run("verify", vec!["unexpected".into()])
        .expect_err("builtin commands must not accept positional arguments");
    assert!(
        extra
            .to_string()
            .contains("builtin interface commands do not accept arguments")
    );
}
