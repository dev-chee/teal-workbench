//! 独立 Teal 调试适配器命令入口。

use std::{net::SocketAddr, path::PathBuf};

use clap::{CommandFactory, FromArgMatches, Parser};
use teal_compiler::{LintLevel, RuntimeProfile};
use teal_dap::DebuggeeOptions;
mod leaf_observability;

#[derive(Debug, Parser)]
#[command(
    name = "teal-dap",
    version,
    about = "Debug Adapter Protocol server for Teal"
)]
struct Cli {
    #[arg(long, hide = true)]
    debuggee: bool,
    #[arg(long, hide = true)]
    connect: Option<SocketAddr>,
    #[arg(long, hide = true)]
    listen: Option<SocketAddr>,
    #[arg(long, hide = true)]
    token: Option<String>,
    #[arg(long, hide = true)]
    program: Option<PathBuf>,
    #[arg(long, hide = true)]
    entry: Option<String>,
    #[arg(long, hide = true)]
    runtime_profile: Option<String>,
    #[arg(long, hide = true)]
    permission_file: Option<PathBuf>,
    #[arg(long, hide = true)]
    compiler_debug: Option<bool>,
    #[arg(long, hide = true)]
    optimization: Option<String>,
    #[arg(long, hide = true)]
    debug_info: Option<bool>,
    #[arg(long, hide = true)]
    allow: Vec<String>,
    #[arg(long, hide = true)]
    warn: Vec<String>,
    #[arg(long, hide = true)]
    deny: Vec<String>,
    #[arg(long, hide = true)]
    forbid: Vec<String>,
    #[arg(long, hide = true)]
    stop_on_entry: bool,
    #[arg(last = true, hide = true)]
    program_args: Vec<String>,
}

fn main() {
    let argv =
        std::iter::once(std::ffi::OsString::from("teal-dap")).chain(std::env::args_os().skip(1));
    let matches = match Cli::command()
        .bin_name("teal-dap")
        .try_get_matches_from(argv)
    {
        Ok(matches) => matches,
        Err(error) => error.exit(),
    };
    let cli = Cli::from_arg_matches(&matches).expect("clap type and value must be consistent");
    let observation = leaf_observability::start();
    if let Err(error) = &observation {
        eprintln!("Teal tracing is unavailable: {error}");
    }
    let code = match execute(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    };
    if let Ok(observation) = observation {
        observation.shutdown();
    }
    std::process::exit(code);
}

fn execute(cli: Cli) -> Result<i32, Box<dyn std::error::Error>> {
    if !teal_security::process_context().is_available() {
        teal_security::install_process_sink(std::sync::Arc::new(
            teal_security::Ledger::open_default()?,
        ))?;
    }
    if cli.debuggee {
        let runtime_profile = RuntimeProfile::builtin(
            cli.runtime_profile.as_deref().unwrap_or("agent"),
        )
        .ok_or(format!(
            "--runtime-profile accepts {}",
            RuntimeProfile::BUILTIN_NAMES.join(", ")
        ))?;
        let permission_rules = cli
            .permission_file
            .as_deref()
            .map(read_permission_rules)
            .transpose()?
            .unwrap_or_default();
        if let Some(value) = cli.optimization.as_deref()
            && !matches!(value, "none" | "basic" | "aggressive")
        {
            return Err("--optimization accepts none, basic, or aggressive".into());
        }
        let mut lints = Vec::new();
        for (values, level) in [
            (cli.allow, LintLevel::Allow),
            (cli.warn, LintLevel::Warn),
            (cli.deny, LintLevel::Deny),
            (cli.forbid, LintLevel::Forbid),
        ] {
            for value in values {
                for name in value.split(',') {
                    lints.push((name.trim().to_owned(), level));
                }
            }
        }
        let options = DebuggeeOptions {
            connect: cli.connect,
            listen: cli.listen,
            token: cli.token.ok_or("debuggee mode requires --token")?,
            program: cli.program.ok_or("debuggee mode requires --program")?,
            runtime_profile,
            permission_rules,
            compiler_debug: cli.compiler_debug,
            lints,
            stop_on_entry: cli.stop_on_entry,
            arguments: cli.program_args,
        };
        return teal_dap::run_debuggee(options);
    }
    teal_dap::run_stdio()?;
    Ok(0)
}

fn read_permission_rules(
    path: &std::path::Path,
) -> Result<Vec<teal_security::PermissionRule>, Box<dyn std::error::Error>> {
    Ok(teal_config::read_permission_file(path)?.rules)
}
