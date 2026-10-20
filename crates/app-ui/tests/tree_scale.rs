//! The file tree in a 10,000-file workspace.

mod support;

use std::time::{Duration, Instant};

use egui::{
    Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, TouchPhase, accesskit::Role, vec2,
};
use egui_kittest::kittest::Queryable;
use support::{FakePicker, create_workspace, harness, settle};

const FILES: usize = 10_000;

/// A workspace with `FILES` files side by side, so every one is a row.
fn big_workspace(app_data: &std::path::Path, folder: &std::path::Path, extension: &str) {
    create_workspace(app_data, "Big", folder);
    for index in 0..FILES {
        std::fs::write(folder.join(format!("File {index:05}.{extension}")), b"x").unwrap();
    }
}

#[test]
fn the_tree_draws_only_the_rows_in_view() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    big_workspace(app_data.path(), &parent.path().join("Big"), "txt");
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);

    let drawn = harness.get_all_by_role(Role::TreeItem).count();
    assert!((10..100).contains(&drawn), "drew {drawn} rows of {FILES}");

    harness
        .get_by_role_and_label(Role::TreeItem, "File 00000.txt")
        .focus();
    harness.run();
    harness.key_press(Key::End);
    harness.run();

    let last = harness.get_by_role_and_label(Role::TreeItem, "File 09999.txt");
    assert!(last.is_focused());
    assert!(harness.query_by_label("File 00000.txt").is_none());
    assert!(harness.get_all_by_role(Role::TreeItem).count() < 100);
}

/// Report the time each frame takes while the tree scrolls. It measures
/// egui's layout and tessellation on the CPU, not the GPU. CI runs it in
/// release on the macOS arm64 runner:
/// `cargo test -p app-ui --release --test tree_scale -- --ignored --nocapture`.
#[test]
#[ignore = "a timing report; CI runs it in release on macOS arm64"]
fn tree_scroll_frame_times() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    big_workspace(app_data.path(), &parent.path().join("Big"), "md");
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);
    let row = harness
        .get_by_role_and_label(Role::TreeItem, "File 00000")
        .rect()
        .center();
    harness.hover_at(row);
    harness.run();

    let mut frames = Vec::new();
    for _ in 0..600 {
        harness.event(Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(0.0, -240.0),
            phase: TouchPhase::Move,
            source: MouseWheelSource::Unknown,
            modifiers: Modifiers::NONE,
        });
        let start = Instant::now();
        harness.step();
        frames.push(start.elapsed());
    }

    frames.sort();
    let at = |fraction: f64| frames[((frames.len() - 1) as f64 * fraction) as usize];
    let ms = |duration: Duration| duration.as_secs_f64() * 1000.0;
    println!(
        "file tree, {FILES} rows, {} scroll frames: p50 {:.3} ms, p95 {:.3} ms, max {:.3} ms",
        frames.len(),
        ms(at(0.5)),
        ms(at(0.95)),
        ms(at(1.0)),
    );
    // The first row stays in the AccessKit tree as the tree's tab stop, so
    // look for the second.
    assert!(
        harness.query_by_label("File 00001").is_none(),
        "the tree scrolled"
    );
}
