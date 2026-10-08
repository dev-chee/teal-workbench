//! `teal-lsp` 的标准输入入口。

use clap::{CommandFactory, FromArgMatches, Parser};
mod leaf_observability;

#[derive(Debug, Parser)]
#[command(name = "teal-lsp", version, about = "Start the Teal language server")]
struct Cli {}

fn main() {
    let argv =
        std::iter::once(std::ffi::OsString::from("teal-lsp")).chain(std::env::args_os().skip(1));
    let matches = match Cli::command()
        .bin_name("teal-lsp")
        .try_get_matches_from(argv)
    {
        Ok(matches) => matches,
        Err(error) => error.exit(),
    };
    Cli::from_arg_matches(&matches).expect("clap type and value must be consistent");

    let observation = leaf_observability::start();
    if let Err(error) = &observation {
        eprintln!("Teal tracing is unavailable: {error}");
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("failed to create the LSP async runtime: {error}");
            std::process::exit(1);
        }
    };
    let code = match runtime.block_on(teal_lsp::run_stdio()) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("LSP stdio failed: {error}");
            1
        }
    };
    if let Ok(observation) = observation {
        observation.shutdown();
    }
    std::process::exit(code);
}
