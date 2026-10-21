//! `sync-server`: the Noura sync service.
//!
//! ```text
//! sync-server migrate     [--data DIR]
//! sync-server serve       [--data DIR] [--listen ADDRESS]
//! sync-server healthcheck [--listen ADDRESS]
//! sync-server version
//! ```
//!
//! `NOURA_DATA_DIR` and `NOURA_LISTEN` set the defaults, which are `data`
//! and `127.0.0.1:8080`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

// The PostgreSQL store holds no code yet (ticket 45). Linking it now means
// adding it can't surprise the static build.
use sync_store_postgres as _;

const USAGE: &str =
    "usage: sync-server <migrate|serve|healthcheck|version> [--data DIR] [--listen ADDRESS]";

struct Arguments {
    command: String,
    data_dir: PathBuf,
    listen: String,
}

fn arguments() -> Result<Arguments, String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or_else(|| USAGE.to_owned())?;
    let mut data_dir =
        std::env::var_os("NOURA_DATA_DIR").map_or_else(|| PathBuf::from("data"), PathBuf::from);
    let mut listen = std::env::var("NOURA_LISTEN").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))?;
        match flag.as_str() {
            "--data" => data_dir = PathBuf::from(value),
            "--listen" => listen = value,
            _ => return Err(format!("unknown option {flag}\n{USAGE}")),
        }
    }
    Ok(Arguments {
        command,
        data_dir,
        listen,
    })
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the async runtime starts")
}

fn listen_address(text: &str) -> Result<SocketAddr, String> {
    text.parse()
        .map_err(|_| format!("{text} isn't an address and port, such as 127.0.0.1:8080"))
}

fn main() -> ExitCode {
    let arguments = match arguments() {
        Ok(arguments) => arguments,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let result = match arguments.command.as_str() {
        "migrate" => runtime()
            .block_on(sync_server::migrate(&arguments.data_dir))
            .map(|migrated| {
                if migrated.created_secret {
                    eprintln!("sync-server: created the server secret");
                }
                eprintln!(
                    "sync-server: ran {} migration(s); the database is current",
                    migrated.applied.len()
                );
            })
            .map_err(|e| e.to_string()),
        "serve" => listen_address(&arguments.listen).and_then(|listen| {
            runtime()
                .block_on(sync_server::serve(sync_server::Config {
                    data_dir: arguments.data_dir,
                    listen,
                }))
                .map_err(|e| e.to_string())
        }),
        "healthcheck" => listen_address(&arguments.listen).and_then(|listen| {
            if sync_server::healthcheck(listen) {
                Ok(())
            } else {
                Err(format!("no sync server answers at {listen}"))
            }
        }),
        "version" => runtime()
            .block_on(sync_server::linked_libraries())
            .map(|libraries| println!("sync-server {} ({libraries})", env!("CARGO_PKG_VERSION")))
            .map_err(|e| e.to_string()),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("sync-server: {message}");
            ExitCode::FAILURE
        }
    }
}
