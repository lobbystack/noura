//! The Noura sync service (docs/workspace-format/sync-v2.md): one binary
//! with `serve`, `migrate` and `healthcheck`. By default it keeps
//! everything in one data directory: the SQLite database, the server secret
//! beside it, and the daily log files. With a database URL it stores
//! everything in PostgreSQL instead, which several instances can serve at
//! once, each given the same server secret, and keeps only its log files in
//! the data directory.

#![forbid(unsafe_code)]

pub mod data;
pub mod http;
pub mod logs;
pub mod postgres;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sync_store::{Backend, Store};
use sync_store_sqlite::SqliteBackend;
use tokio::net::TcpListener;

pub use data::{CommandError, Migrated, check_ready, migrate};
pub use postgres::Postgres;

/// What `serve` runs with.
#[derive(Clone, Debug)]
pub struct Config {
    /// The data directory, which holds the log files, and in SQLite mode
    /// the database and the server secret.
    pub data_dir: PathBuf,
    /// The address to listen on.
    pub listen: SocketAddr,
    /// The PostgreSQL database to use instead of SQLite.
    pub postgres: Option<Postgres>,
}

/// Opens the SQLite store in a ready data directory.
pub async fn open_store(config: &Config) -> Result<Store<SqliteBackend>, CommandError> {
    check_ready(&config.data_dir).await?;
    let backend = SqliteBackend::open(&data::database_path(&config.data_dir))
        .await
        .map_err(|e| CommandError(format!("can't open the database: {e}")))?;
    Store::open(backend)
        .await
        .map_err(|e| CommandError(format!("can't open the store: {e}")))
}

/// Serves `store` on `listener` until `shutdown` resolves.
pub async fn serve_on<B: Backend>(
    store: Arc<Store<B>>,
    listener: TcpListener,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, http::router(store))
        .with_graceful_shutdown(shutdown)
        .await
}

/// Serves `store` on a new listener at `address` in the background, for
/// tests, and returns the bound address.
pub async fn spawn<B: Backend>(store: Arc<Store<B>>, address: &str) -> std::io::Result<SocketAddr> {
    let listener = TcpListener::bind(address).await?;
    let bound = listener.local_addr()?;
    tokio::spawn(serve_on(store, listener, std::future::pending()));
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

/// `sync-server serve`: refuses a database that isn't ready, then serves
/// until it gets an interrupt or a terminate signal, and closes the
/// database.
pub async fn serve(config: Config) -> Result<(), CommandError> {
    match &config.postgres {
        None => run(Arc::new(open_store(&config).await?), &config).await,
        Some(database) => run(Arc::new(postgres::open_store(database).await?), &config).await,
    }
}

async fn run<B: Backend>(store: Arc<Store<B>>, config: &Config) -> Result<(), CommandError> {
    let log_dir = config.data_dir.join(data::LOG_DIR);
    logs::init(&log_dir).map_err(|e| CommandError(format!("can't open the log: {e}")))?;
    let pruning = logs::spawn_pruning(log_dir, logs::PRUNE_PERIOD);
    let listener = TcpListener::bind(config.listen)
        .await
        .map_err(|e| CommandError(format!("can't listen on {}: {e}", config.listen)))?;
    let bound = listener
        .local_addr()
        .map_err(|e| CommandError(e.to_string()))?;
    tracing::info!("serving");
    eprintln!("sync-server listening on {bound}");
    let served = serve_on(store.clone(), listener, shutdown_signal()).await;
    pruning.abort();
    store.backend().close().await;
    tracing::info!("stopped");
    served.map_err(|e| CommandError(format!("the server failed: {e}")))
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
