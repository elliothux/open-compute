//! Thin `ocd` binary: parse, log, exit-code adapter.

use clap::Parser;
use open_compute_service::cli::{Cli, execute};
use std::io::{self, IsTerminal};
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            let _ = err.print();
            if err.kind() == clap::error::ErrorKind::DisplayHelp
                || err.kind() == clap::error::ErrorKind::DisplayVersion
            {
                return ExitCode::SUCCESS;
            }
            return ExitCode::from(open_compute_service::exit::ExitClass::Cli.code());
        }
    };
    init_tracing();
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();
    execute(cli, &mut stdout, &mut stderr).await
}

fn init_tracing() {
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(io::stderr)
        .with_ansi(io::stderr().is_terminal())
        .with_current_span(false)
        .with_span_list(false)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}
