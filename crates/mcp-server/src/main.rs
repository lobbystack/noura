//! `noura-mcp --workspace <folder>`: Noura's MCP server on its own, as the
//! desktop binary's `mcp` mode serves it. Stdout carries only MCP, and
//! warnings go to stderr.

use std::{ffi::OsStr, process::ExitCode};

fn main() -> ExitCode {
    if std::env::args_os().nth(1).as_deref() == Some(OsStr::new("--noura-crdt-worker")) {
        return match local_core::sync::collaboration::run_worker_stdio() {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        };
    }
    noura_mcp::run(std::env::args_os().skip(1))
}
