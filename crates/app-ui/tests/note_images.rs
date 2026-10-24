//! Images in an open note through AccessKit: a workspace image draws in
//! place, AVIF included, keeps drawing while the note saves, and draws
//! again when another app changes it, for every link and tab that shows
//! it. An SVG draws nothing from the files it names, and a remote image
//! waits behind its Load image button, which offers a retry when the load
//! fails.

mod support;

use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui::load::{ImagePoll, SizeHint};
use egui_kittest::{Harness, kittest::Queryable};
use support::{FakePicker, create_workspace, harness, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

/// A workspace holding `Plan.md` with `note` as its text and a 4 by 3 PNG,
/// `pic.png`, opened with the note open.
fn opened_note(app_data: &std::path::Path, folder: &std::path::Path, note: &str) -> Ui {
    opened_note_with(app_data, folder, note, &[])
}

/// [`opened_note`], with `files` written into the workspace too.
fn opened_note_with(
    app_data: &std::path::Path,
    folder: &std::path::Path,
    note: &str,
    files: &[(&str, &[u8])],
) -> Ui {
    create_workspace(app_data, "Garden", folder);
    std::fs::write(folder.join("Plan.md"), note).unwrap();
    std::fs::write(folder.join("pic.png"), png(4, 3)).unwrap();
    for (name, bytes) in files {
        std::fs::write(folder.join(name), bytes).unwrap();
    }
    let mut harness = harness(app_data, FakePicker::default());
    settle(&mut harness);
    harness
        .get_by_role_and_label(Role::TreeItem, "Plan")
        .click();
    harness.run_steps(4);
    harness
}

/// A PNG, `width` by `height` pixels.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(width, height)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    png.into_inner()
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

/// Whether the image `name` drew at a size under 10 points, past the
/// 96-point box it takes while it loads.
fn drew_small(harness: &Ui, name: &str) -> bool {
    harness
        .query_by_role_and_label(Role::Image, name)
        .is_some_and(|image| image.rect().width() < 10.0)
}

#[test]
fn an_avif_image_draws_in_the_note() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let avif = include_bytes!("fixtures/pic.avif");
    let mut harness = opened_note_with(
        app_data.path(),
        &folder,
        "Intro\n\n![[pic.avif]]\n",
        &[("pic.avif", avif)],
    );

    // Decoded, the image takes its own 4 by 4 pixels.
    wait_until(&mut harness, "the AVIF image drew at its size", |harness| {
        drew_small(harness, "pic.avif")
    });
}

#[test]
fn an_svg_draws_nothing_from_a_file_outside_the_workspace() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let outside = tempfile::tempdir().unwrap();
    let red = outside.path().join("red.svg");
    std::fs::write(
        &red,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="red"/></svg>"#,
    )
    .unwrap();
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><image href="{}" width="8" height="8"/></svg>"#,
        red.display()
    );
    let mut harness = opened_note_with(
        app_data.path(),
        &folder,
        "Intro\n\n![[pic.svg]]\n",
        &[("pic.svg", svg.as_bytes())],
    );

    wait_until(&mut harness, "the SVG drew at its size", |harness| {
        drew_small(harness, "pic.svg")
    });
    // egui holds the workspace image's pixels under this address. Every
    // one is clear: the red square outside the workspace never drew.
    let image = match harness
        .ctx
        .try_load_image("bytes://noura-image/pic.svg", SizeHint::default())
    {
        Ok(ImagePoll::Ready { image }) => image,
        _ => panic!("egui holds no pixels for pic.svg"),
    };
    assert_eq!(image.size, [8, 8]);
    assert!(image.pixels.iter().all(|pixel| pixel.a() == 0));
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

#[test]
fn saving_the_note_leaves_its_images_drawn() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened_note(app_data.path(), &folder, "Intro\n\n![[pic.png]]\n");
    wait_until(&mut harness, "the image drew at its size", |harness| {
        drew_small(harness, "pic.png")
    });
    let listing = |harness: &Ui| {
        let app = harness.state().app();
        let tab = app.active_tab().expect("a tab is open").id();
        app.note_links(tab).expect("the tab shows a note").seq()
    };
    let before = listing(&harness);

    harness.event(egui::Event::Text("Typed. ".into()));
    // The note saves after a pause, and the save lists the workspace's
    // files again. Through all of it and for a while after, the image
    // keeps its own size, where reading it again would draw the box it
    // takes while it loads.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut listed = None;
    while listed.is_none_or(|at: Instant| at.elapsed() < Duration::from_millis(500)) {
        assert!(
            Instant::now() < deadline,
            "the save never showed in a listing"
        );
        std::thread::sleep(Duration::from_millis(20));
        harness.step();
        assert!(drew_small(&harness, "pic.png"), "the image loaded again");
        let saved = std::fs::read_to_string(folder.join("Plan.md"))
            .unwrap()
            .contains("Typed. ");
        if listed.is_none() && saved && listing(&harness) != before {
            listed = Some(Instant::now());
        }
    }
}

/// Whether the image `name` drew 20 points wide.
fn drew_20_wide(harness: &Ui, name: &str) -> bool {
    harness
        .query_by_role_and_label(Role::Image, name)
        .is_some_and(|image| (image.rect().width() - 20.0).abs() < 0.5)
}

/// Run frames for a fifth of a second, checking `still` after each.
fn keeps(harness: &mut Ui, what: &str, still: impl Fn(&Ui) -> bool) {
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(20));
        harness.step();
        assert!(still(harness), "{what}");
    }
}

#[test]
fn two_links_to_one_image_both_draw_it_again_after_it_changes() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let note = "Intro\n\n![[pic.png]]\n\n![](./pic.png)\n";
    let mut harness = opened_note(app_data.path(), &folder, note);
    wait_until(&mut harness, "both links drew the image", |harness| {
        drew_small(harness, "pic.png") && drew_small(harness, "./pic.png")
    });

    // Another app makes the image 20 by 15 pixels, then adds a note. The
    // listing that follows shows the note, and the image's new size.
    let listing = |harness: &Ui| {
        let app = harness.state().app();
        let tab = app.active_tab().expect("a tab is open").id();
        app.note_links(tab).expect("the tab shows a note").seq()
    };
    let before = listing(&harness);
    std::fs::write(folder.join("pic.png"), png(20, 15)).unwrap();
    std::fs::write(folder.join("Later.md"), "Later\n").unwrap();
    wait_until(&mut harness, "a new listing", |harness| {
        listing(harness) != before
    });
    let both =
        |harness: &Ui| drew_20_wide(harness, "pic.png") && drew_20_wide(harness, "./pic.png");
    wait_until(&mut harness, "both links drew the new image", both);
    keeps(&mut harness, "a link drew the old image again", both);
}

/// Open `name`'s row menu with a right-click and choose `item`.
fn choose_from_row_menu(harness: &mut Ui, name: &str, item: &str) {
    harness
        .get_by_role_and_label(Role::TreeItem, name)
        .click_secondary();
    harness.run();
    harness
        .get_by_label(&format!("Actions for {name}"))
        .get_by_role_and_label(Role::Button, item)
        .click();
    harness.run();
}

#[test]
fn an_image_two_tabs_draw_stays_drawn_until_neither_draws_it() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    std::fs::write(folder.join("Plan.md"), "Intro\n\n![[pic.png]]\n").unwrap();
    std::fs::create_dir(folder.join("Notes")).unwrap();
    std::fs::write(folder.join("Notes/Other.md"), "Other\n\n![[pic.png]]\n").unwrap();
    std::fs::write(folder.join("pic.png"), png(4, 3)).unwrap();
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);

    // Both notes link to the image at the workspace's root.
    choose_from_row_menu(&mut harness, "Plan", "Open in new tab");
    wait_until(&mut harness, "Plan drew the image", |harness| {
        drew_small(harness, "pic.png")
    });
    harness
        .get_by_role_and_label(Role::TreeItem, "Notes")
        .click();
    harness.run_steps(4);
    choose_from_row_menu(&mut harness, "Other", "Open in new tab");
    wait_until(&mut harness, "Other drew the image", |harness| {
        drew_small(harness, "pic.png")
    });

    // An image of the same name lands beside Other, and Other's link leads
    // to it now.
    std::fs::write(folder.join("Notes/pic.png"), png(20, 15)).unwrap();
    wait_until(&mut harness, "Other drew the image beside it", |harness| {
        drew_20_wide(harness, "pic.png")
    });

    // Plan still draws the image at the root.
    harness.get_by_role_and_label(Role::Tab, "Plan").click();
    harness.run_steps(2);
    keeps(&mut harness, "Plan lost its image", |harness| {
        drew_small(harness, "pic.png")
    });

    // Closing Other leaves Plan's image drawn. Closing Plan too leaves no
    // note drawing it, and egui lets its bytes go.
    harness
        .get_by_role_and_label(Role::Button, "Close Other")
        .click();
    harness.run_steps(2);
    keeps(&mut harness, "closing Other took Plan's image", |harness| {
        drew_small(harness, "pic.png")
    });
    let held = |harness: &Ui| {
        harness
            .ctx
            .try_load_bytes("bytes://noura-image/pic.png")
            .is_ok()
    };
    assert!(held(&harness));
    harness
        .get_by_role_and_label(Role::Button, "Close Plan")
        .click();
    harness.run_steps(2);
    assert!(!held(&harness), "egui still holds the image");
}
