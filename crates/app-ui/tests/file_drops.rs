//! Dragging between the file tree and the file manager, through AccessKit
//! and the dropped files egui hands over.

mod support;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use app_core::Platform;
use egui::{Pos2, accesskit::Role, vec2};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, FakeSystem, create_workspace, harness_with, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

/// A workspace with a folder and two notes, opened at launch.
fn opened(app_data: &Path, folder: &Path, platform: Platform) -> Ui {
    create_workspace(app_data, "Garden", folder);
    for (path, bytes) in [
        ("Docs/Guide.md", b"g".as_slice()),
        ("Readme.md", b"r"),
        ("Todo.md", b"t"),
    ] {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let mut harness = harness_with(app_data, FakePicker::default(), FakeSystem::new(platform));
    settle(&mut harness);
    harness
}

/// A file dropped from the file manager, as egui-winit hands it over.
#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|error| error.to_string())
    }
}

fn rows(harness: &Ui) -> Vec<String> {
    harness
        .get_all_by_role(Role::TreeItem)
        .filter_map(|node| node.accesskit_node().label())
        .collect()
}

fn row_center(harness: &Ui, name: &str) -> Pos2 {
    harness
        .get_by_role_and_label(Role::TreeItem, name)
        .rect()
        .center()
}

/// A point on the blank part of the tree, below its rows.
fn blank(harness: &Ui) -> Pos2 {
    row_center(harness, "Todo") + vec2(0.0, 200.0)
}

/// The rows and the tree that say they'd receive a drop.
fn drop_targets(harness: &Ui) -> Vec<String> {
    let is_target = |node: &egui_kittest::Node<'_>| {
        node.accesskit_node()
            .description()
            .is_some_and(|description| description.contains("drop target"))
    };
    harness
        .get_all_by_role(Role::TreeItem)
        .chain(harness.get_all_by_role(Role::Tree))
        .filter(is_target)
        .filter_map(|node| node.accesskit_node().label())
        .collect()
}

/// The file manager drags `paths` over the window to `at`.
fn hover_files(harness: &mut Ui, paths: &[PathBuf], at: Pos2) {
    harness.input_mut().hovered_files = paths
        .iter()
        .map(|path| egui::HoveredFile {
            path: Some(path.clone()),
            ..Default::default()
        })
        .collect();
    harness.hover_at(at);
    // egui keeps repainting while files hover, so `run` would never settle.
    harness.run_steps(2);
}

/// The file manager drops `paths` where the pointer is.
fn drop_files(harness: &mut Ui, paths: &[PathBuf]) {
    harness.input_mut().hovered_files.clear();
    harness.input_mut().dropped_files = paths
        .iter()
        .map(|path| Arc::new(Dropped(path.clone())) as egui::DroppedFileHandle)
        .collect();
    harness.run();
}

#[test]
fn files_dropped_from_the_file_manager_copy_into_the_folder_under_the_pointer() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder, Platform::current());
    let outside = parent.path().join("Outside");
    std::fs::create_dir_all(outside.join("Photos")).unwrap();
    std::fs::write(outside.join("scan.pdf"), b"%PDF").unwrap();
    std::fs::write(outside.join("Photos/cat.jpg"), b"jpg").unwrap();
    std::fs::write(outside.join("map.png"), b"png").unwrap();

    let docs = row_center(&harness, "Docs");
    let both = [outside.join("scan.pdf"), outside.join("Photos")];
    hover_files(&mut harness, &both, docs);
    drop_files(&mut harness, &both);
    assert_eq!(
        std::fs::read(folder.join("Docs/scan.pdf")).unwrap(),
        b"%PDF"
    );
    assert_eq!(
        std::fs::read(folder.join("Docs/Photos/cat.jpg")).unwrap(),
        b"jpg"
    );
    assert!(outside.join("scan.pdf").is_file());

    // A file row's folder, then the blank part of the tree.
    let guide = row_center(&harness, "Guide");
    hover_files(&mut harness, &[outside.join("map.png")], guide);
    drop_files(&mut harness, &[outside.join("map.png")]);
    assert!(folder.join("Docs/map.png").is_file());
    let blank = blank(&harness);
    hover_files(&mut harness, &[outside.join("map.png")], blank);
    drop_files(&mut harness, &[outside.join("map.png")]);
    assert!(folder.join("map.png").is_file());
    assert!(outside.join("map.png").is_file());
}

#[test]
fn the_folder_under_dragged_files_lights_up_opens_after_a_second_and_clears_when_they_leave() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder, Platform::current());
    let scan = parent.path().join("scan.pdf");
    std::fs::write(&scan, b"%PDF").unwrap();
    let docs = row_center(&harness, "Docs");

    hover_files(&mut harness, std::slice::from_ref(&scan), docs);
    assert_eq!(drop_targets(&harness), ["Docs"]);
    assert_eq!(rows(&harness), ["Docs", "Readme", "Todo"]);
    std::thread::sleep(app_core::SPRING_OPEN_DELAY);
    harness.run_steps(2);
    assert_eq!(rows(&harness), ["Docs", "Guide", "Readme", "Todo"]);

    let blank = blank(&harness);
    hover_files(&mut harness, std::slice::from_ref(&scan), blank);
    assert_eq!(drop_targets(&harness), ["Files"]);

    // The pointer leaves the window, and the file manager says so.
    harness.input_mut().hovered_files.clear();
    harness.run();
    assert!(drop_targets(&harness).is_empty());
    assert_eq!(rows(&harness), ["Docs", "Readme", "Todo"]);

    // The pointer moves past the window's edge while still carrying files.
    hover_files(&mut harness, std::slice::from_ref(&scan), docs);
    hover_files(
        &mut harness,
        std::slice::from_ref(&scan),
        Pos2::new(-20.0, 300.0),
    );
    assert!(drop_targets(&harness).is_empty());

    // The files leave from over a row. The desktop app takes away the
    // pointer it had from the system, so the row doesn't stay lit.
    hover_files(&mut harness, std::slice::from_ref(&scan), docs);
    assert_eq!(drop_targets(&harness), ["Docs"]);
    harness.input_mut().hovered_files.clear();
    harness.event(egui::Event::PointerGone);
    harness.run();
    assert!(drop_targets(&harness).is_empty());
    assert_eq!(harness.ctx.input(|input| input.pointer.hover_pos()), None);
}

#[test]
fn a_drop_says_what_it_copied_and_what_it_skipped() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder, Platform::current());
    let outside = parent.path().join("Outside");
    std::fs::create_dir_all(outside.join("Trip")).unwrap();
    std::fs::write(outside.join("a.txt"), b"a").unwrap();
    std::fs::write(outside.join("b.txt"), b"b").unwrap();
    std::fs::write(outside.join("Trip/.DS_Store"), b"junk").unwrap();
    let paths = [
        outside.join("a.txt"),
        outside.join("b.txt"),
        outside.join("Trip"),
    ];

    let docs = row_center(&harness, "Docs");
    hover_files(&mut harness, &paths, docs);
    drop_files(&mut harness, &paths);

    let notice = harness.get_by_role(Role::Status);
    notice.get_by_label("Copied 3 items to \u{201c}Docs\u{201d}");
    notice.get_by_label("Skipped 1 hidden file or link.");
}

/// Press on the row named `name` and drag it past the window's left edge.
fn drag_past_the_edge(harness: &mut Ui, name: &str) {
    let start = row_center(harness, name);
    harness.hover_at(start);
    harness.run();
    harness.drag_at(start);
    harness.run();
    for x in [start.x - 40.0, 60.0, 0.0, -20.0] {
        harness.hover_at(Pos2::new(x, start.y));
        harness.run();
    }
}

#[test]
fn a_row_dragged_out_and_dropped_back_on_the_tree_moves_and_makes_no_copy() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder, Platform::MacOs);

    drag_past_the_edge(&mut harness, "Readme");
    let drag = harness
        .state_mut()
        .take_drag_out()
        .expect("the row became a native drag");
    assert_eq!(drag.name, "Readme");

    // The system carries the file back over the tree and drops it on Docs.
    let docs = row_center(&harness, "Docs");
    hover_files(&mut harness, std::slice::from_ref(&drag.path), docs);
    drop_files(&mut harness, std::slice::from_ref(&drag.path));

    assert_eq!(std::fs::read(folder.join("Docs/Readme.md")).unwrap(), b"r");
    assert!(!folder.join("Readme.md").exists());
    assert!(!folder.join("Docs/Readme 1.md").exists());
}

#[test]
fn on_linux_a_row_past_the_window_edge_stays_an_in_tree_drag() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder, Platform::Linux);

    drag_past_the_edge(&mut harness, "Readme");
    assert!(harness.state_mut().take_drag_out().is_none());
    assert!(harness.query_by_role(Role::Alert).is_none());

    let docs = row_center(&harness, "Docs");
    harness.hover_at(docs);
    harness.run();
    harness.drop_at(docs);
    harness.run();

    assert_eq!(std::fs::read(folder.join("Docs/Readme.md")).unwrap(), b"r");
    assert!(harness.query_by_role(Role::Alert).is_none());
}
