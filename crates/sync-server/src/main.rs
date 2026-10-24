//! `sync-server`: the Noura sync service.
//!
//! ```text
//! sync-server migrate     [--data DIR] [--database-url URL]
//! sync-server serve       [--data DIR] [--database-url URL] [--listen ADDRESS]
//! sync-server healthcheck [--listen ADDRESS]
//! sync-server version
//! ```
//!
//! `NOURA_DATA_DIR`, `NOURA_DATABASE_URL` and `NOURA_LISTEN` set the
//! defaults, which are `data`, none and `127.0.0.1:8080`. Without a database
//! URL the server runs on SQLite in the data directory. With one it runs on
//! that PostgreSQL database and reads the server secret from
//! `NOURA_SERVER_SECRET` or from the file `NOURA_SERVER_SECRET_FILE` names.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use sync_server::postgres::{SECRET_FILE_VARIABLE, SECRET_VARIABLE, secret_from};

const USAGE: &str = "usage: sync-server <migrate|serve|healthcheck|version> [--data DIR] \
                     [--database-url URL] [--listen ADDRESS]";

struct Arguments {
    command: String,
    data_dir: PathBuf,
    database_url: Option<String>,
    listen: String,
}

fn arguments() -> Result<Arguments, String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or_else(|| USAGE.to_owned())?;
    let mut data_dir =
        std::env::var_os("NOURA_DATA_DIR").map_or_else(|| PathBuf::from("data"), PathBuf::from);
    let mut database_url = std::env::var("NOURA_DATABASE_URL")
        .ok()
        .filter(|url| !url.is_empty());
    let mut listen = std::env::var("NOURA_LISTEN").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))?;
        match flag.as_str() {
            "--data" => data_dir = PathBuf::from(value),
            "--database-url" => database_url = Some(value),
            "--listen" => listen = value,
            _ => return Err(format!("unknown option {flag}\n{USAGE}")),
        }
    }
    Ok(Arguments {
        command,
        data_dir,
        database_url,
        listen,
    })
}

/// The PostgreSQL database and its secret, or `None` for SQLite mode,
/// where the secret lives in the data directory and the variables would
/// go unused.
fn postgres(url: Option<String>) -> Result<Option<sync_server::Postgres>, String> {
    let secret = secret_from(|name| std::env::var_os(name)).map_err(|e| e.to_string())?;
    match url {
        Some(url) => Ok(Some(sync_server::Postgres { url, secret })),
        None if secret.is_some() => Err(format!(
            "{SECRET_VARIABLE} and {SECRET_FILE_VARIABLE} apply to PostgreSQL mode; without a \
             database URL the server keeps its secret in the data directory"
        )),
        None => Ok(None),
    }
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

fn migrate(arguments: Arguments) -> Result<(), String> {
    let database = postgres(arguments.database_url)?;
    let migrated = runtime().block_on(async {
        match &database {
            None => sync_server::migrate(&arguments.data_dir).await,
            Some(database) => sync_server::postgres::migrate(database).await,
        }
    });
    let migrated = migrated.map_err(|e| e.to_string())?;
    if migrated.created_secret {
        eprintln!("sync-server: created the server secret");
    }
    eprintln!(
        "sync-server: ran {} migration(s); the database is current",
        migrated.applied.len()
    );
    Ok(())
}

fn serve(arguments: Arguments) -> Result<(), String> {
    let listen = listen_address(&arguments.listen)?;
    let database = postgres(arguments.database_url)?;
    runtime()
        .block_on(sync_server::serve(sync_server::Config {
            data_dir: arguments.data_dir,
            listen,
            postgres: database,
        }))
        .map_err(|e| e.to_string())
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
        "migrate" => migrate(arguments),
        "serve" => serve(arguments),
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
