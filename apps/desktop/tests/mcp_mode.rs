//! Seam 1: the `noura` binary in `mcp --workspace` mode, as an AI tool
//! starts it, while the app has the workspace open and holds the
//! single-instance lock. It answers MCP on stdout, writes nothing else
//! there, and exits once the tool closes stdin, which a window would
//! outlive.
//!
//! The binary finds its app data through the home folder, which the test
//! points at a folder of its own, so this runs on macOS and Linux.

#![cfg(unix)]

use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use app_core::{App, WorkspaceService};
use serde_json::{Value, json};

const PATIENCE: Duration = Duration::from_secs(30);

#[test]
fn mcp_mode_answers_on_stdout_alone_while_the_app_has_the_workspace_open() {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: this test binary runs this one test, before any other thread
    // reads the environment.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("XDG_DATA_HOME", home.path().join("data"));
    }
    let app_data = app_core::default_app_data_dir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    std::fs::create_dir_all(&folder).unwrap();
    let workspace = WorkspaceService::new(&app_data)
        .create("Garden", &folder)
        .unwrap();
    let root = std::path::PathBuf::from(workspace.root());
    drop(workspace);
    std::fs::write(root.join("Plan.md"), "Plant tomatoes.\n").unwrap();

    // The app runs: it has the workspace open and holds the lock a second
    // launch of the app would stop at.
    let mut app = App::launch(&app_data);
    let deadline = Instant::now() + PATIENCE;
    while app.workspace().is_none() || app.is_busy() {
        app.poll();
        assert!(
            Instant::now() < deadline,
            "the app didn't open the workspace"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(app_data.join("instance.lock"))
        .unwrap();
    lock.lock().unwrap();

    let mut server = Command::new(env!("CARGO_BIN_EXE_noura"))
        .arg("mcp")
        .arg("--workspace")
        .arg(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = server.stdin.take().unwrap();
    let (lines, received) = mpsc::channel();
    let stdout = server.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let mut every_line = Vec::new();
    let reply = |id: u64, every_line: &mut Vec<String>| -> Value {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = received
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no reply to request {id}"));
            every_line.push(line.clone());
            let message: Value = serde_json::from_str(&line)
                .unwrap_or_else(|_| panic!("stdout carried something other than MCP: {line}"));
            if message["id"] == id {
                return message;
            }
        }
    };

    let send = |stdin: &mut std::process::ChildStdin, message: Value| {
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    };
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" },
            },
        }),
    );
    let initialized = reply(1, &mut every_line);
    assert_eq!(initialized["result"]["serverInfo"]["name"], "noura");
    send(
        &mut stdin,
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    );
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "workspace.search", "arguments": { "query": "tomatoes" } },
        }),
    );
    let searched = reply(2, &mut every_line);
    let hits: Value =
        serde_json::from_str(searched["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(hits[0]["path"], "Plan.md");

    // The AI tool goes away: the server exits, with no window holding it.
    drop(stdin);
    let deadline = Instant::now() + PATIENCE;
    let status = loop {
        if let Some(status) = server.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the server didn't exit");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status}");
    reader.join().unwrap();
    every_line.extend(received.try_iter());
    for line in &every_line {
        let message: Value = serde_json::from_str(line)
            .unwrap_or_else(|_| panic!("stdout carried something other than MCP: {line}"));
        assert_eq!(message["jsonrpc"], "2.0", "{line}");
    }
    drop(lock);
    app.close_window();
}
