//! Test connection: start the command an AI tool's configuration names,
//! as the tool would, and wait for the answer to MCP's initialize request.
//! rmcp's client does the handshake, so no JSON-RPC framing is written
//! here.

use std::{
    ffi::OsString,
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use rmcp::{ServiceExt, transport::TokioChildProcess};
use tokio::io::AsyncReadExt;

/// How much of what the server writes to stderr the test keeps: the end,
/// whose last line says why a server stopped.
const STDERR_KEPT: usize = 4096;

/// How long the test gives a server that stopped to finish writing why.
const STDERR_WAIT: Duration = Duration::from_millis(500);

/// The server that answered, as it named itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    pub name: String,
    pub version: String,
}

/// Start `program` with `arguments`, as an AI tool does, send MCP's
/// initialize request and wait up to `timeout`, counted from the start,
/// for the reply. The server stops once the test ends. Returns why the
/// connection doesn't work, in words, otherwise.
pub fn test_connection(
    program: &Path,
    arguments: &[OsString],
    timeout: Duration,
) -> Result<Answered, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|error| {
            tracing::warn!(%error, "the connection test couldn't start");
            "Noura couldn't run the test. Try again.".to_owned()
        })?;
    runtime.block_on(connect(program, arguments, timeout))
}

async fn connect(
    program: &Path,
    arguments: &[OsString],
    timeout: Duration,
) -> Result<Answered, String> {
    let mut command = tokio::process::Command::new(program);
    command.args(arguments).kill_on_drop(true);
    let (transport, stderr) = TokioChildProcess::builder(command)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            tracing::warn!(%error, "the configured command didn't start");
            format!("Noura couldn't start {}.", program.display())
        })?;
    let said = Arc::new(Mutex::new(Vec::new()));
    let reader = stderr.map(|stderr| tokio::spawn(keep_end(stderr, said.clone())));
    let answer = match tokio::time::timeout(timeout, ().serve(transport)).await {
        Err(_) => Err(format!(
            "The server didn't answer within {} seconds.",
            timeout.as_secs()
        )),
        Ok(Err(error)) => {
            tracing::warn!(%error, "the configured command didn't answer");
            if let Some(reader) = reader {
                let _ = tokio::time::timeout(STDERR_WAIT, reader).await;
            }
            Err(match last_line(&said) {
                Some(line) => format!("The server stopped before it answered: {line}"),
                None => "The server stopped before it answered.".to_owned(),
            })
        }
        Ok(Ok(client)) => {
            let implementation = client.peer_info().and_then(|info| info.server_info.clone());
            let _ = client.cancel().await;
            Ok(match implementation {
                Some(implementation) => Answered {
                    name: implementation.name,
                    version: implementation.version,
                },
                None => Answered {
                    name: "An MCP server".to_owned(),
                    version: String::new(),
                },
            })
        }
    };
    // The transport stops the server as it goes, on a task of its own.
    tokio::time::sleep(Duration::from_millis(50)).await;
    answer
}

/// Read `stderr` to its end, keeping the last [`STDERR_KEPT`] bytes.
async fn keep_end(mut stderr: tokio::process::ChildStderr, said: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0u8; 1024];
    while let Ok(read) = stderr.read(&mut chunk).await {
        if read == 0 {
            return;
        }
        let mut said = said.lock().unwrap_or_else(|error| error.into_inner());
        said.extend_from_slice(&chunk[..read]);
        let extra = said.len().saturating_sub(STDERR_KEPT);
        said.drain(..extra);
    }
}

/// The last line the server wrote to stderr, if it wrote one.
fn last_line(said: &Mutex<Vec<u8>>) -> Option<String> {
    let said = said.lock().unwrap_or_else(|error| error.into_inner());
    String::from_utf8_lossy(&said)
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(str::to_owned)
}
