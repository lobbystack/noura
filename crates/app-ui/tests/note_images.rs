//! Images in an open note through AccessKit: a workspace image draws in
//! place, and a remote image waits behind its Load image button, which
//! offers a retry when the load fails.

mod support;

use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui_kittest::{Harness, kittest::Queryable};
use support::{FakePicker, create_workspace, harness, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

/// A workspace holding `Plan.md` with `note` as its text and a 4 by 3 PNG,
/// `pic.png`, opened with the note open.
fn opened_note(app_data: &std::path::Path, folder: &std::path::Path, note: &str) -> Ui {
    create_workspace(app_data, "Garden", folder);
    std::fs::write(folder.join("Plan.md"), note).unwrap();
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(4, 3)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    std::fs::write(folder.join("pic.png"), png.into_inner()).unwrap();
    let mut harness = harness(app_data, FakePicker::default());
    settle(&mut harness);
    harness
        .get_by_role_and_label(Role::TreeItem, "Plan")
        .click();
    harness.run_steps(4);
    harness
}

/// Run frames, waiting in real time, until `done` holds.
fn wait_until(harness: &mut Ui, what: &str, mut done: impl FnMut(&Ui) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(harness) {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(20));
        harness.step();
    }
}

#[test]
fn a_workspace_image_draws_in_the_note() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened_note(app_data.path(), &folder, "Intro\n\n![[pic.png]]\n");

    // While it loads the image takes a 96-pixel box; decoded, it takes
    // its own 4 by 3 pixels.
    wait_until(&mut harness, "the image drew at its size", |harness| {
        harness
            .query_by_role_and_label(Role::Image, "pic.png")
            .is_some_and(|image| image.rect().width() < 10.0)
    });
}

#[test]
fn a_remote_image_waits_for_load_image_and_offers_a_retry_when_it_fails() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    // `.invalid` never resolves, so the load fails without a request.
    let note = "Intro\n\n![remote](https://images.example.invalid/pic.png)\n";
    let mut harness = opened_note(app_data.path(), &folder, note);

    harness
        .get_by_role_and_label(Role::Button, "Load image")
        .click();
    harness.run_steps(2);

    wait_until(&mut harness, "the retry button", |harness| {
        harness
            .query_by_role_and_label(Role::Button, "Couldn\u{2019}t load image. Try again")
            .is_some()
    });
    assert_eq!(
        std::fs::read_to_string(folder.join("Plan.md")).unwrap(),
        note
    );
}
