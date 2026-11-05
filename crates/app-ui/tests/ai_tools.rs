//! Settings' AI tools section through AccessKit: the MCP configuration
//! with the installed binary and the workspace's folder, Copy, the
//! warning, Test connection's result, and a binary path the
//! configuration can't hold.

mod support;

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use app_core::{McpProbe, Platform};
use egui::accesskit::Role;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, FakeSystem, create_workspace, harness_on, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

const PROGRAM: &str = "/Applications/Noura.app/Contents/MacOS/noura";

/// A probe that answers as told, after a moment.
struct FakeProbe(Result<String, String>);

impl McpProbe for FakeProbe {
    fn test(&self, _: &Path, _: &[OsString], _: Duration) -> Result<String, String> {
        std::thread::sleep(Duration::from_millis(50));
        self.0.clone()
    }
}

struct Opened {
    harness: Ui,
    system: Arc<FakeSystem>,
    root: String,
    _folders: (tempfile::TempDir, tempfile::TempDir),
}

/// A workspace open with the AI tools section showing, the MCP server at
/// `program` and a probe that answers `answer`.
fn opened(program: PathBuf, answer: Result<&str, &str>) -> Opened {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    let system = FakeSystem::new(Platform::MacOs);
    let probe = Arc::new(FakeProbe(answer.map(str::to_owned).map_err(str::to_owned)));
    let mut harness = harness_on(
        app_data.path(),
        FakePicker::default(),
        system.clone(),
        move |app| app.set_mcp_server(Ok(program), probe),
    );
    settle(&mut harness);
    let root = harness.state().app().workspace().unwrap().root().to_owned();
    harness
        .get_by_role_and_label(Role::Button, "Settings")
        .click();
    harness.run();
    harness
        .get_by_role_and_label(Role::Navigation, "Settings sections")
        .get_by_role_and_label(Role::Button, "AI tools")
        .click();
    harness.run();
    Opened {
        harness,
        system,
        root,
        _folders: (app_data, parent),
    }
}

/// Run frames until a node labelled `label` shows.
fn wait_for_label(harness: &mut Ui, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while harness.query_by_label(label).is_none() {
        assert!(Instant::now() < deadline, "{label} never showed");
        std::thread::sleep(Duration::from_millis(10));
        harness.step();
    }
    harness.run();
}

#[test]
fn the_section_shows_the_configuration_a_copy_button_and_a_warning() {
    let Opened {
        mut harness,
        system,
        root,
        _folders,
    } = opened(PROGRAM.into(), Ok("noura 0.2.0"));

    let configuration = harness
        .get_by_role_and_label(Role::Group, "Configuration")
        .get_by_label_contains("mcpServers")
        .accesskit_node()
        .value()
        .unwrap();
    assert!(configuration.contains(PROGRAM), "{configuration}");
    // The configuration is JSON, which doubles a Windows path's backslashes.
    let escaped = root.replace('\\', "\\\\");
    assert!(configuration.contains(&escaped), "{configuration}");
    assert!(configuration.contains("\"--workspace\""), "{configuration}");
    harness.get_by_label_contains("without asking you first");

    harness
        .get_by_role_and_label(Role::Group, "Configuration")
        .get_by_role_and_label(Role::Button, "Copy")
        .click();
    harness.run();

    let calls = system.calls();
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("copy ") && call.contains(&escaped)),
        "{calls:?}"
    );
    harness.get_by_role(Role::Status);
    harness.get_by_label("Configuration copied");
}

#[test]
fn test_connection_says_when_the_server_answers() {
    let Opened {
        mut harness,
        _folders,
        ..
    } = opened(PROGRAM.into(), Ok("noura 0.2.0"));

    harness
        .get_by_role_and_label(Role::Button, "Test connection")
        .click_accesskit();
    harness.run();
    wait_for_label(&mut harness, "It works. noura 0.2.0 answered.");

    harness
        .get_by_role(Role::Status)
        .get_by_label("It works. noura 0.2.0 answered.");
}

#[test]
fn test_connection_says_why_it_doesnt_work() {
    let Opened {
        mut harness,
        _folders,
        ..
    } = opened(
        PROGRAM.into(),
        Err("The server didn't answer within 10 seconds."),
    );

    harness
        .get_by_role_and_label(Role::Button, "Test connection")
        .click_accesskit();
    harness.run();
    let message = "It doesn't work. The server didn't answer within 10 seconds.";
    wait_for_label(&mut harness, message);

    harness.get_by_role(Role::Alert).get_by_label(message);
}

#[cfg(unix)]
#[test]
fn a_binary_path_the_configuration_cant_hold_shows_a_clear_error() {
    use std::os::unix::ffi::OsStrExt;

    let program = PathBuf::from(std::ffi::OsStr::from_bytes(b"/opt/No\xffura/noura"));
    let Opened {
        harness, _folders, ..
    } = opened(program, Ok("noura 0.2.0"));

    let group = harness.get_by_role_and_label(Role::Group, "Configuration");
    let problem = group.get_by_role(Role::Alert);
    let text = problem
        .get_by_label_contains("characters a configuration can't hold")
        .accesskit_node()
        .value()
        .unwrap();
    assert!(text.contains("Move Noura to a folder"), "{text}");
    assert!(
        group
            .query_by_role_and_label(Role::Button, "Copy")
            .is_none()
    );
    assert!(
        harness
            .get_by_role_and_label(Role::Button, "Test connection")
            .accesskit_node()
            .is_disabled()
    );
}
