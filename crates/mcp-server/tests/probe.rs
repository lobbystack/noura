//! Seam 1: Test connection starts a configured command as an AI tool
//! would and waits for the answer to MCP's initialize request, here
//! against the `noura-mcp` binary on a real workspace folder. `env` sets
//! the server's home folder, so its app data lands in the test's own
//! folder.

#![cfg(unix)]

mod support;

use std::{
    ffi::OsString,
    path::Path,
    time::{Duration, Instant},
};

use noura_mcp::test_connection;
use support::Fixture;

/// `noura-mcp --workspace <folder>`, with `home` as its home folder.
fn command(home: &Path, folder: &Path) -> Vec<OsString> {
    vec![
        format!("HOME={}", home.display()).into(),
        format!("XDG_DATA_HOME={}", home.join("data").display()).into(),
        env!("CARGO_BIN_EXE_noura-mcp").into(),
        "--workspace".into(),
        folder.as_os_str().to_owned(),
    ]
}

#[test]
fn a_server_that_answers_works() {
    let fixture = Fixture::new(&[]);
    let home = tempfile::tempdir().unwrap();

    let answered = test_connection(
        Path::new("/usr/bin/env"),
        &command(home.path(), &fixture.root),
        Duration::from_secs(10),
    )
    .unwrap();

    assert_eq!(answered.name, "noura");
    assert_eq!(answered.version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn a_server_that_stops_says_why() {
    let fixture = Fixture::new(&[]);
    let home = tempfile::tempdir().unwrap();
    let plain = tempfile::tempdir().unwrap();

    let error = test_connection(
        Path::new("/usr/bin/env"),
        &command(home.path(), plain.path()),
        Duration::from_secs(10),
    )
    .unwrap_err();

    assert!(
        error.starts_with("The server stopped before it answered: "),
        "{error}"
    );
    assert!(error.contains(".noura/workspace.yaml"), "{error}");
    assert!(!plain.path().join(".noura").exists());
    drop(fixture);
}

#[test]
fn a_command_that_never_answers_fails_once_the_wait_ends() {
    let started = Instant::now();

    let error = test_connection(
        Path::new("/bin/sleep"),
        &["30".into()],
        Duration::from_secs(2),
    )
    .unwrap_err();

    assert_eq!(error, "The server didn't answer within 2 seconds.");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_command_that_isnt_there_says_noura_couldnt_start_it() {
    let missing = Path::new("/nonexistent/noura");

    let error = test_connection(missing, &[], Duration::from_secs(2)).unwrap_err();

    assert_eq!(error, "Noura couldn't start /nonexistent/noura.");
}
