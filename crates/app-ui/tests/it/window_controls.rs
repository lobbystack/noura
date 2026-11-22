//! The window's own buttons in the title bar row, for a window with no
//! frame, as on Linux, through AccessKit; and the workspace switcher lined
//! up with the sidebar's rows.

use crate::support;
use app_core::Platform;
use app_ui::{WindowControl, WindowControls};
use egui::{ViewportCommand, ViewportId, accesskit::Role};
use egui_kittest::kittest::Queryable;
use support::{create_workspace, harness_edge_to_edge, settle};

type Ui = egui_kittest::Harness<'static, app_ui::NouraUi>;

/// A Linux window on a workspace, its buttons laid out as `controls`.
fn opened(controls: Option<WindowControls>) -> (Ui, tempfile::TempDir, tempfile::TempDir) {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), &parent.path().join("Garden"));
    let mut harness = harness_edge_to_edge(app_data.path(), Platform::Linux);
    harness.state_mut().set_window_controls(controls);
    settle(&mut harness);
    (harness, app_data, parent)
}

/// What the window was told in the last frame.
fn commands(harness: &Ui) -> Vec<ViewportCommand> {
    harness
        .output()
        .viewport_output
        .get(&ViewportId::ROOT)
        .map(|output| output.commands.clone())
        .unwrap_or_default()
}

fn press(harness: &mut Ui, name: &str) -> Vec<ViewportCommand> {
    harness.get_by_role_and_label(Role::Button, name).click();
    harness.step();
    commands(harness)
}

#[test]
fn each_button_is_named_and_does_what_it_says() {
    let (mut harness, _app_data, _parent) = opened(Some(WindowControls {
        start: vec![],
        end: vec![
            WindowControl::Minimize,
            WindowControl::Maximize,
            WindowControl::Close,
        ],
    }));

    assert!(press(&mut harness, "Minimize").contains(&ViewportCommand::Minimized(true)));
    assert!(press(&mut harness, "Maximize").contains(&ViewportCommand::Maximized(true)));
    assert!(press(&mut harness, "Close").contains(&ViewportCommand::Close));

    // The buttons sit at the end of the title bar row, in the layout's
    // order, clear of the tabs.
    let rect = |name| harness.get_by_role_and_label(Role::Button, name).rect();
    let (minimize, maximize, close) = (rect("Minimize"), rect("Maximize"), rect("Close"));
    assert!(minimize.right() < maximize.left() && maximize.right() < close.left());
    let window = harness.ctx.viewport_rect();
    assert!(
        window.right() - close.right() < 10.0,
        "{close:?} in {window:?}"
    );
    assert!(close.top() < 36.0);
}

#[test]
fn a_layout_without_maximize_shows_none_and_the_system_frame_shows_none_at_all() {
    let (harness, _app_data, _parent) = opened(Some(WindowControls {
        start: vec![],
        end: vec![WindowControl::Minimize, WindowControl::Close],
    }));
    harness.get_by_role_and_label(Role::Button, "Minimize");
    harness.get_by_role_and_label(Role::Button, "Close");
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "Maximize")
            .is_none()
    );

    let (harness, _app_data, _parent) = opened(None);
    for name in ["Minimize", "Maximize", "Close"] {
        assert!(
            harness
                .query_by_role_and_label(Role::Button, name)
                .is_none()
        );
    }
}

/// Buttons at the start of the row push the workspace switcher past them.
#[test]
fn buttons_at_the_start_leave_the_switcher_room() {
    let (harness, _app_data, _parent) = opened(Some(WindowControls {
        start: vec![
            WindowControl::Close,
            WindowControl::Minimize,
            WindowControl::Maximize,
        ],
        end: vec![],
    }));
    let maximize = harness
        .get_by_role_and_label(Role::Button, "Maximize")
        .rect();
    let switcher = harness
        .get_by_role_and_label(Role::Button, "Workspace: Garden")
        .rect();
    assert!(
        maximize.right() < switcher.left(),
        "{maximize:?} {switcher:?}"
    );
}

/// The switcher's folder icon lines up with the Files heading's caret and
/// the tree's icons under it, and sits in the middle of the row.
#[test]
fn the_switcher_lines_up_with_the_sidebar_s_rows() {
    let (harness, _app_data, _parent) = opened(Some(WindowControls {
        start: vec![],
        end: vec![WindowControl::Close],
    }));
    let switcher = harness
        .get_by_role_and_label(Role::Button, "Workspace: Garden")
        .rect();
    let files = harness.get_by_role_and_label(Role::Button, "Files").rect();
    assert_eq!(switcher.left(), files.left());
    let row = 36.0;
    let top = harness.ctx.viewport_rect().top();
    assert_eq!(switcher.center().y - top, row / 2.0);
}
