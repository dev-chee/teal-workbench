//! 独立 Teal 源码格式化命令入口。

use std::{
    ffi::OsStr,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use clap::{ArgAction, CommandFactory, FromArgMatches, Parser};
mod leaf_observability;

#[derive(Debug, Parser)]
#[command(
    name = "teal-fmt",
    version,
    about = "Format Teal source code",
    disable_help_flag = true,
    disable_version_flag = true
)]
struct Cli {
    #[arg(long)]
    check: bool,
    #[arg(value_name = "FILE")]
    files: Vec<PathBuf>,
    #[arg(short = 'h', long = "help", help = "Print help", action = ArgAction::Help)]
    help: Option<bool>,
    #[arg(
        short = 'V',
        long = "version",
        help = "Print version",
        action = ArgAction::Version
    )]
    version: Option<bool>,
}

fn main() {
    let argv =
        std::iter::once(std::ffi::OsString::from("teal-fmt")).chain(std::env::args_os().skip(1));
    let matches = match Cli::command()
        .bin_name("teal-fmt")
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
    let code = run(cli);
    if let Ok(observation) = observation {
        observation.shutdown();
    }
    std::process::exit(code);
}

fn run(cli: Cli) -> i32 {
    if cli.files.is_empty() || (cli.files.len() == 1 && is_standard_input(&cli.files[0])) {
        return format_standard_input(cli.check);
    }
    if cli.files.iter().any(|file| is_standard_input(file)) {
        eprintln!("`-` cannot be used together with a file path");
        return 1;
    }

    let mut failed = false;
    for file in cli.files {
        let code = match std::fs::read_to_string(&file) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("Unable to read {}: {error}", file.display());
                failed = true;
                continue;
            }
        };
        let formatted = match teal_fmt::format_named(file.to_string_lossy().as_ref(), &code) {
            Ok(formatted) => formatted,
            Err(error) => {
                eprintln!("{:?}", miette::Report::new(error));
                failed = true;
                continue;
            }
        };
        if formatted == code {
            continue;
        }
        if cli.check {
            eprintln!("Requires formatting: {}", file.display());
            failed = true;
        } else {
            let ledger = match teal_security::Ledger::open_default() {
                Ok(ledger) => ledger,
                Err(error) => {
                    eprintln!("Audit is unavailable; file was not changed: {error}");
                    return 1;
                }
            };
            let context = teal_security::AuditContext::new(std::sync::Arc::new(ledger));
            let mut event = match teal_security::Event::new(
                teal_security::Action::FileMutation,
                teal_security::Phase::Intent,
                teal_security::Outcome::Pending,
                teal_security::new_correlation_id(),
            ) {
                Ok(event) => event.with_actor(teal_security::Actor::User),
                Err(error) => {
                    eprintln!("Audit is unavailable; file was not changed: {error}");
                    return 1;
                }
            };
            if let Ok(workspace) = std::env::current_dir().and_then(|path| path.canonicalize()) {
                event = event.with_workspace(&workspace);
            }
            let attempt = match context.begin(event) {
                Ok(attempt) => attempt,
                Err(error) => {
                    eprintln!("Audit is unavailable; file was not changed: {error}");
                    return 1;
                }
            };
            let write = replace_if_unchanged(&file, code.as_bytes(), formatted.as_bytes());
            let outcome = match &write {
                Ok(()) => teal_security::Outcome::Succeeded,
                Err(FmtWriteError::Failed(_)) => teal_security::Outcome::Failed,
                Err(FmtWriteError::Unknown(_)) => teal_security::Outcome::Unknown,
            };
            if let Err(error) = attempt.finish(outcome) {
                eprintln!("Audit result could not be recorded; file state may be unknown: {error}");
                return 1;
            }
            if let Err(error) = write {
                eprintln!("Unable to write to {}: {error}", file.display());
                failed = true;
            }
        }
    }
    i32::from(failed)
}

#[derive(Debug, thiserror::Error)]
enum FmtWriteError {
    #[error("{0}")]
    Failed(std::io::Error),
    #[error("atomic replacement may have an unknown effect: {0}")]
    Unknown(std::io::Error),
}

fn replace_if_unchanged(
    file: &Path,
    original: &[u8],
    formatted: &[u8],
) -> Result<(), FmtWriteError> {
    let metadata = std::fs::symlink_metadata(file).map_err(FmtWriteError::Failed)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(FmtWriteError::Failed(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "source must be a regular file, not a link",
        )));
    }
    let parent = file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(FmtWriteError::Failed)?;
    staged.write_all(formatted).map_err(FmtWriteError::Failed)?;
    staged.as_file().sync_all().map_err(FmtWriteError::Failed)?;
    std::fs::set_permissions(staged.path(), metadata.permissions())
        .map_err(FmtWriteError::Failed)?;

    // 暂存完整输出后才复查当前字节；变化时保留用户写入且不发布暂存内容。
    let current = std::fs::read(file).map_err(FmtWriteError::Failed)?;
    let current_metadata = std::fs::symlink_metadata(file).map_err(FmtWriteError::Failed)?;
    if current != original
        || current_metadata.file_type().is_symlink()
        || !current_metadata.is_file()
    {
        return Err(FmtWriteError::Failed(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "source changed while formatting",
        )));
    }
    staged
        .persist(file)
        .map_err(|error| FmtWriteError::Unknown(error.error))?;
    Ok(())
}

fn is_standard_input(path: &std::path::Path) -> bool {
    path.as_os_str() == OsStr::new("-")
}

fn format_standard_input(check: bool) -> i32 {
    let mut code = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut code) {
        eprintln!("Unable to read standard input: {error}");
        return 1;
    }
    let formatted = match teal_fmt::format_named("<stdin>", &code) {
        Ok(formatted) => formatted,
        Err(error) => {
            eprintln!("{:?}", miette::Report::new(error));
            return 1;
        }
    };
    if check {
        if formatted != code {
            eprintln!("Standard input needs to be formatted");
            return 1;
        }
        return 0;
    }
    if let Err(error) = std::io::stdout().write_all(formatted.as_bytes()) {
        eprintln!("Unable to write to standard output: {error}");
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_replace_rejects_changed_source_without_overwriting_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.tl");
        std::fs::write(&path, b"changed by another writer").unwrap();

        let error = replace_if_unchanged(&path, b"original", b"formatted").unwrap_err();

        assert!(matches!(error, FmtWriteError::Failed(ref error)
            if error.kind() == std::io::ErrorKind::AlreadyExists));
        assert_eq!(std::fs::read(&path).unwrap(), b"changed by another writer");
    }

    #[test]
    fn atomic_replace_rejects_a_symbolic_link_without_touching_its_target() {
        let directory = tempfile::tempdir().unwrap();
        let outside = directory.path().join("outside.tl");
        let link = directory.path().join("linked.tl");
        std::fs::write(&outside, b"outside bytes").unwrap();
        if let Err(error) = file_link(&outside, &link) {
            #[cfg(windows)]
            {
                eprintln!(
                    "skipping file-link regression without Windows symlink permission: {error}"
                );
                return;
            }
            #[cfg(unix)]
            panic!("file link should be creatable: {error}");
        }

        assert!(replace_if_unchanged(&link, b"outside bytes", b"formatted").is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside bytes");
    }

    #[cfg(unix)]
    fn file_link(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn file_link(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }
}
