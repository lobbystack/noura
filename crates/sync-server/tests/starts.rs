//! `sync-server version` reports the native libraries the binary links:
//! the SQLite that sqlx bundles and the vendored OpenSSL.

use std::process::Command;

#[test]
fn version_reports_the_linked_libraries() {
    let output = Command::new(env!("CARGO_BIN_EXE_sync-server"))
        .arg("version")
        .output()
        .expect("binary runs");

    assert!(output.status.success(), "exit status {}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 output");
    assert!(stdout.starts_with("sync-server "), "{stdout}");
    assert!(stdout.contains("SQLite 3."), "{stdout}");
    assert!(stdout.contains("OpenSSL 3."), "{stdout}");
}

#[test]
fn an_unknown_command_prints_the_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_sync-server"))
        .arg("launch")
        .output()
        .expect("binary runs");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 output");
    assert!(stderr.contains("usage: sync-server"), "{stderr}");
}
