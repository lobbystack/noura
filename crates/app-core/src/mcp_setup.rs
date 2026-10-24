//! Settings' AI tools section: the configuration that connects an AI tool
//! outside Noura, such as Claude Desktop or Cursor, to the open workspace
//! through the desktop binary's `mcp --workspace <folder>` mode, and Test
//! connection, which starts that command as the AI tool would.

use std::{
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// How long Test connection waits for the server's answer to MCP's
/// initialize request.
pub const MCP_TEST_WAIT: Duration = Duration::from_secs(10);

/// The name the configuration gives Noura's server among the AI tool's
/// MCP servers.
const SERVER_NAME: &str = "noura";

/// Starts a command as an AI tool does and waits for the answer to MCP's
/// initialize request. The desktop app tests through `noura-mcp`.
pub trait McpProbe: Send + Sync {
    /// Start `program` with `arguments` and wait up to `wait` for the
    /// answer. Returns the name and version the server gave, or why it
    /// didn't answer, in words.
    fn test(
        &self,
        program: &Path,
        arguments: &[OsString],
        wait: Duration,
    ) -> Result<String, String>;
}

/// The command AI tools start for Noura's MCP server on this device, and
/// how Settings tests it.
#[derive(Clone)]
pub(crate) struct McpServer {
    /// The installed `noura` binary.
    pub(crate) program: PathBuf,
    pub(crate) probe: Arc<dyn McpProbe>,
}

impl fmt::Debug for McpServer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpServer")
            .field("program", &self.program)
            .finish_non_exhaustive()
    }
}

/// What the AI tools section shows for the open workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpSetup {
    /// The JSON to paste into the AI tool's MCP settings, or why Noura
    /// can't write it.
    pub configuration: Result<String, String>,
    /// Where the last Test connection stands.
    pub test: McpTest,
}

/// Where Test connection stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum McpTest {
    /// Nobody tested the connection since Settings opened.
    #[default]
    NotRun,
    /// The command runs, and Noura waits for its answer.
    Testing,
    /// The server answered: the name and version it gave.
    Works(String),
    /// Why the connection doesn't work, in words.
    Failed(String),
}

/// The arguments that start the MCP server for the workspace in `folder`.
pub(crate) fn arguments(folder: &str) -> Vec<OsString> {
    vec!["mcp".into(), "--workspace".into(), folder.into()]
}

/// The configuration that starts `program` for the workspace in `folder`,
/// in the `mcpServers` form Claude Desktop and Cursor read, or why a
/// configuration can't hold `program`'s path.
pub(crate) fn configuration(program: &Path, folder: &str) -> Result<String, String> {
    let Some(command) = program.to_str() else {
        return Err(format!(
            "Noura can't write a configuration, because the path of the folder Noura is in, \
             {}, has characters a configuration can't hold. Move Noura to a folder whose path \
             has only standard characters, then open Settings again.",
            program.display()
        ));
    };
    let arguments: Vec<String> = arguments(folder)
        .into_iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let configuration = serde_json::json!({
        "mcpServers": {
            SERVER_NAME: {
                "command": command,
                "args": arguments,
            }
        }
    });
    serde_json::to_string_pretty(&configuration).map_err(|_| {
        "Noura couldn't write the configuration. Open Settings again to retry.".to_owned()
    })
}
