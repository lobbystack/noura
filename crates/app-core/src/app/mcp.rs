//! Settings' AI tools section: the MCP configuration for the open
//! workspace, its Copy button and Test connection.

use std::{
    path::PathBuf,
    sync::{Arc, mpsc::TryRecvError},
};

use super::{App, Change};
use crate::{
    NoticeKind, SettingsGroup,
    mcp_setup::{self, MCP_TEST_WAIT, McpProbe, McpServer, McpSetup, McpTest},
    settings::settings_section,
};

impl App {
    /// Offer AI tools outside Noura the MCP server that `program`, the
    /// installed `noura` binary, serves in its `mcp` mode, and test it
    /// through `probe`. This adds the AI tools section to Settings.
    pub fn set_mcp_server(&mut self, program: PathBuf, probe: Arc<dyn McpProbe>) {
        self.mcp = Some(McpServer { program, probe });
        self.register_settings_section(
            settings_section::AI_TOOLS,
            "AI tools",
            SettingsGroup::Workspace,
        );
    }

    /// What the AI tools section shows: the configuration for the open
    /// workspace and where Test connection stands. `None` until a client
    /// offers the MCP server, and while no workspace is open.
    pub fn mcp_setup(&self) -> Option<McpSetup> {
        let server = self.mcp.as_ref()?;
        let workspace = self.workspace.as_ref()?;
        Some(McpSetup {
            configuration: mcp_setup::configuration(&server.program, workspace.root()),
            test: self.mcp_test.clone(),
        })
    }

    /// Put the configuration on the clipboard, and say so in Settings.
    pub fn copy_mcp_configuration(&mut self) {
        let Some(McpSetup {
            configuration: Ok(configuration),
            ..
        }) = self.mcp_setup()
        else {
            return;
        };
        self.system.copy_text(&configuration);
        self.settings_notice(
            "Configuration copied",
            "Paste it into your AI tool's MCP settings, then restart the tool.",
            NoticeKind::Done,
        );
    }

    /// Start the configured command as an AI tool would, off the window's
    /// thread, and wait up to [`MCP_TEST_WAIT`] for its answer, which
    /// [`App::poll`] takes. A test that runs already keeps running.
    pub fn test_mcp_connection(&mut self) {
        if self.mcp_test == McpTest::Testing {
            return;
        }
        let (Some(server), Some(workspace)) = (self.mcp.clone(), self.workspace.clone()) else {
            return;
        };
        let (sender, answer) = std::sync::mpsc::channel();
        let listeners = self.listeners.clone();
        let arguments = mcp_setup::arguments(workspace.root());
        let started = std::thread::Builder::new()
            .name("mcp-test".into())
            .spawn(move || {
                let result = server
                    .probe
                    .test(&server.program, &arguments, MCP_TEST_WAIT);
                let _ = sender.send(result);
                listeners.emit(Change::Settings);
            });
        self.mcp_test = match started {
            Ok(_) => {
                self.mcp_answer = Some(answer);
                McpTest::Testing
            }
            Err(error) => {
                tracing::warn!(%error, "the connection test didn't start");
                McpTest::Failed("Noura couldn't run the test. Try again.".to_owned())
            }
        };
        self.listeners.emit(Change::Settings);
    }

    /// Take Test connection's answer once it arrived.
    pub(super) fn poll_mcp_test(&mut self) -> bool {
        let Some(answer) = &self.mcp_answer else {
            return false;
        };
        let result = match answer.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                Err("The test stopped before it finished. Try again.".to_owned())
            }
        };
        self.mcp_answer = None;
        self.mcp_test = match result {
            Ok(server) => McpTest::Works(server),
            Err(reason) => McpTest::Failed(reason),
        };
        self.listeners.emit(Change::Settings);
        true
    }
}
