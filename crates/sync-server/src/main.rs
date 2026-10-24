//! `sync-server`: the Noura sync service.
//!
//! ```text
//! sync-server migrate     [--data DIR]
//! sync-server serve       [--data DIR] [--listen ADDRESS] [--origin ORIGIN]
//! sync-server healthcheck [--listen ADDRESS]
//! sync-server version
//! ```
//!
//! `NOURA_DATA_DIR` and `NOURA_LISTEN` set the defaults, which are `data`
//! and `127.0.0.1:8080`. `serve` also reads:
//!
//! - `NOURA_PUBLIC_ORIGIN` (or `--origin`): the HTTPS origin people reach the
//!   server at. A server on a loopback address defaults to
//!   `http://localhost:{port}`.
//! - `NOURA_ALLOWED_EMAILS`: the comma-separated addresses that may sign up.
//!   Empty admits nobody.
//! - `NOURA_MAIL_FROM`: the sender of sign-in mail.
//! - `NOURA_SMTP_URL`, or `NOURA_RESEND_API_KEY` with an optional
//!   `NOURA_RESEND_ENDPOINT`: how the server sends mail.
//! - `NOURA_ACCOUNT_WEB_DIR`: the account-web build the pages load.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

// The PostgreSQL store holds no code yet (ticket 45). Linking it now means
// adding it can't surprise the static build.
use sync_store_postgres as _;

const USAGE: &str = "usage: sync-server <migrate|serve|healthcheck|version> [--data DIR] [--listen ADDRESS] [--origin ORIGIN]";

/// A set, non-empty environment variable.
fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// The `serve` configuration from the arguments and the environment.
fn serve_config(arguments: Arguments, listen: SocketAddr) -> sync_server::Config {
    let mail = match (env("NOURA_SMTP_URL"), env("NOURA_RESEND_API_KEY")) {
        (Some(url), _) => sync_server::MailConfig::Smtp(url),
        (None, Some(api_key)) => sync_server::MailConfig::Resend {
            api_key,
            endpoint: env("NOURA_RESEND_ENDPOINT"),
        },
        (None, None) => sync_server::MailConfig::None,
    };
    let mut config = sync_server::Config::new(arguments.data_dir, listen);
    config.origin = arguments.origin;
    config.allowed_emails = env("NOURA_ALLOWED_EMAILS").unwrap_or_default();
    if let Some(from) = env("NOURA_MAIL_FROM") {
        config.mail_from = from;
    }
    config.mail = mail;
    config.account_web = env("NOURA_ACCOUNT_WEB_DIR").map(PathBuf::from);
    config
}

struct Arguments {
    command: String,
    data_dir: PathBuf,
    listen: String,
    origin: Option<String>,
}

fn arguments() -> Result<Arguments, String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or_else(|| USAGE.to_owned())?;
    let mut data_dir =
        std::env::var_os("NOURA_DATA_DIR").map_or_else(|| PathBuf::from("data"), PathBuf::from);
    let mut listen = std::env::var("NOURA_LISTEN").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    let mut origin = env("NOURA_PUBLIC_ORIGIN");
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))?;
        match flag.as_str() {
            "--data" => data_dir = PathBuf::from(value),
            "--listen" => listen = value,
            "--origin" => origin = Some(value),
            _ => return Err(format!("unknown option {flag}\n{USAGE}")),
        }
    }
    Ok(Arguments {
        command,
        data_dir,
        listen,
        origin,
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
    let command = arguments.command.clone();
    let result = match command.as_str() {
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
                .block_on(sync_server::serve(serve_config(arguments, listen)))
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
