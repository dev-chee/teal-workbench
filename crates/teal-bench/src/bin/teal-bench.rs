//! Teal 终端工作台的命令行入口。

use std::{ffi::OsString, path::PathBuf};

use clap::Parser;

#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
enum LogFormat {
    #[default]
    Text,
    Json,
}

#[derive(clap::Args, Debug, Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent explicit enablement of command line switches, not states of the same state machine"
)]
struct ObservationArgs {
    #[arg(
        long,
        value_enum,
        default_value = "text",
        help = "Local log encoding; application logs are never written to the terminal"
    )]
    log_format: LogFormat,
    #[arg(long, help = "Override the local rolling log directory")]
    log_dir: Option<PathBuf>,
    #[arg(long, help = "Local target and level filter; defaults to RUST_LOG")]
    log_filter: Option<String>,
    #[arg(long, help = "Explicitly enable OTLP log export")]
    otel_logs: bool,
    #[arg(long, help = "Explicitly enable OTLP trace export")]
    otel_traces: bool,
    #[arg(long, help = "Explicitly enable OTLP metrics export")]
    otel_metrics: bool,
    #[arg(
        long,
        help = "Shared OTLP/HTTP base URL; does not enable export by itself"
    )]
    otel_endpoint: Option<String>,
    #[arg(long, help = "Dedicated OTLP trace target and level filter")]
    otel_trace_filter: Option<String>,
    #[arg(long, help = "Dedicated OTLP log target and level filter")]
    otel_log_filter: Option<String>,
    #[arg(
        long,
        requires = "otel_traces",
        help = "Include Teal function spans; requires --otel-traces"
    )]
    trace_functions: bool,
}

impl From<ObservationArgs> for teal_workbench_services::observability::Arguments {
    fn from(value: ObservationArgs) -> Self {
        Self {
            log_format: match value.log_format {
                LogFormat::Text => Self::default().log_format,
                LogFormat::Json => teal_workbench_services::observability::LogFormat::Json,
            },
            log_dir: value.log_dir,
            log_filter: value.log_filter,
            otel_logs: value.otel_logs,
            otel_traces: value.otel_traces,
            otel_metrics: value.otel_metrics,
            otel_endpoint: value.otel_endpoint,
            otel_trace_filter: value.otel_trace_filter,
            otel_log_filter: value.otel_log_filter,
            trace_functions: value.trace_functions,
        }
    }
}

#[derive(Parser)]
#[command(
    version,
    about = "Windows-first modal terminal programming workspace",
    disable_help_flag = true,
    disable_version_flag = true,
    next_help_heading = "Options",
    help_template = "{about-with-newline}Usage: {usage}\n\n{all-args}"
)]
struct Args {
    #[command(flatten)]
    observation: ObservationArgs,
    #[arg(
        value_name = "PATH",
        help_heading = "Arguments",
        help = "File or directory to open; omit to restore a workspace session"
    )]
    path: Option<PathBuf>,
    #[arg(
        long,
        help = "Skip startup scripts, Agents, built-in grammar, and session recovery"
    )]
    safe_mode: bool,
    #[arg(long, help = "Disable user and workspace Teal configuration")]
    no_config: bool,
    #[arg(long, conflicts_with_all = ["otel_logs", "otel_traces", "otel_metrics"], help = "Disable remote models, network access, telemetry export, and extension side effects")]
    offline: bool,
    #[arg(long, help = "Explicit session JSON file")]
    session: Option<PathBuf>,
    #[arg(
        long,
        help = "Keep native terminal mouse selection and disable Teal Bench mouse handling"
    )]
    no_mouse: bool,
    #[arg(
        long,
        value_name = "DIR",
        help = "Capture model request bodies and raw SSE in a private local directory"
    )]
    model_protocol_capture: Option<PathBuf>,
    #[arg(short = 'h', long, help = "Print help", action = clap::ArgAction::Help)]
    help: Option<bool>,
    #[arg(short = 'V', long, help = "Print version information", action = clap::ArgAction::Version)]
    version: Option<bool>,
}

fn cli_output(error: &clap::Error) -> String {
    use clap::error::ErrorKind;
    match error.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            error.to_string().replace("[OPTIONS]", "[options]")
        }
        kind => {
            let reason = match kind {
                ErrorKind::UnknownArgument => "Unknown argument",
                ErrorKind::MissingRequiredArgument => "Missing required argument",
                ErrorKind::InvalidValue | ErrorKind::ValueValidation => "Invalid parameter value",
                ErrorKind::TooFewValues => "Argument is missing a value",
                ErrorKind::TooManyValues | ErrorKind::WrongNumberOfValues => {
                    "Incorrect number of argument values"
                }
                ErrorKind::ArgumentConflict => "Arguments conflict",
                ErrorKind::InvalidUtf8 => "Argument is not valid UTF-8 text",
                _ => "Unable to parse arguments",
            };
            format!("Command-line error: {reason}.\nRun --help to view usage.\n")
        }
    }
}

fn main() {
    std::process::exit(run_from_args(std::env::args_os().skip(1)));
}

fn run_from_args(args: impl IntoIterator<Item = OsString>) -> i32 {
    let args = args.into_iter().collect::<Vec<_>>();
    if args
        .first()
        .and_then(|argument| argument.to_str())
        .is_some_and(|argument| matches!(argument, "runs" | "inspect" | "resume" | "profile"))
    {
        eprintln!(
            "Command-line error: persistent run CLI commands were removed; use the :runs workbench panel or std.runs."
        );
        return 2;
    }
    let argv = std::iter::once(OsString::from("teal-bench")).chain(args);
    let args = match Args::try_parse_from(argv) {
        Ok(args) => args,
        Err(error) => {
            let output = cli_output(&error);
            if error.use_stderr() {
                eprint!("{output}");
            } else {
                print!("{output}");
            }
            return error.exit_code();
        }
    };

    if !teal_security::process_context().is_available() {
        let ledger = match teal_security::Ledger::open_default() {
            Ok(ledger) => ledger,
            Err(error) => {
                eprintln!("Audit is unavailable; workbench was not started: {error}");
                return 1;
            }
        };
        if let Err(error) = teal_security::install_process_sink(std::sync::Arc::new(ledger)) {
            eprintln!("Audit is unavailable; workbench was not started: {error}");
            return 1;
        }
    }

    let options = teal_bench::StartupOptions {
        path: args.path,
        safe_mode: args.safe_mode,
        no_config: args.no_config,
        offline: args.offline,
        session: args.session,
        capture_mouse: !args.no_mouse,
        observation: args.observation.into(),
        model_protocol_capture: args.model_protocol_capture,
    };
    match teal_bench::run(options) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

#[cfg(test)]
#[path = "teal-bench/tests.rs"]
mod tests;
