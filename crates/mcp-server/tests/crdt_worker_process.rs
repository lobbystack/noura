use std::{io::Write, process::Stdio};

#[test]
fn host_executable_runs_the_bounded_crdt_worker_before_mcp_startup() {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_noura-mcp"))
        .arg("--noura-crdt-worker")
        .env_clear()
        .env("NOURA_TEST_ALLOW_INHERITED_SANDBOX", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(r#"{"operation":"fresh","text":"isolated 😀"}"#.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    #[cfg(target_os = "windows")]
    {
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    #[cfg(not(target_os = "windows"))]
    {
        assert!(output.status.success());
        let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["response"]["result"], "snapshot");
        assert_eq!(reply["response"]["snapshot"]["text"], "isolated 😀");
        assert!(reply["errorCode"].is_null());
    }
}
