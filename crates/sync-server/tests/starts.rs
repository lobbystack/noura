//! Trial S0: the server binary starts, reports the native libraries it links,
//! and exits successfully.

use std::process::Command;

#[test]
fn starts_and_exits_successfully() {
    let output = Command::new(env!("CARGO_BIN_EXE_sync-server"))
        .output()
        .expect("binary runs");

    assert!(output.status.success(), "exit status {}", output.status);
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 output");
    assert!(stdout.starts_with("sync-server "), "{stdout}");
    assert!(stdout.contains("SQLite 3."), "{stdout}");
    assert!(stdout.contains("OpenSSL 3."), "{stdout}");
}
