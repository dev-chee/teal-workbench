#[path = "config_output.rs"]
mod output;
#[path = "perm_impl.rs"]
mod perm_impl;
use std::path::PathBuf;
use clap::{ArgAction, Parser, Subcommand, ValueEnum};
#[derive(Debug, PartialEq, Eq)]
pub enum PermissionsCommand {
    Show { json: bool },
    Preset(teal_security::PermissionPreset),
    Grant(PathBuf),
    Explain { request: PathBuf, json: bool },
    Operations { json: bool },
    Revoke,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Format {
    Text,
    Json,
}
impl Format {
    fn json(self) -> bool {
        matches!(self, Self::Json)
    }
}
#[derive(Debug, Parser)]
#[command(
    name = "teal-perm",
    version,
    about = "Manage workspace permission scopes",
    disable_help_subcommand = true,
    disable_version_flag = true
)]
struct Cli {
    #[arg(short='v',long="version",action=ArgAction::Version)]
    version: Option<bool>,
    #[command(subcommand)]
    command: Cmd,
}
#[derive(Debug, Subcommand)]
enum Cmd {
    Show {
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
    Preset {
        preset: Preset,
    },
    Grant {
        file: PathBuf,
    },
    Explain {
        #[arg(long)]
        request: PathBuf,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
    Operations {
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
    Revoke,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Preset {
    Explore,
    Ask,
    Develop,
}
fn main() {
    let c = match Cli::try_parse() {
        Ok(v) => v,
        Err(e) => {
            let _ = e.print();
            std::process::exit(e.exit_code())
        }
    };
    let ledger = match teal_security::Ledger::open_default() {
        Ok(ledger) => std::sync::Arc::new(ledger),
        Err(error) => {
            eprintln!("Audit is unavailable; action was not started: {error}");
            std::process::exit(1);
        }
    };
    if !teal_security::process_context().is_available()
        && let Err(error) = teal_security::install_process_sink(ledger)
    {
        eprintln!("Audit is unavailable; action was not started: {error}");
        std::process::exit(1);
    }
    let c = match c.command {
        Cmd::Show { format } => PermissionsCommand::Show {
            json: format.json(),
        },
        Cmd::Preset { preset } => PermissionsCommand::Preset(match preset {
            Preset::Explore => teal_security::PermissionPreset::Explore,
            Preset::Ask => teal_security::PermissionPreset::Ask,
            Preset::Develop => teal_security::PermissionPreset::Develop,
        }),
        Cmd::Grant { file } => PermissionsCommand::Grant(file),
        Cmd::Explain { request, format } => PermissionsCommand::Explain {
            request,
            json: format.json(),
        },
        Cmd::Operations { format } => PermissionsCommand::Operations {
            json: format.json(),
        },
        Cmd::Revoke => PermissionsCommand::Revoke,
    };
    std::process::exit(perm_impl::execute_permissions_command(c))
}
