#[path = "config_impl.rs"]
mod config_impl;
#[path = "config_output.rs"]
mod output;
#[path = "perm_impl.rs"]
mod perm_impl;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use std::{
    io::{self, IsTerminal, Write},
    path::PathBuf,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigCommand {
    List {
        file: Option<teal_config::ConfigFile>,
        layer: Option<teal_config::ConfigScope>,
        json: bool,
    },
    Get {
        file: teal_config::ConfigFile,
        key: String,
        layer: Option<teal_config::ConfigScope>,
        json: bool,
    },
    Set {
        file: teal_config::ConfigFile,
        key: String,
        value: String,
        layer: teal_config::ConfigScope,
        json: bool,
    },
    Unset {
        file: teal_config::ConfigFile,
        key: String,
        layer: teal_config::ConfigScope,
    },
    Paths {
        json: bool,
    },
}
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
    name = "teal-config",
    version,
    about = "Inspect and edit layered Teal configuration",
    disable_help_subcommand = true,
    disable_version_flag = true
)]
struct ConfigCli {
    #[arg(short='v',long="version",action=ArgAction::Version)]
    version: Option<bool>,
    #[command(subcommand)]
    command: ConfigBuiltin,
}
#[derive(Debug, Subcommand)]
enum ConfigBuiltin {
    List {
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        layer: Option<String>,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
    Get {
        path: String,
        #[arg(long)]
        layer: Option<String>,
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
    Set {
        path: String,
        value: String,
        #[arg(long)]
        layer: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Unset {
        path: String,
        #[arg(long)]
        layer: Option<String>,
    },
    Paths {
        #[arg(long, value_enum, default_value = "text")]
        format: Format,
    },
}
fn layer(v: Option<String>) -> Result<Option<teal_config::ConfigScope>, String> {
    v.map(|v| {
        teal_config::ConfigScope::parse(&v).ok_or_else(|| {
            format!("unknown configuration layer `{v}`; use install, user, or workspace")
        })
    })
    .transpose()
}
fn file(v: Option<String>) -> Result<Option<teal_config::ConfigFile>, String> {
    v.map(|v| {
        teal_config::ConfigFile::parse(&v).ok_or_else(|| {
            format!("unknown configuration file `{v}`; use config, model, bench, runtime, compiler, lint, agent, or observability")
        })
    })
    .transpose()
}
fn split(path: String) -> Result<(teal_config::ConfigFile, String), String> {
    let Some((f, k)) = path.split_once('.') else {
        return Err(format!(
            "configuration path `{path}` must start with config, model, bench, or runtime"
        ));
    };
    let f = teal_config::ConfigFile::parse(f).ok_or_else(|| {
        "unknown configuration file; use config, model, bench, runtime, compiler, lint, agent, or observability".to_owned()
    })?;
    if k.is_empty() || k.split('.').any(str::is_empty) {
        return Err(format!("configuration path `{path}` is invalid"));
    }
    Ok((f, k.to_owned()))
}
fn parse_config(args: Vec<std::ffi::OsString>) -> Result<ConfigCommand, String> {
    let c = ConfigCli::try_parse_from(std::iter::once("teal-config".into()).chain(args))
        .map_err(|e| e.to_string())?;
    match c.command {
        ConfigBuiltin::List {
            file: f,
            layer: l,
            format,
        } => Ok(ConfigCommand::List {
            file: file(f)?,
            layer: layer(l)?,
            json: format.json(),
        }),
        ConfigBuiltin::Get {
            path,
            layer: l,
            format,
        } => {
            let (f, k) = split(path)?;
            Ok(ConfigCommand::Get {
                file: f,
                key: k,
                layer: layer(l)?,
                json: format.json(),
            })
        }
        ConfigBuiltin::Set {
            path,
            value,
            layer: l,
            json,
        } => {
            let (f, k) = split(path)?;
            Ok(ConfigCommand::Set {
                file: f,
                key: k,
                value,
                layer: layer(l)?.unwrap_or(teal_config::ConfigScope::User),
                json,
            })
        }
        ConfigBuiltin::Unset { path, layer: l } => {
            let (f, k) = split(path)?;
            Ok(ConfigCommand::Unset {
                file: f,
                key: k,
                layer: layer(l)?.unwrap_or(teal_config::ConfigScope::User),
            })
        }
        ConfigBuiltin::Paths { format } => Ok(ConfigCommand::Paths {
            json: format.json(),
        }),
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
struct PermCli {
    #[arg(short='v',long="version",action=ArgAction::Version)]
    version: Option<bool>,
    #[command(subcommand)]
    command: PermBuiltin,
}
#[derive(Debug, Subcommand)]
enum PermBuiltin {
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
fn parse_perm(args: Vec<std::ffi::OsString>) -> Result<PermissionsCommand, String> {
    let c = PermCli::try_parse_from(std::iter::once("teal-perm".into()).chain(args))
        .map_err(|e| e.to_string())?;
    Ok(match c.command {
        PermBuiltin::Show { format } => PermissionsCommand::Show {
            json: format.json(),
        },
        PermBuiltin::Preset { preset } => PermissionsCommand::Preset(match preset {
            Preset::Explore => teal_security::PermissionPreset::Explore,
            Preset::Ask => teal_security::PermissionPreset::Ask,
            Preset::Develop => teal_security::PermissionPreset::Develop,
        }),
        PermBuiltin::Grant { file } => PermissionsCommand::Grant(file),
        PermBuiltin::Explain { request, format } => PermissionsCommand::Explain {
            request,
            json: format.json(),
        },
        PermBuiltin::Operations { format } => PermissionsCommand::Operations {
            json: format.json(),
        },
        PermBuiltin::Revoke => PermissionsCommand::Revoke,
    })
}

pub fn require_workspace_trust() -> Result<(), String> {
    let workspace = std::env::current_dir().map_err(|e| e.to_string())?;
    let effective = teal_security::load(Some(&workspace)).map_err(|e| e.to_string())?;
    if teal_bench::config::trusted(&effective).map_err(|error| error.to_string())? {
        return Ok(());
    }
    if teal_bench::config::permissions(&effective)
        .map_err(|error| error.to_string())?
        .trust_level
        == teal_security::TrustLevel::Unknown
        && io::stdin().is_terminal()
        && io::stdout().is_terminal()
    {
        print!(
            "Trust Teal configuration in workspace {}? [y/N] ",
            workspace.display()
        );
        io::stdout().flush().map_err(|e| e.to_string())?;
        let mut s = String::new();
        io::stdin().read_line(&mut s).map_err(|e| e.to_string())?;
        if matches!(s.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            teal_security::set_workspace_trust(
                effective.paths(),
                teal_security::TrustLevel::Trusted,
            )
            .map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
    Err("Workspace configuration requires trust".into())
}

fn main() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    match parse_config(args) {
        Ok(c) => {
            install_audit_sink();
            if let Err(e) = config_impl::run(c) {
                eprintln!("{e}");
                std::process::exit(1)
            }
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2)
        }
    }
}

fn install_audit_sink() {
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
}
