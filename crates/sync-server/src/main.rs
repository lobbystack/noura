//! `sync-server`: the Noura sync service.
//!
//! ```text
//! sync-server migrate     [--data DIR] [--database-url URL]
//! sync-server serve       [--data DIR] [--database-url URL] [--listen ADDRESS]
//!                         [--origin ORIGIN]
//! sync-server healthcheck [--listen ADDRESS]
//! sync-server blobs migrate-to-s3 [--data DIR]
//! sync-server backup --to FILE [--data DIR] [--database-url URL]
//! sync-server restore --from FILE [--secret FILE] [--data DIR]
//! sync-server mark-restored [--data DIR] [--database-url URL]
//! sync-server version
//! ```
//!
//! `NOURA_DATA_DIR`, `NOURA_DATABASE_URL` and `NOURA_LISTEN` set the
//! defaults, which are `data`, none and `127.0.0.1:8080`. Without a database
//! URL the server runs on SQLite in the data directory. With one it runs on
//! that PostgreSQL database and reads the server secret from
//! `NOURA_SERVER_SECRET` or from the file `NOURA_SERVER_SECRET_FILE` names.
//! `serve` also reads:
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
//! - `NOURA_RETAINED_BYTES`: the weight each domain's log keeps behind a
//!   checkpoint, 1 GiB by default.
//! - `NOURA_WORKSPACE_QUOTA_BYTES`: each workspace's storage quota, unlimited
//!   by default.
//! - `NOURA_S3_BUCKET`, with an optional `NOURA_S3_ENDPOINT` for an
//!   S3-compatible service: keep blob segments in that bucket instead of the
//!   data directory's `blobs` directory. The standard `AWS_*` variables give
//!   the credentials and region.
//! - `NOURA_TRUSTED_PROXY_HEADER`: the header your reverse proxy sets to the
//!   client's IP. Without it the server ignores such headers and counts
//!   sign-in per connection address.
//! - `NOURA_RATE_PUSHES_PER_SECOND`, `NOURA_RATE_PUSH_BYTES_PER_MINUTE`,
//!   `NOURA_RATE_UPLOAD_BYTES_PER_MINUTE`, `NOURA_RATE_READ_BYTES_PER_MINUTE`,
//!   `NOURA_RATE_BOOTSTRAP_BYTES_PER_MINUTE`,
//!   `NOURA_RATE_OTHER_REQUESTS_PER_MINUTE` and
//!   `NOURA_RATE_SIGN_IN_PER_MINUTE`: the rate limits of each device, and of
//!   each client IP for sign-in.
//!
//! `blobs migrate-to-s3` moves a disk store's segments into the bucket those
//! variables name, checks each one's hash, and deletes the local copy only
//! then. Stop `serve` first, and rerun it after an interruption.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use sync_server::postgres::{SECRET_FILE_VARIABLE, SECRET_VARIABLE, secret_from};

const USAGE: &str = "usage: sync-server <migrate|serve|healthcheck|blobs migrate-to-s3|backup|restore|\
                     mark-restored|version> [--data DIR] [--database-url URL] [--listen ADDRESS] \
                     [--origin ORIGIN] [--to FILE] [--from FILE] [--secret FILE]";

/// A set, non-empty environment variable.
fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

struct Arguments {
    command: String,
    data_dir: PathBuf,
    database_url: Option<String>,
    listen: String,
    origin: Option<String>,
    to: Option<PathBuf>,
    from: Option<PathBuf>,
    secret: Option<PathBuf>,
}

fn arguments() -> Result<Arguments, String> {
    let mut args = std::env::args().skip(1);
    let mut command = args.next().ok_or_else(|| USAGE.to_owned())?;
    if command == "blobs" {
        let sub = args.next().ok_or_else(|| USAGE.to_owned())?;
        command = format!("blobs {sub}");
    }
    let mut data_dir =
        std::env::var_os("NOURA_DATA_DIR").map_or_else(|| PathBuf::from("data"), PathBuf::from);
    let mut database_url = std::env::var("NOURA_DATABASE_URL")
        .ok()
        .filter(|url| !url.is_empty());
    let mut listen = std::env::var("NOURA_LISTEN").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    let mut origin = env("NOURA_PUBLIC_ORIGIN");
    let (mut to, mut from, mut secret) = (None, None, None);
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))?;
        match flag.as_str() {
            "--data" => data_dir = PathBuf::from(value),
            "--database-url" => database_url = Some(value),
            "--listen" => listen = value,
            "--origin" => origin = Some(value),
            "--to" => to = Some(PathBuf::from(value)),
            "--from" => from = Some(PathBuf::from(value)),
            "--secret" => secret = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown option {flag}\n{USAGE}")),
        }
    }
    Ok(Arguments {
        command,
        data_dir,
        database_url,
        listen,
        origin,
        to,
        from,
        secret,
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
    let mail = match (env("NOURA_SMTP_URL"), env("NOURA_RESEND_API_KEY")) {
        (Some(url), _) => sync_server::MailConfig::Smtp(url),
        (None, Some(api_key)) => sync_server::MailConfig::Resend {
            api_key,
            endpoint: env("NOURA_RESEND_ENDPOINT"),
        },
        (None, None) => sync_server::MailConfig::None,
    };
    let mut config = sync_server::Config::new(arguments.data_dir, listen);
    config.postgres = database;
    config.origin = arguments.origin;
    config.allowed_emails = env("NOURA_ALLOWED_EMAILS").unwrap_or_default();
    if let Some(from) = env("NOURA_MAIL_FROM") {
        config.mail_from = from;
    }
    config.mail = mail;
    config.account_web = env("NOURA_ACCOUNT_WEB_DIR").map(PathBuf::from);
    config.blob_store = blob_store();
    config.retention = retention()?;
    config.rate_limits = rate_limits()?;
    config.trusted_proxy_header = env("NOURA_TRUSTED_PROXY_HEADER");
    runtime()
        .block_on(sync_server::serve(config))
        .map_err(|e| e.to_string())
}

/// `NOURA_RETAINED_BYTES` and `NOURA_WORKSPACE_QUOTA_BYTES`, each a number
/// of bytes.
fn retention() -> Result<sync_store::Retention, String> {
    let bytes = |name: &str| {
        env(name)
            .map(|value| {
                value
                    .parse::<u64>()
                    .map_err(|_| format!("{name} isn't a number of bytes"))
            })
            .transpose()
    };
    let mut retention = sync_store::Retention::default();
    if let Some(retained) = bytes("NOURA_RETAINED_BYTES")? {
        retention.retained_bytes = retained;
    }
    retention.quota_bytes = bytes("NOURA_WORKSPACE_QUOTA_BYTES")?;
    Ok(retention)
}

/// The default rate limits with each `NOURA_RATE_*` variable applied.
fn rate_limits() -> Result<sync_server::RateLimits, String> {
    let mut limits = sync_server::RateLimits::default();
    for (name, limit) in [
        ("NOURA_RATE_PUSHES_PER_SECOND", &mut limits.pushes),
        ("NOURA_RATE_PUSH_BYTES_PER_MINUTE", &mut limits.push_bytes),
        (
            "NOURA_RATE_UPLOAD_BYTES_PER_MINUTE",
            &mut limits.upload_bytes,
        ),
        ("NOURA_RATE_READ_BYTES_PER_MINUTE", &mut limits.read_bytes),
        (
            "NOURA_RATE_BOOTSTRAP_BYTES_PER_MINUTE",
            &mut limits.bootstrap_bytes,
        ),
        ("NOURA_RATE_OTHER_REQUESTS_PER_MINUTE", &mut limits.requests),
        ("NOURA_RATE_SIGN_IN_PER_MINUTE", &mut limits.sign_in),
    ] {
        if let Some(value) = env(name) {
            limit.allowance = value
                .parse()
                .map_err(|_| format!("{name} isn't a whole number"))?;
        }
    }
    Ok(limits)
}

/// The blob store `NOURA_S3_BUCKET` and `NOURA_S3_ENDPOINT` name, or the
/// data directory's.
fn blob_store() -> sync_server::BlobStoreConfig {
    match env("NOURA_S3_BUCKET") {
        Some(bucket) => sync_server::BlobStoreConfig::S3 {
            bucket,
            endpoint: env("NOURA_S3_ENDPOINT"),
        },
        None => sync_server::BlobStoreConfig::Disk,
    }
}

fn migrate_blobs(arguments: Arguments) -> Result<(), String> {
    let migration = runtime()
        .block_on(sync_server::migrate_blobs_to_s3(
            &arguments.data_dir,
            &blob_store(),
        ))
        .map_err(|e| e.to_string())?;
    eprintln!(
        "sync-server: moved {} blob segment(s) to S3; set NOURA_S3_BUCKET when you serve",
        migration.moved
    );
    Ok(())
}

fn backup(arguments: Arguments) -> Result<(), String> {
    if arguments.database_url.is_some() {
        return Err(sync_server::restore::backup_postgres().to_string());
    }
    let to = arguments
        .to
        .ok_or_else(|| format!("backup needs --to FILE\n{USAGE}"))?;
    let secret = runtime()
        .block_on(sync_server::restore::backup(&arguments.data_dir, &to))
        .map_err(|e| e.to_string())?;
    eprintln!(
        "sync-server: wrote {} and its secret {}; keep them together",
        to.display(),
        secret.display()
    );
    Ok(())
}

fn raised(generation: u64) {
    eprintln!(
        "sync-server: restore generation {generation}; serve may start now, while the blob store \
         check runs"
    );
}

fn report(restored: sync_server::restore::Restored) {
    let blobs = restored.blobs;
    eprintln!(
        "sync-server: the blob store check finished; quarantined {} object(s), returned {}, gave \
         {} blob(s) a fresh 24 hours, and {} blob(s) wait for devices to upload them again",
        blobs.quarantined, blobs.returned, blobs.refreshed, blobs.missing
    );
}

fn restore(arguments: Arguments) -> Result<(), String> {
    if arguments.database_url.is_some() {
        return Err(
            "restore takes a SQLite backup; after restoring PostgreSQL with its own tools, run \
             mark-restored"
                .to_owned(),
        );
    }
    let from = arguments
        .from
        .ok_or_else(|| format!("restore needs --from FILE\n{USAGE}"))?;
    let restored = runtime()
        .block_on(sync_server::restore::restore(
            &arguments.data_dir,
            &from,
            arguments.secret.as_deref(),
            &blob_store(),
            raised,
        ))
        .map_err(|e| e.to_string())?;
    report(restored);
    Ok(())
}

fn mark_restored(arguments: Arguments) -> Result<(), String> {
    let database = postgres(arguments.database_url)?;
    let store = blob_store();
    let restored = runtime()
        .block_on(async {
            match &database {
                None => {
                    sync_server::restore::mark_restored(&arguments.data_dir, &store, raised).await
                }
                Some(database) => {
                    sync_server::restore::mark_restored_postgres(
                        database,
                        &arguments.data_dir,
                        &store,
                        raised,
                    )
                    .await
                }
            }
        })
        .map_err(|e| e.to_string())?;
    report(restored);
    Ok(())
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
        "blobs migrate-to-s3" => migrate_blobs(arguments),
        "backup" => backup(arguments),
        "restore" => restore(arguments),
        "mark-restored" => mark_restored(arguments),
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
