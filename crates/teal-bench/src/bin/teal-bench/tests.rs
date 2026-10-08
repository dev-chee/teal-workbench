//! Teal Bench 根二进制的命令行契约测试。

use clap::Parser;

use super::{Args, cli_output};

#[test]
fn startup_help_and_argument_errors_are_english() {
    let help = Args::try_parse_from(["teal-bench", "--help"])
        .err()
        .expect("Help should exit early");
    let text = cli_output(&help);
    assert!(text.contains("Usage:"));
    assert!(text.contains("Print help"));
    assert!(text.contains("--no-mouse"));
    assert!(text.contains("--model-protocol-capture"));
    assert!(!text.contains("[OPTIONS]"));
    assert!(text.contains("[default:"));

    let error = Args::try_parse_from(["teal-bench", "--unknown-option"])
        .err()
        .expect("Unknown parameters should be rejected");
    assert!(cli_output(&error).contains("Unknown argument"));
    assert_eq!(error.exit_code(), 2);
    assert_eq!(
        Args::try_parse_from(["teal-bench", "--version"])
            .err()
            .expect("Version should exit early")
            .exit_code(),
        0
    );
}

#[test]
fn model_protocol_capture_requires_an_explicit_directory() {
    let default = Args::try_parse_from(["teal-bench"]).unwrap();
    assert!(default.model_protocol_capture.is_none());
    let captured =
        Args::try_parse_from(["teal-bench", "--model-protocol-capture", "private-captures"])
            .unwrap();
    assert_eq!(
        captured.model_protocol_capture.unwrap(),
        std::path::PathBuf::from("private-captures")
    );
    assert!(Args::try_parse_from(["teal-bench", "--model-protocol-capture"]).is_err());
}

#[test]
fn mouse_is_enabled_by_default_and_can_be_disabled() {
    let default = Args::try_parse_from(["teal-bench"]).expect("default arguments");
    assert!(!default.no_mouse);

    let disabled =
        Args::try_parse_from(["teal-bench", "--no-mouse"]).expect("mouse-disabled arguments");
    assert!(disabled.no_mouse);
}

#[test]
fn offline_mode_parses_and_rejects_all_telemetry_export_switches() {
    let offline =
        Args::try_parse_from(["teal-bench", "--offline"]).expect("offline mode should parse");
    assert!(offline.offline);

    for telemetry in ["--otel-logs", "--otel-traces", "--otel-metrics"] {
        let error = match Args::try_parse_from(["teal-bench", "--offline", telemetry]) {
            Ok(_) => panic!("offline mode must reject telemetry export"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }
}
