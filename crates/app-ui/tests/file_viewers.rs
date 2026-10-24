//! The file viewers through AccessKit: a PDF tab's toolbar, page and
//! thumbnails, its password form, a PDF embedded in a note, and the
//! image tab, each by its label and from the keyboard.

mod support;

use std::{
    path::Path,
    time::{Duration, Instant},
};

use app_core::{PdfState, PdfZoom, TabId};
use egui::{Key, accesskit::Role};
use egui_kittest::{Harness, kittest::Queryable};
use support::{FakePicker, create_workspace, harness, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

/// The bytes of the PDF fixture `name`, which `app-core`'s tests share.
fn pdf(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../app-core/tests/fixtures/pdf")
            .join(name),
    )
    .unwrap()
}

/// A PNG, `width` by `height` pixels.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(width, height)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    png.into_inner()
}

/// A workspace holding `files`, open in a window.
fn workspace(app_data: &Path, folder: &Path, files: &[(&str, &[u8])]) -> Ui {
    create_workspace(app_data, "Garden", folder);
    for (name, bytes) in files {
        let path = folder.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let mut harness = harness(app_data, FakePicker::default());
    settle(&mut harness);
    harness
}

/// Open the tree row labelled `row`.
fn open(harness: &mut Ui, row: &str) {
    harness.get_by_role_and_label(Role::TreeItem, row).click();
    harness.run_steps(4);
}

/// Run frames, waiting in real time, until `done` holds.
fn wait_until(harness: &mut Ui, what: &str, mut done: impl FnMut(&Ui) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(harness) {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(10));
        harness.step();
    }
}

fn active(harness: &Ui) -> TabId {
    harness.state().app().active_tab().unwrap().id()
}

fn page(harness: &Ui) -> usize {
    let app = harness.state().app();
    app.pdf(active(harness)).unwrap().page()
}

/// The drawn page labelled `label` shows.
fn shows_page(harness: &Ui, label: &str) -> bool {
    harness
        .query_by_role_and_label(Role::Image, label)
        .is_some()
}

#[test]
fn a_large_pdf_shows_its_page_count_and_the_window_keeps_drawing_while_the_page_renders() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = workspace(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("large.pdf", &pdf("large.pdf"))],
    );
    let hold = harness.state().app().hold_pdf_rendering_for_testing();

    open(&mut harness, "large.pdf");
    wait_until(&mut harness, "the page count shows", |harness| {
        harness.query_by_label("of 400").is_some()
    });
    // The page renders on the PDF's own thread, which the test holds: the
    // window keeps drawing frames meanwhile and says the page is coming.
    for _ in 0..20 {
        let started = Instant::now();
        harness.step();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "a frame stalled"
        );
    }
    harness.get_by_label("Rendering page 1\u{2026}");
    assert!(!shows_page(&harness, "Page 1 of 400"));

    drop(hold);
    wait_until(&mut harness, "the page draws", |harness| {
        shows_page(harness, "Page 1 of 400")
    });
}

#[test]
fn the_toolbar_moves_through_pages_and_zooms_from_the_keyboard_and_the_tab_keeps_its_place() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = workspace(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("text.pdf", &pdf("text.pdf")), ("Plan.md", b"# Plan\n")],
    );
    open(&mut harness, "text.pdf");
    wait_until(&mut harness, "the first page draws", |harness| {
        shows_page(harness, "Page 1 of 3")
    });
    let tab = active(&harness);
    // A kept tab stays open beside the next file you open.
    harness.state_mut().app_mut().keep_tab(tab);
    harness.get_by_role_and_label(Role::Toolbar, "PDF navigation");
    for label in [
        "Thumbnails",
        "Previous page",
        "Next page",
        "Zoom out",
        "Zoom in",
        "Fit width",
        "Previous match",
        "Next match",
    ] {
        harness.get_by_role_and_label(Role::Button, label);
    }

    harness
        .get_by_role_and_label(Role::Button, "Next page")
        .focus();
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    harness.run_steps(2);
    assert_eq!(page(&harness), 2);
    wait_until(&mut harness, "the second page draws", |harness| {
        shows_page(harness, "Page 2 of 3")
    });

    let field = harness.get_by_role_and_label(Role::TextInput, "Page number");
    field.focus();
    harness.run_steps(2);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    harness
        .get_by_role_and_label(Role::TextInput, "Page number")
        .type_text("3");
    harness.key_press(Key::Enter);
    harness.run_steps(2);
    assert_eq!(page(&harness), 3);

    harness
        .get_by_role_and_label(Role::Button, "Zoom in")
        .focus();
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    harness.run_steps(2);
    let zoom = harness.state().app().pdf(tab).unwrap().zoom();
    assert!(matches!(zoom, PdfZoom::Scale(_)), "{zoom:?}");

    harness
        .get_by_role_and_label(Role::TreeItem, "Plan")
        .click();
    harness.run_steps(4);
    assert_ne!(active(&harness), tab);
    harness.get_by_role_and_label(Role::Tab, "text.pdf").click();
    harness.run_steps(4);
    assert_eq!(active(&harness), tab);
    assert_eq!(page(&harness), 3);
    assert_eq!(harness.state().app().pdf(tab).unwrap().zoom(), zoom);
    assert_eq!(
        harness
            .get_by_role_and_label(Role::TextInput, "Page number")
            .value()
            .as_deref(),
        Some("3")
    );

    harness
        .get_by_role_and_label(Role::Button, "Fit width")
        .click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().app().pdf(tab).unwrap().zoom(),
        PdfZoom::FitWidth
    );
}

#[test]
fn a_search_from_the_field_moves_between_matches() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = workspace(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("text.pdf", &pdf("text.pdf"))],
    );
    open(&mut harness, "text.pdf");
    wait_until(&mut harness, "the PDF opens", |harness| {
        harness.query_by_label("of 3").is_some()
    });

    let field = harness.get_by_role_and_label(Role::TextInput, "Search PDF");
    field.focus();
    field.type_text("apple");
    harness.key_press(Key::Enter);
    wait_until(&mut harness, "the matches show", |harness| {
        harness.query_by_label("1 of 3").is_some()
    });
    assert_eq!(page(&harness), 1);
    harness.key_press(Key::Enter);
    harness.run_steps(2);
    assert_eq!(page(&harness), 3);
    harness.get_by_label("2 of 3");
    harness
        .get_by_role_and_label(Role::Button, "Previous match")
        .click();
    harness.run_steps(2);
    assert_eq!(page(&harness), 1);
}

#[test]
fn the_thumbnails_panel_lists_one_button_per_page_and_opens_the_page_you_pick() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = workspace(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("text.pdf", &pdf("text.pdf"))],
    );
    open(&mut harness, "text.pdf");
    wait_until(&mut harness, "the PDF opens", |harness| {
        harness.query_by_label("of 3").is_some()
    });
    assert!(harness.query_by_label("Page thumbnails").is_none());

    harness
        .get_by_role_and_label(Role::Button, "Thumbnails")
        .click();
    harness.run_steps(2);
    harness.get_by_label("Page thumbnails");
    for number in 1..=3 {
        harness.get_by_role_and_label(Role::Button, &format!("Go to page {number}"));
    }
    assert!(harness.query_by_label("Go to page 4").is_none());
    let tab = active(&harness);
    wait_until(&mut harness, "the thumbnails draw", |harness| {
        harness
            .state()
            .app()
            .pdf(tab)
            .unwrap()
            .thumbnail(3)
            .is_some()
    });

    harness
        .get_by_role_and_label(Role::Button, "Go to page 2")
        .click();
    harness.run_steps(2);
    assert_eq!(page(&harness), 2);
}

#[test]
fn a_locked_pdf_asks_for_its_password_and_says_when_its_wrong() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = workspace(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("locked.pdf", &pdf("locked.pdf"))],
    );
    open(&mut harness, "locked.pdf");
    wait_until(&mut harness, "the PDF asks for its password", |harness| {
        harness
            .query_by_role_and_label(Role::PasswordInput, "PDF password")
            .is_some()
    });

    let field = harness.get_by_role_and_label(Role::PasswordInput, "PDF password");
    field.focus();
    field.type_text("sesame");
    harness.key_press(Key::Enter);
    wait_until(
        &mut harness,
        "the PDF says the password is wrong",
        |harness| {
            harness
                .query_by_label("That password didn\u{2019}t open the PDF. Try again.")
                .is_some()
        },
    );

    let field = harness.get_by_role_and_label(Role::PasswordInput, "PDF password");
    assert_eq!(field.value().as_deref().unwrap_or_default(), "");
    field.focus();
    field.type_text("open sesame");
    harness
        .get_by_role_and_label(Role::Button, "Unlock")
        .click();
    let tab = active(&harness);
    wait_until(&mut harness, "the PDF opens", |harness| {
        harness.state().app().pdf(tab).unwrap().state() == &PdfState::Ready
    });
    harness.run_steps(2);
    harness.get_by_label("of 2");
}

#[test]
fn a_pdf_embedded_in_a_note_shows_its_page_and_opens_in_a_tab_at_that_page() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let note = "# Reading\n\nIntro\n\n![[text.pdf#page=2]]\n";
    let mut harness = workspace(
        app_data.path(),
        &folder,
        &[
            ("Docs/text.pdf", &pdf("text.pdf")),
            ("Reading.md", note.as_bytes()),
        ],
    );
    open(&mut harness, "Reading");
    let note_tab = active(&harness);

    wait_until(&mut harness, "the embed draws its page", |harness| {
        harness
            .query_by_role_and_label(Role::Button, "Open text.pdf in a tab")
            .is_some()
    });
    harness.get_by_role_and_label(Role::Image, "text.pdf#page=2");
    harness
        .get_by_role_and_label(Role::Button, "Open text.pdf in a tab")
        .click();
    harness.run_steps(4);

    let tab = active(&harness);
    assert_ne!(tab, note_tab);
    wait_until(&mut harness, "the PDF opens at page 2", |harness| {
        shows_page(harness, "Page 2 of 3")
    });
    assert_eq!(
        std::fs::read_to_string(folder.join("Reading.md")).unwrap(),
        note
    );
}

#[test]
fn an_image_opens_in_a_viewer_labelled_with_its_file_name() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = workspace(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("Photos/cat.png", &png(40, 30))],
    );
    harness
        .get_by_role_and_label(Role::TreeItem, "Photos")
        .click();
    harness.run_steps(4);
    open(&mut harness, "cat.png");

    wait_until(&mut harness, "the image draws", |harness| {
        harness
            .query_by_role_and_label(Role::Image, "cat.png")
            .is_some()
    });
    let tab = active(&harness);
    assert!(harness.state().app().image(tab).is_some());
}
