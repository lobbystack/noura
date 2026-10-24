//! Settings' AI tools section: this binary is the command AI tools start,
//! in its `mcp --workspace <folder>` mode, and `noura-mcp` tests it.

use std::{ffi::OsString, path::Path, sync::Arc, time::Duration};

use app_core::{App, McpProbe};

/// Starts the configured command as an AI tool would, through
/// `noura-mcp`'s MCP client.
struct Probe;

impl McpProbe for Probe {
    fn test(
        &self,
        program: &Path,
        arguments: &[OsString],
        wait: Duration,
    ) -> Result<String, String> {
        noura_mcp::test_connection(program, arguments, wait).map(|answered| {
            if answered.version.is_empty() {
                answered.name
            } else {
                format!("{} {}", answered.name, answered.version)
            }
        })
    }
}

/// Offer AI tools this binary's MCP server in Settings. Without its own
/// path, Noura can't write a configuration, so Settings leaves the
/// section out.
pub(crate) fn offer(app: &mut App) {
    match std::env::current_exe() {
        Ok(program) => app.set_mcp_server(program, Arc::new(Probe)),
        Err(error) => tracing::warn!(%error, "Noura doesn't know its own path"),
    }
}
