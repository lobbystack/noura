//! The file viewers through AccessKit: a PDF tab's toolbar, page and
//! thumbnails, its password form, a PDF embedded in a note, and the
//! image tab, each by its label and from the keyboard.

mod support;

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use app_core::{PdfState, PdfZoom, Platform, TabId};
use egui::{Event, Key, Modifiers, PointerButton, accesskit::Role};
use egui_kittest::{Harness, kittest::Queryable};
use support::{FakePicker, FakeSystem, create_workspace, harness_with, settle};

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
    workspace_on(
        app_data,
        folder,
        files,
        FakeSystem::new(Platform::current()),
    )
}

/// A workspace holding `files`, open in a window whose app asks `system`.
fn workspace_on(
    app_data: &Path,
    folder: &Path,
    files: &[(&str, &[u8])],
    system: Arc<FakeSystem>,
) -> Ui {
    create_workspace(app_data, "Garden", folder);
    for (name, bytes) in files {
        let path = folder.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let mut harness = harness_with(app_data, FakePicker::default(), system);
    settle(&mut harness);
    harness
}

/// Click at `pos`, in a frame of its own. The harness steps a quarter
/// second a frame, so two clicks in a row make a double-click, as a
/// person's do: the second click's frame reports a click and a
/// double-click.
fn click_at(harness: &mut Ui, pos: egui::Pos2) {
    let button = |pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    harness
        .input_mut()
        .events
        .extend([Event::PointerMoved(pos), button(true), button(false)]);
    harness.step();
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
fn a_page_that_cant_be_drawn_says_so_in_place_of_the_page() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = workspace(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("text.pdf", &pdf("text.pdf"))],
    );
    let failing = harness.state().app().fail_pdf_drawing_for_testing();

    open(&mut harness, "text.pdf");
    wait_until(&mut harness, "the page says it can't be drawn", |harness| {
        harness
            .query_by_label("Noura couldn\u{2019}t draw page 1.")
            .is_some()
    });
    assert!(harness.query_by_label("Rendering page 1\u{2026}").is_none());
    assert!(!shows_page(&harness, "Page 1 of 3"));

    // Another page asks for its drawing again.
    drop(failing);
    harness
        .get_by_role_and_label(Role::Button, "Next page")
        .click();
    wait_until(&mut harness, "page 2 draws", |harness| {
        shows_page(harness, "Page 2 of 3")
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

    // The field takes the keyboard when the form shows.
    harness.run_steps(2);
    let field = harness.get_by_role_and_label(Role::PasswordInput, "PDF password");
    assert!(field.is_focused());
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

    // And again after a wrong password, so you can type the next one.
    harness.run_steps(2);
    let field = harness.get_by_role_and_label(Role::PasswordInput, "PDF password");
    assert_eq!(field.value().as_deref().unwrap_or_default(), "");
    assert!(field.is_focused());
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
fn the_password_field_takes_the_127_characters_a_pdf_password_can_hold() {
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
    harness.run_steps(2);

    harness
        .get_by_role_and_label(Role::PasswordInput, "PDF password")
        .type_text(&"\u{e9}".repeat(300));
    harness.run_steps(2);

    let field = harness.get_by_role_and_label(Role::PasswordInput, "PDF password");
    let typed = field.value().unwrap_or_default();
    assert_eq!(typed.chars().count(), 127);
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

#[test]
fn an_image_too_large_for_a_texture_draws_scaled_down_to_fit() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = workspace(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("wide.png", &png(9000, 10)), ("tall.png", &png(10, 9000))],
    );
    // kittest's limit, which the desktop app's GPU sets at 8192.
    harness.input_mut().max_texture_side = Some(2048);

    for name in ["wide.png", "tall.png"] {
        open(&mut harness, name);
        wait_until(&mut harness, "the image draws", |harness| {
            harness.query_by_role_and_label(Role::Image, name).is_some()
        });
        let shown = harness.get_by_role_and_label(Role::Image, name).rect();
        // It keeps its shape: 900 to 1, fit to the tab.
        let (long, short) = (
            shown.width().max(shown.height()),
            shown.width().min(shown.height()),
        );
        assert!(long > 400.0 && short < 1.0, "{name} drew at {shown:?}");
    }
    let max_side = harness.ctx.input(|input| input.max_texture_side);
    let textures = harness.ctx.tex_manager();
    for (_, meta) in textures.read().allocated() {
        assert!(
            meta.size.iter().all(|side| (1..=max_side).contains(side)),
            "{} is {:?}, past the {max_side}-pixel limit",
            meta.name,
            meta.size
        );
    }
}

#[test]
fn a_click_selects_a_file_that_opens_in_no_tab_and_a_double_click_opens_it_once() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let system = FakeSystem::new(Platform::current());
    let mut harness = workspace_on(
        app_data.path(),
        &folder,
        &[("archive.zip", b"PK")],
        system.clone(),
    );
    let row = harness
        .get_by_role_and_label(Role::TreeItem, "archive.zip")
        .rect()
        .center();

    click_at(&mut harness, row);
    harness.run();

    assert!(
        harness
            .get_by_role_and_label(Role::TreeItem, "archive.zip")
            .is_focused()
    );
    assert!(harness.state().app().tabs().is_empty());
    assert_eq!(system.calls(), Vec::<String>::new());

    click_at(&mut harness, row);
    click_at(&mut harness, row);
    harness.run();

    let canonical = folder.canonicalize().unwrap();
    assert_eq!(
        system.calls(),
        [format!(
            "open file {}",
            canonical.join("archive.zip").display()
        )]
    );
    assert!(harness.state().app().tabs().is_empty());
}
