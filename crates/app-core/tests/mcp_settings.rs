//! Seam 1: Settings' AI tools section against a real workspace: the MCP
//! configuration with the installed binary and the workspace's folder,
//! Copy, and Test connection's result, with a probe that stands in for
//! the server.

mod support;

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use app_core::{App, MCP_TEST_WAIT, McpProbe, McpTest, NoticeKind, settings_section};
use support::{Call, FakeSystem, Fixture};

/// A probe that answers as told and records what it was asked to start.
struct FakeProbe {
    answer: Result<String, String>,
    started: Mutex<Vec<(PathBuf, Vec<OsString>, Duration)>>,
}

impl FakeProbe {
    fn answering(answer: Result<&str, &str>) -> Arc<Self> {
        Arc::new(Self {
            answer: answer.map(str::to_owned).map_err(str::to_owned),
            started: Mutex::new(Vec::new()),
        })
    }
}

impl McpProbe for FakeProbe {
    fn test(
        &self,
        program: &Path,
        arguments: &[OsString],
        wait: Duration,
    ) -> Result<String, String> {
        self.started
            .lock()
            .unwrap()
            .push((program.to_owned(), arguments.to_vec(), wait));
        self.answer.clone()
    }
}

const PROGRAM: &str = "/Applications/Noura.app/Contents/MacOS/noura";

fn tested(app: &mut App) -> McpTest {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.poll();
        let test = app.mcp_setup().unwrap().test;
        if test != McpTest::Testing {
            return test;
        }
        assert!(Instant::now() < deadline, "the test never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_section_shows_a_configuration_with_the_binary_and_the_workspace_folder_and_copies_it() {
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    assert_eq!(app.mcp_setup(), None);

    app.set_mcp_server(Ok(PROGRAM.into()), FakeProbe::answering(Ok("noura 0.2.0")));
    app.open_settings_at(settings_section::AI_TOOLS);

    let titles: Vec<String> = app
        .settings_sections()
        .into_iter()
        .find(|(group, _)| group.title() == "Workspace")
        .unwrap()
        .1
        .into_iter()
        .map(|section| section.title.clone())
        .collect();
    assert_eq!(
        titles,
        [
            "Name and location",
            "Plugins",
            "Sync",
            "Members",
            "AI tools"
        ]
    );
    assert_eq!(app.settings().unwrap().section, settings_section::AI_TOOLS);
    let root = app.workspace().unwrap().root().to_owned();
    let configuration = app.mcp_setup().unwrap().configuration.unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&configuration).unwrap();
    assert_eq!(
        parsed,
        serde_json::json!({
            "mcpServers": {
                "noura": {
                    "command": PROGRAM,
                    "args": ["mcp", "--workspace", root],
                }
            }
        })
    );

    app.copy_mcp_configuration();

    assert!(system.calls().contains(&Call::Copy(configuration)));
    let notice = app.settings().unwrap().notice.clone().unwrap();
    assert_eq!(notice.kind, NoticeKind::Done);
    assert_eq!(notice.title, "Configuration copied");
}

#[test]
fn test_connection_starts_the_configured_command_and_says_whether_it_works() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let probe = FakeProbe::answering(Ok("noura 0.2.0"));
    app.set_mcp_server(Ok(PROGRAM.into()), probe.clone());
    app.open_settings_at(settings_section::AI_TOOLS);
    assert_eq!(app.mcp_setup().unwrap().test, McpTest::NotRun);

    app.test_mcp_connection();

    assert_eq!(tested(&mut app), McpTest::Works("noura 0.2.0".into()));
    let root = app.workspace().unwrap().root().to_owned();
    assert_eq!(
        *probe.started.lock().unwrap(),
        [(
            PathBuf::from(PROGRAM),
            vec![
                OsString::from("mcp"),
                OsString::from("--workspace"),
                OsString::from(root)
            ],
            MCP_TEST_WAIT
        )]
    );
    assert_eq!(MCP_TEST_WAIT, Duration::from_secs(10));

    // Settings opens afresh, with no result from last time.
    app.close_settings();
    app.open_settings_at(settings_section::AI_TOOLS);
    assert_eq!(app.mcp_setup().unwrap().test, McpTest::NotRun);

    let failing = FakeProbe::answering(Err("The server didn't answer within 10 seconds."));
    app.set_mcp_server(Ok(PROGRAM.into()), failing);
    app.test_mcp_connection();
    assert_eq!(
        tested(&mut app),
        McpTest::Failed("The server didn't answer within 10 seconds.".into())
    );
}

#[cfg(unix)]
#[test]
fn a_binary_path_the_configuration_cant_hold_shows_why_and_copies_nothing() {
    use std::os::unix::ffi::OsStrExt;

    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    let program = PathBuf::from(std::ffi::OsStr::from_bytes(b"/opt/No\xffura/noura"));
    app.set_mcp_server(Ok(program), FakeProbe::answering(Ok("noura 0.2.0")));
    app.open_settings_at(settings_section::AI_TOOLS);

    let error = app.mcp_setup().unwrap().configuration.unwrap_err();

    assert!(
        error.contains("characters a configuration can't hold"),
        "{error}"
    );
    app.copy_mcp_configuration();
    assert!(
        !system
            .calls()
            .iter()
            .any(|call| matches!(call, Call::Copy(_)))
    );
    assert_eq!(app.settings().unwrap().notice, None);
}

#[test]
fn a_copy_of_noura_without_a_lasting_program_shows_why_and_tests_nothing() {
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    let probe = FakeProbe::answering(Ok("noura 0.2.0"));
    let reason = "Noura runs from a temporary folder.";
    app.set_mcp_server(Err(reason.to_owned()), probe.clone());
    app.open_settings_at(settings_section::AI_TOOLS);

    assert_eq!(
        app.mcp_setup().unwrap().configuration,
        Err(reason.to_owned())
    );
    app.copy_mcp_configuration();
    app.test_mcp_connection();

    assert!(
        !system
            .calls()
            .iter()
            .any(|call| matches!(call, Call::Copy(_)))
    );
    assert_eq!(app.mcp_setup().unwrap().test, McpTest::NotRun);
    assert!(probe.started.lock().unwrap().is_empty());
}
