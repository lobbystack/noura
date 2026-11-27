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
//! sync-server billing announce-price --plan PLAN --effective YYYY-MM-DD
//!                         --old-en TEXT --new-en TEXT --old-fr TEXT --new-fr TEXT
//!                         [--data DIR] [--database-url URL]
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
//! - `NOURA_ALLOWED_EMAILS`: the comma-separated addresses that may sign up,
//!   or `*` for everyone.
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
//! - `NOURA_MAX_CONNECTIONS`: how many connections stay open at once,
//!   realtime channels included, 1,024 by default. Keep the process's file
//!   descriptor limit above it.
//! - `NOURA_POLAR_ACCESS_TOKEN`, `NOURA_POLAR_WEBHOOK_SECRET` and
//!   `NOURA_POLAR_PRODUCTS`, with an optional `NOURA_POLAR_API`: sell plans
//!   through Polar, with its products mapped to plan IDs as `plan=product`
//!   pairs. Without them the server sells no plans.
//! - `NOURA_MERCHANT_NAME`, `NOURA_MERCHANT_ADDRESS`, `NOURA_MERCHANT_EMAIL`
//!   and `NOURA_MERCHANT_PHONE`, which a server that sells plans needs, and
//!   `NOURA_TERMS_URL` and `NOURA_TERMS_URL_FR`: who sells the plans, and
//!   the terms in each language, for the pages and the mail about plans.
//! - `NOURA_SIGN_UPS_PER_DAY` and `NOURA_BLOCKED_EMAIL_DOMAINS`: while the
//!   server sells plans, the new accounts one client IP starts per UTC day,
//!   5 by default, and a file of email domains that may not sign up, on top
//!   of the built-in disposable ones.
//! - `NOURA_RATE_PUSHES_PER_SECOND`, `NOURA_RATE_PUSH_BYTES_PER_MINUTE`,
//!   `NOURA_RATE_UPLOAD_BYTES_PER_MINUTE`, `NOURA_RATE_READ_BYTES_PER_MINUTE`,
//!   `NOURA_RATE_BOOTSTRAP_BYTES_PER_MINUTE`,
//!   `NOURA_RATE_OTHER_REQUESTS_PER_MINUTE` and
//!   `NOURA_RATE_SIGN_IN_PER_MINUTE`: the rate limits of each device, and of
//!   each client IP for sign-in.
//!
//! `billing announce-price` records a price change at least 30 days ahead,
//! with the clause before and after in English and in French; a running
//! server that sells plans mails each subscriber to the plan its notices.
//!
//! `blobs migrate-to-s3` moves a disk store's segments into the bucket those
//! variables name, checks each one's hash, and deletes the local copy only
//! then. Stop `serve` first, and rerun it after an interruption.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use sync_server::postgres::{SECRET_FILE_VARIABLE, SECRET_VARIABLE, secret_from};

const USAGE: &str = "usage: sync-server <migrate|serve|healthcheck|blobs migrate-to-s3|backup|restore|\
                     mark-restored|billing announce-price|version> [--data DIR] \
                     [--database-url URL] [--listen ADDRESS] [--origin ORIGIN] [--to FILE] \
                     [--from FILE] [--secret FILE]";

/// The options of `billing announce-price`.
const PRICE_OPTIONS: [&str; 6] = [
    "--plan",
    "--effective",
    "--old-en",
    "--new-en",
    "--old-fr",
    "--new-fr",
];

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
    /// The options of `billing announce-price`, by flag.
    price: std::collections::BTreeMap<&'static str, String>,
}

fn arguments() -> Result<Arguments, String> {
    let mut args = std::env::args().skip(1);
    let mut command = args.next().ok_or_else(|| USAGE.to_owned())?;
    if command == "blobs" || command == "billing" {
        let sub = args.next().ok_or_else(|| USAGE.to_owned())?;
        command = format!("{command} {sub}");
    }
    let mut data_dir =
        std::env::var_os("NOURA_DATA_DIR").map_or_else(|| PathBuf::from("data"), PathBuf::from);
    let mut database_url = std::env::var("NOURA_DATABASE_URL")
        .ok()
        .filter(|url| !url.is_empty());
    let mut listen = std::env::var("NOURA_LISTEN").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    let mut origin = env("NOURA_PUBLIC_ORIGIN");
    let (mut to, mut from, mut secret) = (None, None, None);
    let mut price = std::collections::BTreeMap::new();
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
            other => match PRICE_OPTIONS.iter().find(|option| **option == other) {
                Some(option) => {
                    price.insert(*option, value);
                }
                None => return Err(format!("unknown option {flag}\n{USAGE}")),
            },
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
        price,
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
    config.billing = polar()?;
    config.merchant = merchant(config.billing.is_some())?;
    if let Some(value) = env("NOURA_SIGN_UPS_PER_DAY") {
        config.sign_ups_per_day = Some(
            value
                .parse()
                .map_err(|_| "NOURA_SIGN_UPS_PER_DAY isn't a whole number")?,
        );
    }
    if let Some(path) = env("NOURA_BLOCKED_EMAIL_DOMAINS") {
        config.blocked_email_domains = std::fs::read_to_string(&path)
            .map_err(|e| format!("can't read NOURA_BLOCKED_EMAIL_DOMAINS: {e}"))?;
    }
    if let Some(value) = env("NOURA_MAX_CONNECTIONS") {
        config.connections.connections = value
            .parse()
            .ok()
            .filter(|count| *count > 0)
            .ok_or("NOURA_MAX_CONNECTIONS isn't a positive whole number")?;
    }
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

/// Polar's settings from `NOURA_POLAR_*`: all three of the token, the
/// webhook secret and the products, or none of them.
fn polar() -> Result<Option<sync_server::billing::polar::PolarConfig>, String> {
    let token = env("NOURA_POLAR_ACCESS_TOKEN");
    let secret = env("NOURA_POLAR_WEBHOOK_SECRET");
    let products = env("NOURA_POLAR_PRODUCTS");
    let (Some(access_token), Some(webhook_secret), Some(products)) =
        (token.clone(), secret.clone(), products.clone())
    else {
        if token.is_some() || secret.is_some() || products.is_some() {
            return Err(
                "set NOURA_POLAR_ACCESS_TOKEN, NOURA_POLAR_WEBHOOK_SECRET and NOURA_POLAR_PRODUCTS together"
                    .into(),
            );
        }
        return Ok(None);
    };
    let api = env("NOURA_POLAR_API").unwrap_or_else(|| sync_server::billing::polar::API.to_owned());
    if !api.starts_with("https://") {
        return Err("NOURA_POLAR_API isn't an HTTPS URL".into());
    }
    Ok(Some(sync_server::billing::polar::PolarConfig {
        access_token,
        webhook_secret,
        api,
        products: sync_server::billing::polar::PolarConfig::parse_products(&products)?,
    }))
}

/// Who sells the plans, from `NOURA_MERCHANT_*` and `NOURA_TERMS_URL*`,
/// which a server that sells plans must name in full.
fn merchant(billing: bool) -> Result<Option<sync_server::billing::Merchant>, String> {
    let names = [
        "NOURA_MERCHANT_NAME",
        "NOURA_MERCHANT_ADDRESS",
        "NOURA_MERCHANT_EMAIL",
        "NOURA_MERCHANT_PHONE",
    ];
    let values: Vec<Option<String>> = names.iter().map(|name| env(name)).collect();
    if !billing {
        return Ok(None);
    }
    if let Some(missing) = names.iter().zip(&values).find(|(_, v)| v.is_none()) {
        return Err(format!(
            "a server that sells plans needs {}: the merchant's name, street address, email and phone",
            missing.0
        ));
    }
    let [name, address, email, phone] = <[Option<String>; 4]>::try_from(values)
        .map_err(|_| "four merchant settings")?
        .map(Option::unwrap_or_default);
    Ok(Some(sync_server::billing::Merchant {
        name,
        address,
        email,
        phone,
        terms_en: env("NOURA_TERMS_URL").unwrap_or_else(|| "https://noura.app/terms/".into()),
        terms_fr: env("NOURA_TERMS_URL_FR").unwrap_or_else(|| "https://noura.app/fr/terms/".into()),
    }))
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
    if let Some(side) = &restored.set_aside {
        eprintln!(
            "sync-server: the old database couldn't be read; it stays at {}",
            side.display()
        );
    }
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

fn announce_price(mut arguments: Arguments) -> Result<(), String> {
    let mut option = |name: &str| {
        arguments
            .price
            .remove(name)
            .ok_or_else(|| format!("billing announce-price needs {name}\n{USAGE}"))
    };
    let announcement = sync_server::billing::Announcement {
        plan: option("--plan")?,
        effective: option("--effective")?,
        old_en: option("--old-en")?,
        new_en: option("--new-en")?,
        old_fr: option("--old-fr")?,
        new_fr: option("--new-fr")?,
    };
    let database = postgres(arguments.database_url)?;
    let config = sync_server::Config::new(arguments.data_dir, listen_address(&arguments.listen)?);
    let change = runtime().block_on(async {
        match &database {
            None => {
                let store = sync_server::open_store(&config)
                    .await
                    .map_err(|e| e.to_string())?;
                sync_server::billing::announce_price(&store, announcement).await
            }
            Some(database) => {
                let store = sync_server::postgres::open_store(database)
                    .await
                    .map_err(|e| e.to_string())?;
                sync_server::billing::announce_price(&store, announcement).await
            }
        }
    })?;
    eprintln!("sync-server: announced price change {change}; the server mails its notices");
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
        "billing announce-price" => announce_price(arguments),
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
