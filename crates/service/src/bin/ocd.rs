//! Thin `ocd` binary: parse, log, exit-code adapter.

use clap::Parser;
use open_compute_service::cli::{Cli, execute};
use std::ffi::OsStr;
use std::io::{self, IsTerminal};
use std::process::ExitCode;

fn main() -> ExitCode {
    if let Some((max_address_space_bytes, max_cpu_seconds)) = document_parser_child_limits() {
        if apply_document_parser_limits(max_address_space_bytes, max_cpu_seconds).is_err() {
            return ExitCode::from(70);
        }
        let stdin = io::stdin();
        let stdout = io::stdout();
        return match open_compute_document_parser::run_child(stdin.lock(), stdout.lock()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::from(70),
        };
    }
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    else {
        return ExitCode::from(open_compute_service::exit::ExitClass::Run.code());
    };
    runtime.block_on(run_cli())
}

async fn run_cli() -> ExitCode {
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

fn document_parser_child_limits() -> Option<(u64, u64)> {
    let mut arguments = std::env::args_os();
    let _program = arguments.next();
    if arguments.next().as_deref() != Some(OsStr::new("__document-parser-v1")) {
        return None;
    }
    let max_address_space_bytes = arguments.next()?.to_str()?.parse().ok()?;
    let max_cpu_seconds = arguments.next()?.to_str()?.parse().ok()?;
    if arguments.next().is_some() || max_address_space_bytes == 0 || max_cpu_seconds == 0 {
        return None;
    }
    Some((max_address_space_bytes, max_cpu_seconds))
}

fn apply_document_parser_limits(
    max_address_space_bytes: u64,
    max_cpu_seconds: u64,
) -> io::Result<()> {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

    fn lower(resource: Resource, requested: u64) -> io::Result<()> {
        let inherited = getrlimit(resource);
        let bounded = inherited
            .maximum
            .map_or(requested, |limit| limit.min(requested));
        setrlimit(
            resource,
            Rlimit {
                current: Some(bounded),
                maximum: Some(bounded),
            },
        )
        .map_err(io::Error::other)
    }

    #[cfg(target_os = "macos")]
    {
        // Darwin exposes RLIMIT_AS but rejects setrlimit requests. Keep the
        // other independent child fences active; macOS RSS enforcement remains
        // an explicit release-qualification item.
        let _ = lower(Resource::As, max_address_space_bytes);
    }
    #[cfg(not(target_os = "macos"))]
    lower(Resource::As, max_address_space_bytes)?;
    lower(Resource::Cpu, max_cpu_seconds)?;
    lower(Resource::Core, 0)?;
    lower(Resource::Fsize, 0)
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
