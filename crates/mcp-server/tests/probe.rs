//! Seam 1: Test connection starts a configured command as an AI tool
//! would and waits for the answer to MCP's initialize request, here
//! against the `noura-mcp` binary on a real workspace folder, with its
//! app data in the test's own folder.

mod support;

use std::{
    ffi::OsString,
    path::Path,
    time::{Duration, Instant},
};

use noura_mcp::test_connection;
use support::Fixture;

const SERVER: &str = env!("CARGO_BIN_EXE_noura-mcp");

/// `noura-mcp`'s arguments for `folder`, with `app_data` as its app data.
fn arguments(app_data: &Path, folder: &Path) -> Vec<OsString> {
    vec![
        "--workspace".into(),
        folder.as_os_str().to_owned(),
        "--app-data".into(),
        app_data.as_os_str().to_owned(),
    ]
}

/// A program that runs for 30 seconds and says nothing.
fn silent_command() -> (&'static Path, Vec<OsString>) {
    if cfg!(windows) {
        let sleep = "Start-Sleep -Seconds 30";
        let arguments = ["-NoProfile", "-Command", sleep].map(OsString::from);
        (Path::new("powershell.exe"), arguments.to_vec())
    } else {
        (Path::new("/bin/sleep"), vec!["30".into()])
    }
}

#[test]
fn a_server_that_answers_works() {
    let fixture = Fixture::new(&[]);
    let app_data = tempfile::tempdir().unwrap();

    let answered = test_connection(
        Path::new(SERVER),
        &arguments(app_data.path(), &fixture.root),
        Duration::from_secs(10),
    )
    .unwrap();

    assert_eq!(answered.name, "noura");
    assert_eq!(answered.version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn a_server_that_stops_says_why() {
    let fixture = Fixture::new(&[]);
    let app_data = tempfile::tempdir().unwrap();
    let plain = tempfile::tempdir().unwrap();

    let error = test_connection(
        Path::new(SERVER),
        &arguments(app_data.path(), plain.path()),
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
    let (program, arguments) = silent_command();
    let started = Instant::now();

    let error = test_connection(program, &arguments, Duration::from_secs(2)).unwrap_err();

    assert_eq!(error, "The server didn't answer within 2 seconds.");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_command_that_isnt_there_says_noura_couldnt_start_it() {
    let missing = Path::new("/nonexistent/noura");

    let error = test_connection(missing, &[], Duration::from_secs(2)).unwrap_err();

    assert_eq!(error, "Noura couldn't start /nonexistent/noura.");
}
