//! The Noura sync service (docs/workspace-format/sync-v2.md): one binary
//! with `serve`, `migrate` and `healthcheck`. By default it keeps
//! everything in one data directory: the SQLite database, the server secret
//! beside it, and the daily log files. With a database URL it stores
//! everything in PostgreSQL instead, which several instances can serve at
//! once, each given the same server secret, and keeps only its log files in
//! the data directory.

#![forbid(unsafe_code)]

pub mod accounts;
pub mod data;
pub mod http;
pub mod logs;
pub mod postgres;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sync_store::{Backend, Limiter, Store};
use sync_store_sqlite::SqliteBackend;
use tokio::net::TcpListener;

pub use accounts::{Accounts, Allowlist, MailTransport, PublicOrigin};
pub use data::{CommandError, Migrated, check_ready, migrate};
pub use http::AppState;
pub use postgres::Postgres;

/// How `serve` sends mail.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum MailConfig {
    /// No transport: sign-up and email sign-in can't work.
    #[default]
    None,
    /// SMTP through lettre, at this URL.
    Smtp(String),
    /// Resend's HTTPS API with this key, at its endpoint or another one.
    Resend {
        /// The API key.
        api_key: String,
        /// Another endpoint than Resend's, such as a relay.
        endpoint: Option<String>,
    },
}

/// What `serve` runs with.
#[derive(Clone, Debug)]
pub struct Config {
    /// The data directory, which holds the log files, and in SQLite mode
    /// the database and the server secret.
    pub data_dir: PathBuf,
    /// The address to listen on.
    pub listen: SocketAddr,
    /// The public origin, such as `https://sync.noura.app`. Without one,
    /// a server on a loopback address serves `http://localhost:{port}`.
    pub origin: Option<String>,
    /// The comma-separated email allowlist; empty admits nobody.
    pub allowed_emails: String,
    /// The sender of sign-in mail.
    pub mail_from: String,
    /// The mail transport.
    pub mail: MailConfig,
    /// The account-web build: `account_web.js`, `account_web_bg.wasm` and
    /// `THIRD_PARTY_NOTICES.txt`.
    pub account_web: Option<PathBuf>,
    /// The PostgreSQL database to use instead of SQLite.
    pub postgres: Option<Postgres>,
}

impl Config {
    /// A configuration with no origin, allowlist or mail.
    pub fn new(data_dir: PathBuf, listen: SocketAddr) -> Self {
        Self {
            data_dir,
            listen,
            origin: None,
            allowed_emails: String::new(),
            mail_from: "Noura <noura@localhost>".into(),
            mail: MailConfig::None,
            account_web: None,
            postgres: None,
        }
    }
}

/// Opens the SQLite store in a ready data directory, with the server secret
/// that account IDs derive from.
pub async fn open_store(config: &Config) -> Result<Store<SqliteBackend>, CommandError> {
    check_ready(&config.data_dir).await?;
    let secret = data::read_secret(&config.data_dir.join(data::SECRET_FILE))?
        .ok_or_else(|| CommandError("the data directory holds no server secret".into()))?;
    let backend = SqliteBackend::open(&data::database_path(&config.data_dir))
        .await
        .map_err(|e| CommandError(format!("can't open the database: {e}")))?;
    Ok(Store::open(backend)
        .await
        .map_err(|e| CommandError(format!("can't open the store: {e}")))?
        .with_secret(secret))
}

/// The public origin `serve` uses: the configured one, which must be HTTPS
/// except on loopback, or `http://localhost:{port}` on a loopback address.
pub fn public_origin(
    configured: Option<&str>,
    bound: SocketAddr,
) -> Result<PublicOrigin, CommandError> {
    match configured {
        Some(origin) => PublicOrigin::parse(origin).map_err(CommandError),
        None if bound.ip().is_loopback() => {
            PublicOrigin::parse(&format!("http://localhost:{}", bound.port())).map_err(CommandError)
        }
        None => Err(CommandError(
            "set NOURA_PUBLIC_ORIGIN to the server's HTTPS origin, such as https://sync.example.com"
                .into(),
        )),
    }
}

/// The account pages for `config` at `origin`.
pub fn accounts_for(config: &Config, origin: PublicOrigin) -> Result<Accounts, CommandError> {
    let mailer: Box<dyn MailTransport> = match &config.mail {
        MailConfig::None => Box::new(accounts::NoMail),
        MailConfig::Smtp(url) => {
            Box::new(accounts::mail::Smtp::from_url(url).map_err(|e| CommandError(e.to_string()))?)
        }
        MailConfig::Resend { api_key, endpoint } => Box::new(accounts::mail::Resend::new(
            api_key.clone(),
            endpoint.clone(),
        )),
    };
    Ok(Accounts::new(
        origin,
        Allowlist::parse(&config.allowed_emails),
        config.mail_from.clone(),
        mailer,
        config.account_web.clone(),
    ))
}

/// Serves `state` on `listener` until `shutdown` resolves.
pub async fn serve_on<B: Backend>(
    state: AppState<B>,
    listener: TcpListener,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, http::router(state))
        .with_graceful_shutdown(shutdown)
        .await
}

/// Serves `store` on a new listener at `address` in the background, for
/// tests, with account pages at `http://localhost:{port}` that admit no
/// sign-up, and returns the bound address.
pub async fn spawn<B: Backend>(store: Arc<Store<B>>, address: &str) -> std::io::Result<SocketAddr> {
    spawn_with(store, address, Accounts::local).await
}

/// Serves `store` on a new listener at `address` in the background, with
/// the account pages `accounts` builds for the origin
/// `http://localhost:{port}`, and returns the bound address.
pub async fn spawn_with<B: Backend>(
    store: Arc<Store<B>>,
    address: &str,
    accounts: impl FnOnce(PublicOrigin) -> Accounts,
) -> std::io::Result<SocketAddr> {
    let listener = TcpListener::bind(address).await?;
    let bound = listener.local_addr()?;
    let origin = PublicOrigin::parse(&format!("http://localhost:{}", bound.port()))
        .map_err(std::io::Error::other)?;
    let state = AppState {
        store,
        accounts: Arc::new(accounts(origin)),
    };
    tokio::spawn(serve_on(state, listener, std::future::pending()));
    Ok(bound)
}

async fn shutdown_signal() {
    let interrupt = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("the terminate signal installs");
        tokio::select! {
            _ = interrupt => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = interrupt.await;
    }
}

/// `sync-server serve`: refuses a database that isn't ready or a public
/// origin that isn't HTTPS away from loopback, then serves until it gets an
/// interrupt or a terminate signal, and closes the database.
pub async fn serve(config: Config) -> Result<(), CommandError> {
    match &config.postgres {
        None => run(Arc::new(open_store(&config).await?), &config).await,
        Some(database) => run(Arc::new(postgres::open_store(database).await?), &config).await,
    }
}

async fn run<B: Backend>(store: Arc<Store<B>>, config: &Config) -> Result<(), CommandError> {
    let listener = TcpListener::bind(config.listen)
        .await
        .map_err(|e| CommandError(format!("can't listen on {}: {e}", config.listen)))?;
    let bound = listener
        .local_addr()
        .map_err(|e| CommandError(e.to_string()))?;
    let origin = public_origin(config.origin.as_deref(), bound)?;
    let accounts = accounts_for(config, origin)?;
    if !accounts.has_passkeys() {
        eprintln!(
            "sync-server: passkeys need a domain name in the public origin; the pages offer email links alone"
        );
    }
    let log_dir = config.data_dir.join(data::LOG_DIR);
    logs::init(&log_dir).map_err(|e| CommandError(format!("can't open the log: {e}")))?;
    let pruning = logs::spawn_pruning(log_dir, logs::PRUNE_PERIOD);
    let counters = spawn_counter_pruning(store.backend().limiter());
    let expiry = accounts::spawn_expiry(store.clone());
    tracing::info!("serving");
    eprintln!("sync-server listening on {bound}");
    let state = AppState {
        store: store.clone(),
        accounts: Arc::new(accounts),
    };
    let served = serve_on(state, listener, shutdown_signal()).await;
    pruning.abort();
    expiry.abort();
    counters.abort();
    store.backend().close().await;
    tracing::info!("stopped");
    served.map_err(|e| CommandError(format!("the server failed: {e}")))
}

/// Deletes the limiter's counts of ended windows every minute, so no count
/// outlives its window by more than that (§29).
fn spawn_counter_pruning(limiter: Arc<dyn Limiter>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(sync_store::limiter::PRUNE_EVERY);
        loop {
            every.tick().await;
            if let Err(error) = limiter.prune().await {
                tracing::warn!("can't prune the rate counters: {error}");
            }
        }
    })
}

/// `sync-server healthcheck`: whether a Noura sync server answers
/// `GET /v2/limits` at `address` within five seconds. An unspecified
/// address checks the loopback address on that port.
pub fn healthcheck(mut address: SocketAddr) -> bool {
    if address.ip().is_unspecified() {
        address.set_ip(match address {
            SocketAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
            SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
        });
    }
    let timeout = Duration::from_secs(5);
    let check = || -> std::io::Result<bool> {
        let mut stream = TcpStream::connect_timeout(&address, timeout)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        stream.write_all(
            b"GET /v2/limits HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )?;
        let mut response = Vec::new();
        stream.take(64 * 1024).read_to_end(&mut response)?;
        let text = String::from_utf8_lossy(&response).to_ascii_lowercase();
        Ok(text.starts_with("http/1.1 200")
            && text
                .contains(&sync_protocol::messages::RESTORE_GENERATION_HEADER.to_ascii_lowercase()))
    };
    check().unwrap_or(false)
}

/// The libraries the binary links, for `sync-server version`: the SQLite
/// that sqlx bundles and the OpenSSL that webauthn-rs needs, built from
/// source so the Linux server stays one static binary.
pub async fn linked_libraries() -> Result<String, Box<dyn std::error::Error>> {
    let origin = url::Url::parse("https://localhost")?;
    webauthn_rs::WebauthnBuilder::new("localhost", &origin)?.build()?;
    let sqlite = sync_store_sqlite::sqlite_version().await?;
    Ok(format!("SQLite {sqlite}, {}", openssl::version::version()))
}
