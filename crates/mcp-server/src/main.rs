//! `noura-mcp --workspace <folder>`: Noura's MCP server on its own, as the
//! desktop binary's `mcp` mode serves it. Stdout carries only MCP, and
//! warnings go to stderr.

use std::process::ExitCode;

fn main() -> ExitCode {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--noura-crdt-worker")) {
        return match local_core::sync::collaboration::run_worker_stdio() {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        };
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    let folder = match noura_mcp::workspace_argument(std::env::args_os().skip(1)) {
        Ok(folder) => folder,
        Err(usage) => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };
    let Some(app_data) = app_core::default_app_data_dir() else {
        eprintln!("This system has no app data folder for Noura.");
        return ExitCode::FAILURE;
    };
    let served = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())
        .and_then(|runtime| runtime.block_on(noura_mcp::serve_stdio(app_data, folder)));
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
