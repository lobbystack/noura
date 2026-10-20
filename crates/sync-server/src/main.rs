//! The Noura sync service.
//!
//! For trial S0 the binary only proves that the server links as one static
//! Linux binary: it prints the native libraries it carries and exits. Ticket 44
//! replaces this with `serve`, `migrate` and `healthcheck`.

use std::error::Error;
use std::process::ExitCode;

// The PostgreSQL store is pure Rust and holds no code yet. Linking it now means
// adding the store later can't surprise the static build.
use sync_store_postgres as _;
use url::Url;
use webauthn_rs::WebauthnBuilder;

fn main() -> ExitCode {
    match linked_libraries() {
        Ok(libraries) => {
            println!("sync-server {} ({libraries})", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("sync-server: {error}");
            ExitCode::FAILURE
        }
    }
}

fn linked_libraries() -> Result<String, Box<dyn Error>> {
    let origin = Url::parse("https://localhost")?;
    WebauthnBuilder::new("localhost", &origin)?.build()?;
    let sqlite = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(sync_store_sqlite::sqlite_version())?;
    Ok(format!("SQLite {sqlite}, {}", openssl::version::version()))
}
