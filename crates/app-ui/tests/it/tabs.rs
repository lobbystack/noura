//! The tab bar, through AccessKit.

use crate::support;
use egui::{Event, Key, Modifiers, PointerButton, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, create_workspace, harness_registering, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

/// A workspace with three notes, opened at launch with two test pages.
fn opened(app_data: &std::path::Path, folder: &std::path::Path) -> Ui {
    create_workspace(app_data, folder);
    for name in ["Alpha.md", "Beta.md", "Gamma.md"] {
        std::fs::write(folder.join(name), b"x").unwrap();
    }
    let mut harness = harness_registering(app_data, FakePicker::default(), |app| {
        app.register_page("test-inbox", "Test inbox");
    });
    settle(&mut harness);
    harness
}

fn row<'a>(harness: &'a Ui, name: &'a str) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::TreeItem, name)
}

/// Each tab's label, left to right.
fn tabs(harness: &Ui) -> Vec<String> {
    match harness.query_by_role_and_label(Role::TabList, "Tabs") {
        Some(list) => list
            .get_all_by_role(Role::Tab)
            .filter_map(|tab| tab.accesskit_node().label())
            .collect(),
        None => Vec::new(),
    }
}

/// The selected tab's label.
fn selected(harness: &Ui) -> Option<String> {
    harness
        .get_all_by_role(Role::Tab)
        .find(|tab| tab.accesskit_node().is_selected() == Some(true))
        .and_then(|tab| tab.accesskit_node().label())
}

/// Double-click at `pos`. Both clicks land in one frame: the harness
/// steps a quarter second per event, slower than a double-click.
fn double_click_at(harness: &mut Ui, pos: egui::Pos2) {
    let button = |pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    harness.input_mut().events.extend([
        Event::PointerMoved(pos),
        button(true),
        button(false),
        button(true),
        button(false),
    ]);
    harness.step();
    harness.run();
}

/// Double-click the tree row `name`.
fn double_click(harness: &mut Ui, name: &str) {
    let pos = row(harness, name).rect().center();
    double_click_at(harness, pos);
}

#[test]
fn opening_a_file_opens_a_selected_tab_with_a_close_button() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    assert!(tabs(&harness).is_empty());

    row(&harness, "Alpha").click();
    harness.run();

    assert_eq!(tabs(&harness), ["Alpha (preview)"]);
    assert_eq!(selected(&harness).as_deref(), Some("Alpha (preview)"));
    harness.get_by_role_and_label(Role::Button, "Close Alpha");
    assert!(harness.unnamed_widgets().is_empty());
}

#[test]
fn a_click_replaces_the_preview_and_a_double_click_keeps_it() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));

    row(&harness, "Alpha").click();
    harness.run();
    row(&harness, "Beta").click();
    harness.run();
    assert_eq!(tabs(&harness), ["Beta (preview)"]);

    double_click(&mut harness, "Beta");
    row(&harness, "Gamma").click();
    harness.run();

    assert_eq!(tabs(&harness), ["Beta", "Gamma (preview)"]);
}

#[test]
fn a_double_click_on_a_tab_keeps_it() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    row(&harness, "Alpha").click();
    harness.run();

    let tab = harness
        .get_by_role_and_label(Role::Tab, "Alpha (preview)")
        .rect()
        .left_center()
        + egui::vec2(12.0, 0.0);
    double_click_at(&mut harness, tab);

    assert_eq!(tabs(&harness), ["Alpha"]);
}

#[test]
fn the_close_button_and_a_middle_click_close_tabs() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    for name in ["Alpha", "Beta", "Gamma"] {
        double_click(&mut harness, name);
    }
    harness.get_by_role_and_label(Role::Tab, "Beta").click();
    harness.run();

    harness
        .get_by_role_and_label(Role::Button, "Close Beta")
        .click();
    harness.run();
    assert_eq!(tabs(&harness), ["Alpha", "Gamma"]);
    assert_eq!(selected(&harness).as_deref(), Some("Gamma"));

    harness
        .get_by_role_and_label(Role::Tab, "Alpha")
        .click_button(PointerButton::Middle);
    harness.run();
    assert_eq!(tabs(&harness), ["Gamma"]);
    assert_eq!(selected(&harness).as_deref(), Some("Gamma"));
}

#[test]
fn closing_the_last_tab_leaves_nothing_open_or_highlighted() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    row(&harness, "Alpha").click();
    harness.run();

    harness
        .get_by_role_and_label(Role::Button, "Close Alpha")
        .click();
    harness.run();

    assert!(tabs(&harness).is_empty());
    assert_eq!(row(&harness, "Alpha").accesskit_node().is_selected(), None);
    let document = harness.get_by_role_and_label(Role::Region, "Document");
    assert!(document.query_by_label("Alpha").is_none());
}

/// The tab with keyboard focus.
fn focused_tab(harness: &Ui) -> Option<String> {
    harness
        .get_all_by_role(Role::Tab)
        .find(|tab| tab.is_focused())
        .and_then(|tab| tab.accesskit_node().label())
}

#[test]
fn the_tab_key_reaches_the_selected_tab_from_the_tree() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    for name in ["Alpha", "Beta", "Gamma"] {
        double_click(&mut harness, name);
    }
    harness.get_by_role_and_label(Role::Tab, "Beta").click();
    harness.run();
    row(&harness, "Alpha").focus();
    harness.run();

    let mut presses = 0;
    while focused_tab(&harness).is_none() {
        assert!(presses < 5, "Tab never reached the tab bar");
        harness.key_press(Key::Tab);
        harness.run();
        presses += 1;
    }

    assert_eq!(focused_tab(&harness).as_deref(), Some("Beta"));
}

#[test]
fn the_keyboard_moves_between_tabs_activates_and_closes_them() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    for name in ["Alpha", "Beta", "Gamma"] {
        double_click(&mut harness, name);
    }
    harness.get_by_role_and_label(Role::Tab, "Gamma").focus();
    harness.run();

    harness.key_press(Key::ArrowRight);
    harness.run();
    assert_eq!(focused_tab(&harness).as_deref(), Some("Alpha"));
    harness.key_press(Key::End);
    harness.run();
    assert_eq!(focused_tab(&harness).as_deref(), Some("Gamma"));
    harness.key_press(Key::ArrowLeft);
    harness.run();
    assert_eq!(focused_tab(&harness).as_deref(), Some("Beta"));
    assert_eq!(selected(&harness).as_deref(), Some("Gamma"));

    harness.key_press(Key::Enter);
    harness.run();
    assert_eq!(selected(&harness).as_deref(), Some("Beta"));

    harness.key_press(Key::Delete);
    harness.run();
    assert_eq!(tabs(&harness), ["Alpha", "Gamma"]);
    assert_eq!(focused_tab(&harness).as_deref(), Some("Gamma"));
}

/// Whether the last frame asked to move the window, as a drag on the title
/// bar does.
fn moves_window(harness: &Ui) -> bool {
    harness.output().viewport_output.values().any(|viewport| {
        viewport
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::StartDrag))
    })
}

/// Drag from `from` to `to` in steps, as a mouse does, and run
/// `before_drop` before letting go. Returns whether any frame asked to
/// move the window.
fn drag(
    harness: &mut Ui,
    from: egui::Pos2,
    to: egui::Pos2,
    before_drop: impl FnOnce(&mut Ui),
) -> bool {
    let mut moved_window = false;
    harness.hover_at(from);
    harness.step();
    harness.drag_at(from);
    harness.step();
    moved_window |= moves_window(harness);
    for step in 1..=4 {
        harness.hover_at(from + (to - from) * (step as f32 / 4.0));
        harness.step();
        moved_window |= moves_window(harness);
    }
    before_drop(harness);
    harness.drop_at(to);
    harness.step();
    moved_window |= moves_window(harness);
    harness.run();
    moved_window
}

/// The tab row is the window's title bar. A tab drags to a new place
/// without moving the window; the row's empty part and the strip above
/// the sidebar still move it, and a click and the close button still work.
#[test]
fn dragging_a_tab_reorders_the_tabs_and_leaves_the_window_in_place() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    // Wide enough for the row to end in an empty part.
    harness.set_size(egui::vec2(1300.0, 700.0));
    for name in ["Alpha", "Beta", "Gamma"] {
        double_click(&mut harness, name);
    }
    // A point in the first quarter of the tab `name`, and one in its
    // last quarter.
    let near_start = |harness: &Ui, name: &str| {
        let rect = harness.get_by_role_and_label(Role::Tab, name).rect();
        rect.left_center() + egui::vec2(rect.width() / 4.0, 0.0)
    };
    let near_end = |harness: &Ui, name: &str| {
        let rect = harness.get_by_role_and_label(Role::Tab, name).rect();
        rect.right_center() - egui::vec2(rect.width() / 4.0, 0.0)
    };

    let (from, to) = (near_start(&harness, "Alpha"), near_end(&harness, "Gamma"));
    let moved_window = drag(&mut harness, from, to, |_| {});
    assert!(!moved_window, "dragging a tab moved the window");
    assert_eq!(tabs(&harness), ["Beta", "Gamma", "Alpha"]);
    assert_eq!(selected(&harness).as_deref(), Some("Gamma"));

    let (from, to) = (near_start(&harness, "Alpha"), near_start(&harness, "Beta"));
    let moved_window = drag(&mut harness, from, to, |_| {});
    assert!(!moved_window, "dragging a tab moved the window");
    assert_eq!(tabs(&harness), ["Alpha", "Beta", "Gamma"]);

    // Escape cancels a drag.
    let (from, to) = (near_start(&harness, "Alpha"), near_end(&harness, "Gamma"));
    drag(&mut harness, from, to, |harness| {
        harness.key_press(Key::Escape);
        harness.step();
    });
    assert_eq!(tabs(&harness), ["Alpha", "Beta", "Gamma"]);

    // The empty row after the last tab takes a dropped tab, and dragging
    // it moves the window.
    let gamma = harness.get_by_role_and_label(Role::Tab, "Gamma").rect();
    let empty = egui::pos2(gamma.right() + 80.0, gamma.center().y);
    let from = near_start(&harness, "Beta");
    drag(&mut harness, from, empty, |_| {});
    assert_eq!(tabs(&harness), ["Alpha", "Gamma", "Beta"]);
    let moved_window = drag(&mut harness, empty, empty + egui::vec2(60.0, 0.0), |_| {});
    assert!(
        moved_window,
        "the row's empty part no longer moves the window"
    );
    assert_eq!(tabs(&harness), ["Alpha", "Gamma", "Beta"]);

    let switcher = harness
        .get_by_role_and_label(Role::Button, "Workspace: Garden")
        .rect();
    let above_switcher = switcher.center_top() - egui::vec2(0.0, 2.0);
    let moved_window = drag(
        &mut harness,
        above_switcher,
        above_switcher + egui::vec2(60.0, 0.0),
        |_| {},
    );
    assert!(
        moved_window,
        "the strip above the sidebar no longer moves the window"
    );

    harness.get_by_role_and_label(Role::Tab, "Alpha").click();
    harness.run();
    assert_eq!(selected(&harness).as_deref(), Some("Alpha"));
    harness
        .get_by_role_and_label(Role::Button, "Close Gamma")
        .click();
    harness.run();
    assert_eq!(tabs(&harness), ["Alpha", "Beta"]);
}

/// The commands in `output` that move, zoom or minimize the window.
fn window_commands(output: &egui::FullOutput) -> Vec<egui::ViewportCommand> {
    output
        .viewport_output
        .values()
        .flat_map(|viewport| viewport.commands.iter().cloned())
        .filter(|command| {
            matches!(
                command,
                egui::ViewportCommand::StartDrag
                    | egui::ViewportCommand::Maximized(_)
                    | egui::ViewportCommand::Minimized(_)
            )
        })
        .collect()
}

/// Double-click at `pos`, as `double_click_at` does, and return the
/// window commands the frames asked for.
fn double_click_commands(harness: &mut Ui, pos: egui::Pos2) -> Vec<egui::ViewportCommand> {
    let button = |pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    harness.input_mut().events.extend([
        Event::PointerMoved(pos),
        button(true),
        button(false),
        button(true),
        button(false),
    ]);
    harness.step();
    let mut commands = window_commands(harness.output());
    harness.run();
    commands.extend(window_commands(harness.output()));
    commands
}

/// A double-click on the title bar row's empty part asks the desktop
/// shell, once, for what the system's setting says a title bar does, and
/// sends no window command of its own. One on a tab only keeps the tab.
#[test]
fn a_double_click_on_the_row_asks_once_for_the_system_s_title_bar_action() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    harness.set_size(egui::vec2(1300.0, 700.0));
    row(&harness, "Alpha").click();
    harness.run();
    let asked =
        |harness: &Ui| app_ui::take_title_bar_double_click(&harness.ctx, egui::ViewportId::ROOT);

    let tab = harness
        .get_by_role_and_label(Role::Tab, "Alpha (preview)")
        .rect();
    let commands = double_click_commands(&mut harness, tab.left_center() + egui::vec2(12.0, 0.0));
    assert_eq!(tabs(&harness), ["Alpha"]);
    assert!(
        !asked(&harness),
        "a double-click on a tab asked for the action"
    );
    assert!(
        commands.is_empty(),
        "a double-click on a tab sent {commands:?}"
    );

    let empty = egui::pos2(tab.right() + 80.0, tab.center().y);
    let commands = double_click_commands(&mut harness, empty);
    assert!(asked(&harness), "a double-click on the row asked nothing");
    assert!(!asked(&harness), "one double-click asked twice");
    assert!(commands.is_empty(), "the row sent {commands:?}");
}

/// The sidebar's resize edge takes its drags in the title bar row too,
/// where the row's empty part would move the window, and shows its cursor
/// there.
#[test]
fn the_sidebar_s_edge_resizes_it_in_the_title_bar_row() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    let edge = harness.get_by_role(Role::Splitter).rect();
    // Just past the divider, inside the edge and the title bar row.
    let grab = egui::pos2(edge.right() - 1.0, edge.top() + 10.0);

    harness.hover_at(grab);
    harness.run();
    let cursor = harness.output().platform_output.cursor_icon;
    assert!(
        matches!(
            cursor,
            egui::CursorIcon::ResizeEast
                | egui::CursorIcon::ResizeWest
                | egui::CursorIcon::ResizeHorizontal
        ),
        "the edge shows {cursor:?}"
    );
    let moved_window = drag(&mut harness, grab, grab + egui::vec2(40.0, 0.0), |_| {});
    assert!(
        !moved_window,
        "grabbing the sidebar's edge moved the window"
    );
    let widened = harness.get_by_role(Role::Splitter).rect().center().x - edge.center().x;
    assert!(widened > 30.0, "the sidebar widened by {widened}");
}

/// Where each textured mesh of the last frame drew: the icons, here.
fn textured_meshes(harness: &Ui) -> Vec<egui::Rect> {
    fn visit(shape: &egui::Shape, found: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    visit(shape, found);
                }
            }
            egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                found.push(mesh.calc_bounds());
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in &harness.output().shapes {
        visit(&clipped.shape, &mut found);
    }
    found
}

/// At two pixels a point, every icon starts on a screen pixel, the close
/// buttons of tabs a fractional width wide included, so each of an icon's
/// pixels covers one screen pixel.
#[test]
fn icons_sit_on_the_pixel_grid() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    harness.set_pixels_per_point(2.0);
    for name in ["Alpha", "Beta", "Gamma"] {
        double_click(&mut harness, name);
    }
    let tab = harness.get_by_role_and_label(Role::Tab, "Beta").rect();
    harness.hover_at(tab.center());
    harness.run();
    assert!(
        tab.left().fract() != 0.0,
        "the tabs need a fractional width: {tab:?}"
    );

    let icons = textured_meshes(&harness);
    let tabs: Vec<egui::Rect> = ["Alpha", "Beta", "Gamma"]
        .iter()
        .map(|name| harness.get_by_role_and_label(Role::Tab, name).rect())
        .collect();
    let close_icons = icons
        .iter()
        .filter(|icon| tabs.iter().any(|tab| tab.contains_rect(**icon)))
        .count();
    assert_eq!(close_icons, 2, "the active and the hovered tab: {icons:?}");
    let off_grid: Vec<_> = icons
        .iter()
        .filter(|icon| {
            let on_grid = |value: f32| ((value * 2.0) - (value * 2.0).round()).abs() < 0.01;
            !(on_grid(icon.min.x) && on_grid(icon.min.y))
        })
        .collect();
    assert!(
        off_grid.is_empty(),
        "icons off the pixel grid: {off_grid:?}"
    );
}

/// Open `name`'s row menu with a right-click and choose `item`.
fn choose_from_row_menu(harness: &mut Ui, name: &str, item: &str) {
    row(harness, name).click_secondary();
    harness.run();
    harness
        .get_by_label(&format!("Actions for {name}"))
        .get_by_role_and_label(Role::Button, item)
        .click();
    harness.run();
}

#[test]
fn renaming_a_file_from_its_row_menu_renames_its_tab() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    choose_from_row_menu(&mut harness, "Alpha", "Open in new tab");
    row(&harness, "Beta").click();
    harness.run();
    assert_eq!(tabs(&harness), ["Alpha", "Beta (preview)"]);

    choose_from_row_menu(&mut harness, "Alpha", "Rename\u{2026}");
    harness
        .get_by_role_and_label(Role::TextInput, "New name for Alpha")
        .type_text("Omega");
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();

    assert!(folder.join("Omega.md").is_file());
    assert_eq!(tabs(&harness), ["Omega", "Beta (preview)"]);
    assert_eq!(selected(&harness).as_deref(), Some("Beta (preview)"));
}

#[test]
fn moving_a_file_to_the_trash_from_its_row_menu_closes_its_tab() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    choose_from_row_menu(&mut harness, "Alpha", "Open in new tab");
    choose_from_row_menu(&mut harness, "Beta", "Open in new tab");
    choose_from_row_menu(&mut harness, "Gamma", "Open");
    assert_eq!(tabs(&harness), ["Alpha", "Beta", "Gamma (preview)"]);
    harness.get_by_role_and_label(Role::Tab, "Beta").click();
    harness.run();

    choose_from_row_menu(&mut harness, "Beta", "Move to Trash");

    assert!(!folder.join("Beta.md").exists());
    assert_eq!(tabs(&harness), ["Alpha", "Gamma (preview)"]);
    assert_eq!(selected(&harness).as_deref(), Some("Gamma (preview)"));
}

/// What the last frame painted wholly inside `area`, leaving out text.
fn painted_inside(harness: &Ui, area: egui::Rect) -> Vec<egui::Rect> {
    fn visit(shape: &egui::Shape, area: egui::Rect, found: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    visit(shape, area, found);
                }
            }
            egui::Shape::Noop | egui::Shape::Text(_) => {}
            shape => {
                let bounds = shape.visual_bounding_rect();
                if bounds.is_positive() && area.contains_rect(bounds) {
                    found.push(bounds);
                }
            }
        }
    }
    let mut found = Vec::new();
    for clipped in &harness.output().shapes {
        visit(&clipped.shape, area, &mut found);
    }
    found
}

/// Tabs share the row evenly between 100 and 240 points: one tab is at
/// its widest, and a few shrink together while they still fit, with no
/// button to list them.
#[test]
fn tabs_share_the_row_between_their_narrowest_and_widest() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), &folder);
    let names: Vec<String> = (1..=4).map(|n| format!("Note {n}")).collect();
    for name in &names {
        std::fs::write(folder.join(format!("{name}.md")), b"x").unwrap();
    }
    let mut harness = harness_registering(app_data.path(), FakePicker::default(), |_| {});
    settle(&mut harness);
    harness.set_size(egui::vec2(1100.0, 750.0));
    harness.run();
    let open = |harness: &mut Ui, name: &str| {
        harness
            .state_mut()
            .app_mut()
            .open_file(&format!("{name}.md"), app_core::OpenAs::Kept);
        harness.run();
    };
    let width = |harness: &Ui, name: &str| {
        harness
            .get_by_role_and_label(Role::Tab, name)
            .rect()
            .width()
    };

    open(&mut harness, &names[0]);
    assert_eq!(width(&harness, &names[0]), 240.0);
    for name in &names[1..] {
        open(&mut harness, name);
    }
    let widths: Vec<f32> = names.iter().map(|name| width(&harness, name)).collect();
    assert!(
        widths.iter().all(|width| (100.0..240.0).contains(width)),
        "{widths:?}"
    );
    assert!(
        widths.windows(2).all(|pair| pair[0] == pair[1]),
        "{widths:?}"
    );
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "All tabs")
            .is_none()
    );
}

/// At a normal window width, tabs past the window's edge scroll sideways
/// with the mouse wheel, inside the title bar row, with no scrollbar.
#[test]
fn tabs_fit_the_title_bar_row_with_no_scrollbar_and_still_scroll() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), &folder);
    let names: Vec<String> = (1..=12).map(|n| format!("A long note name {n}")).collect();
    for name in &names {
        std::fs::write(folder.join(format!("{name}.md")), b"x").unwrap();
    }
    let mut harness = harness_registering(app_data.path(), FakePicker::default(), |_| {});
    settle(&mut harness);
    harness.set_size(egui::vec2(1100.0, 750.0));
    harness.run();
    for name in &names {
        harness
            .state_mut()
            .app_mut()
            .open_file(&format!("{name}.md"), app_core::OpenAs::Kept);
    }
    harness
        .state_mut()
        .app_mut()
        .open_page(app_core::INBOX_PAGE, app_core::OpenAs::Kept);
    harness.run();

    let tab_rect = |harness: &Ui, name: &str| harness.get_by_role_and_label(Role::Tab, name).rect();
    let first = tab_rect(&harness, &names[0]);
    let last = tab_rect(&harness, "Inbox");
    // The row starts where the window's content does, here the harness's
    // margin. Tabs stand on the bottom of its 36 points, where the page
    // starts, 6 points below its top, and are no narrower than 100.
    let row_top = harness
        .get_by_role_and_label(Role::Splitter, "Resize panel")
        .rect()
        .top();
    assert!(
        (first.top() - (row_top + 6.0)).abs() <= 0.5,
        "{first:?} under {row_top}"
    );
    assert!(
        (first.bottom() - (row_top + 36.0)).abs() <= 0.5,
        "{first:?} under {row_top}"
    );
    assert!(first.width() >= 100.0, "{first:?}");
    assert!(
        first.left() < 0.0 || last.right() > 1100.0,
        "the tabs should run past the window: {first:?} {last:?}"
    );

    let wheel = |harness: &mut Ui, delta: f32| {
        harness.hover_at(egui::pos2(600.0, 28.0));
        harness.run();
        for _ in 0..8 {
            harness.event(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, delta),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers: Modifiers::NONE,
            });
            harness.step();
        }
        harness.run();
    };

    // The Inbox opened last, so the bar may sit anywhere; scroll to its
    // start, then down moves the tabs left.
    wheel(&mut harness, 300.0);
    let start = tab_rect(&harness, &names[0]).left();
    wheel(&mut harness, -300.0);
    let moved = tab_rect(&harness, &names[0]).left();
    assert!(moved < start - 200.0, "the wheel moved {start} to {moved}");

    // A scrollbar shows with the pointer over the row, in the strip under
    // the tabs. Nothing paints there.
    harness.hover_at(egui::pos2(600.0, 28.0));
    harness.run_steps(3);
    let under_the_tabs = egui::Rect::from_x_y_ranges(0.0..=1100.0, first.bottom()..=row_top + 35.0);
    let bars = painted_inside(&harness, under_the_tabs);
    assert!(bars.is_empty(), "painted under the tabs: {bars:?}");
    assert!(
        tab_rect(&harness, "Inbox").right() <= 1100.0,
        "the wheel reaches the last tab"
    );
}

/// A workspace with `count` notes, "Note 1" on, each open in a kept tab, in
/// a window `width` wide. The last one opened is active.
fn many_tabs(
    app_data: &std::path::Path,
    parent: &std::path::Path,
    count: usize,
    width: f32,
) -> (Ui, Vec<String>) {
    let folder = parent.join("Garden");
    create_workspace(app_data, &folder);
    let names: Vec<String> = (1..=count).map(|n| format!("Note {n}")).collect();
    for name in &names {
        std::fs::write(folder.join(format!("{name}.md")), b"x").unwrap();
    }
    let mut harness = harness_registering(app_data, FakePicker::default(), |_| {});
    settle(&mut harness);
    harness.set_size(egui::vec2(width, 750.0));
    harness.run();
    for name in &names {
        harness
            .state_mut()
            .app_mut()
            .open_file(&format!("{name}.md"), app_core::OpenAs::Kept);
    }
    harness.run();
    (harness, names)
}

fn tab_rect(harness: &Ui, name: &str) -> egui::Rect {
    harness.get_by_role_and_label(Role::Tab, name).rect()
}

/// Whether the tab `name` shows whole in the part of the row that shows.
fn shows(harness: &Ui, name: &str) -> bool {
    let strip = harness.get_by_role_and_label(Role::TabList, "Tabs").rect();
    let tab = tab_rect(harness, name);
    tab.left() >= strip.left() - 0.5 && tab.right() <= strip.right() + 0.5
}

/// Swipe sideways on a trackpad over the row, `delta` points a frame.
fn swipe(harness: &mut Ui, delta: f32) {
    let over = harness
        .get_by_role_and_label(Role::TabList, "Tabs")
        .rect()
        .center();
    harness.hover_at(over);
    harness.run();
    for _ in 0..8 {
        harness.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(delta, 0.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Trackpad,
            modifiers: Modifiers::NONE,
        });
        harness.step();
    }
    harness.run();
}

/// Pick `name` from the menu of every tab, and return that menu's items.
fn pick_from_list(harness: &mut Ui, name: &str) -> Vec<String> {
    harness
        .get_by_role_and_label(Role::Button, "All tabs")
        .click();
    harness.run();
    let items = harness
        .get_all_by_role(Role::MenuItem)
        .filter_map(|item| item.accesskit_node().label())
        .collect();
    harness.get_by_role_and_label(Role::MenuItem, name).click();
    harness.run();
    items
}

/// Fourteen tabs in a 1,000-point window shrink to their narrowest, then
/// the row scrolls sideways with a trackpad. The button at the row's end
/// lists every tab, and picking one out of sight activates it and scrolls
/// it into sight, the active one too.
#[test]
fn tabs_that_don_t_fit_shrink_scroll_and_show_in_a_list() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let (mut harness, names) = many_tabs(app_data.path(), parent.path(), 14, 1000.0);
    let widths: Vec<f32> = names
        .iter()
        .map(|name| tab_rect(&harness, name).width())
        .collect();
    assert!(widths.iter().all(|width| *width == 100.0), "{widths:?}");
    let shown = names.iter().filter(|name| shows(&harness, name)).count();
    assert!(shown >= 6, "{shown} tabs show whole");
    assert!(
        shows(&harness, &names[13]),
        "the active tab is out of sight"
    );
    assert!(!shows(&harness, &names[0]));

    // A swipe right brings the first tabs into sight, and one left takes
    // them out again.
    let start = tab_rect(&harness, &names[0]).left();
    swipe(&mut harness, 120.0);
    let swiped = tab_rect(&harness, &names[0]).left();
    assert!(
        swiped > start + 300.0,
        "the swipe moved {start} to {swiped}"
    );
    swipe(&mut harness, -120.0);
    assert!(tab_rect(&harness, &names[0]).left() < swiped - 300.0);

    let items = pick_from_list(&mut harness, &names[0]);
    assert_eq!(items, names);
    assert_eq!(selected(&harness).as_deref(), Some("Note 1"));
    assert!(shows(&harness, &names[0]), "the picked tab is out of sight");

    // Picked again once scrolled away, the active tab comes back.
    swipe(&mut harness, -120.0);
    assert!(!shows(&harness, &names[0]));
    pick_from_list(&mut harness, &names[0]);
    assert!(
        shows(&harness, &names[0]),
        "the active tab stayed out of sight"
    );
}

/// The active tab stays in sight when the window narrows, and when a tab
/// before it closes.
#[test]
fn the_active_tab_stays_in_sight_as_the_row_changes() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let (mut harness, names) = many_tabs(app_data.path(), parent.path(), 14, 1600.0);
    harness.get_by_role_and_label(Role::Tab, &names[10]).click();
    harness.run();
    assert!(shows(&harness, &names[10]));

    harness.set_size(egui::vec2(800.0, 750.0));
    harness.run();
    assert!(shows(&harness, &names[10]), "narrowing hid the active tab");

    swipe(&mut harness, 120.0);
    assert!(!shows(&harness, &names[10]));
    harness
        .get_by_role_and_label(Role::Button, "Close Note 1")
        .click();
    harness.run();
    assert!(
        shows(&harness, &names[10]),
        "closing a tab hid the active one"
    );
}

/// A tab dragged to the end of a row that scrolls scrolls it, and drops
/// past the tabs that showed when the drag began, without moving the
/// window.
#[test]
fn a_tab_dragged_to_the_row_s_end_scrolls_it_and_drops_there() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let (mut harness, names) = many_tabs(app_data.path(), parent.path(), 14, 1000.0);
    pick_from_list(&mut harness, &names[0]);
    let last_shown = names
        .iter()
        .rposition(|name| shows(&harness, name))
        .unwrap();
    assert!(last_shown < 10, "{last_shown}");

    let strip = harness.get_by_role_and_label(Role::TabList, "Tabs").rect();
    let from = tab_rect(&harness, &names[0]).left_center() + egui::vec2(25.0, 0.0);
    let end = egui::pos2(strip.right() - 4.0, from.y);
    let moved_window = drag(&mut harness, from, end, |harness| {
        for _ in 0..40 {
            harness.hover_at(end);
            harness.step();
        }
    });
    assert!(!moved_window, "dragging a tab moved the window");
    let order = tabs(&harness);
    let at = order.iter().position(|tab| tab == "Note 1").unwrap();
    assert_eq!(at, 13, "{order:?}");
}
