use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[path = "knowledge_impl.rs"]
mod knowledge_impl;

#[derive(Debug, PartialEq, Eq)]
pub enum KnowledgeCommand {
    Proposals,
    Show {
        id: String,
    },
    RetryReview {
        id: String,
    },
    Status,
    Verify,
    Search {
        query: String,
        scope: Option<String>,
    },
    Read {
        id: String,
        max_bytes: usize,
    },
    Lookup {
        symbol: String,
        limit: usize,
    },
    Export {
        format: String,
        scope: Option<String>,
        output: Option<PathBuf>,
        force: bool,
    },
    Add {
        file: PathBuf,
        layer: String,
        name: Option<String>,
    },
    Write {
        layer: String,
        title: String,
        text: Option<String>,
        file: Option<PathBuf>,
        name: Option<String>,
    },
    Refresh {
        layer: Option<String>,
    },
}

#[derive(Debug, Parser)]
#[command(
    name = "teal-knowledge",
    version,
    about = "Inspect and manage versioned Teal knowledge",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Proposals,
    Show {
        id: String,
    },
    #[command(name = "retry-review")]
    RetryReview {
        id: String,
    },
    Status,
    Verify,
    Search {
        #[arg(value_name = "QUERY", num_args = 1..)]
        query: Vec<String>,
        #[arg(long, value_enum, default_value = "all")]
        scope: Scope,
    },
    Read {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long, value_name = "BYTES", default_value_t = 8192)]
        max_bytes: usize,
    },
    Lookup {
        #[arg(value_name = "SYMBOL")]
        symbol: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    Export {
        #[arg(long, value_enum, default_value = "md")]
        format: ExportFormat,
        #[arg(long, value_enum, default_value = "official")]
        scope: Scope,
        #[arg(long, value_name = "DIRECTORY")]
        output: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
    Add {
        #[arg(value_name = "FILE")]
        file: PathBuf,
        #[arg(long, value_enum)]
        layer: Layer,
        #[arg(long)]
        name: Option<String>,
    },
    Write {
        #[arg(long, value_enum)]
        layer: Layer,
        #[arg(long)]
        title: String,
        #[arg(long, conflicts_with = "file")]
        text: Option<String>,
        #[arg(long, value_name = "FILE", conflicts_with = "text")]
        file: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
    },
    Refresh {
        #[arg(long, value_enum, default_value = "all")]
        layer: RefreshLayer,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Scope {
    Official,
    User,
    Project,
    All,
}
impl Scope {
    fn normalized(self) -> Option<String> {
        match self {
            Self::Official => Some("official".into()),
            Self::User => Some("user".into()),
            Self::Project => Some("project".into()),
            Self::All => None,
        }
    }
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Layer {
    User,
    Workspace,
}
impl Layer {
    const fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Workspace => "workspace",
        }
    }
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum RefreshLayer {
    User,
    Workspace,
    All,
}
impl RefreshLayer {
    const fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Workspace => "workspace",
            Self::All => "all",
        }
    }
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum ExportFormat {
    #[value(name = "md", alias = "markdown")]
    Markdown,
    Jsonl,
    Toml,
}
impl ExportFormat {
    const fn name(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Jsonl => "jsonl",
            Self::Toml => "toml",
        }
    }
}

fn command(cli: Cli) -> KnowledgeCommand {
    match cli.command {
        None | Some(Command::Status) => KnowledgeCommand::Status,
        Some(Command::Proposals) => KnowledgeCommand::Proposals,
        Some(Command::Show { id }) => KnowledgeCommand::Show { id },
        Some(Command::RetryReview { id }) => KnowledgeCommand::RetryReview { id },
        Some(Command::Verify) => KnowledgeCommand::Verify,
        Some(Command::Search { query, scope }) => KnowledgeCommand::Search {
            query: query.join(" "),
            scope: scope.normalized(),
        },
        Some(Command::Read { id, max_bytes }) => KnowledgeCommand::Read { id, max_bytes },
        Some(Command::Lookup { symbol, limit }) => KnowledgeCommand::Lookup { symbol, limit },
        Some(Command::Export {
            format,
            scope,
            output,
            force,
        }) => KnowledgeCommand::Export {
            format: format.name().into(),
            scope: scope.normalized(),
            output,
            force,
        },
        Some(Command::Add { file, layer, name }) => KnowledgeCommand::Add {
            file,
            layer: layer.name().into(),
            name,
        },
        Some(Command::Write {
            layer,
            title,
            text,
            file,
            name,
        }) => KnowledgeCommand::Write {
            layer: layer.name().into(),
            title,
            text,
            file,
            name,
        },
        Some(Command::Refresh { layer }) => KnowledgeCommand::Refresh {
            layer: Some(layer.name().into()),
        },
    }
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            std::process::exit(error.exit_code());
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
    if let Err(error) = knowledge_impl::execute(&command(cli)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
