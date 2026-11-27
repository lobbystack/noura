//! The file viewers through AccessKit: a PDF tab's toolbar, its pages as
//! they scroll and the textures they draw, its thumbnails and password
//! form, a PDF embedded in a note, and the image tab, each by its label
//! and from the keyboard.

use crate::support;
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use app_core::{App, PAGE_GAP, PdfState, PdfZoom, Platform, TILE_HEIGHT, TabId};
use app_ui::NouraUi;
use egui::{Event, Key, Modifiers, PointerButton, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
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
    create_workspace(app_data, folder);
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

/// The drawn page labelled `label` shows with its top a gap below the top
/// of the pages' view, under the toolbar.
fn shows_page_at_top(harness: &Ui, label: &str) -> bool {
    let Some(page) = harness.query_by_role_and_label(Role::Image, label) else {
        return false;
    };
    let view = harness
        .get_by_role_and_label(Role::ScrollView, "Pages")
        .rect();
    // The gap above each page.
    (page.rect().top() - view.top() - PAGE_GAP).abs() < 1.0
}

/// The numbers of the drawn pages that show.
fn pages_shown(harness: &Ui) -> Vec<usize> {
    let count = harness
        .state()
        .app()
        .pdf(active(harness))
        .unwrap()
        .page_count();
    (1..=count)
        .filter(|page| shows_page(harness, &format!("Page {page} of {count}")))
        .collect()
}

/// Turn the mouse wheel over the pages `steps` times, `delta` points each.
fn wheel(harness: &mut Ui, delta: f32, steps: usize) {
    wheel_at(harness, egui::pos2(600.0, 400.0), delta, steps);
}

/// Turn the mouse wheel at `pos` `steps` times, `delta` points each.
fn wheel_at(harness: &mut Ui, pos: egui::Pos2, delta: f32, steps: usize) {
    harness.hover_at(pos);
    harness.run_steps(2);
    for _ in 0..steps {
        harness.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, delta),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: Modifiers::NONE,
        });
        harness.step();
    }
    harness.run_steps(4);
}

/// Press `key` in a frame of its own, then let the pages settle.
fn press(harness: &mut Ui, key: Key) {
    harness.key_press(key);
    harness.run_steps(4);
}

/// The textures the pages' tiles uploaded: each one's size in pixels.
fn tile_textures(harness: &Ui) -> Vec<[usize; 2]> {
    let textures = harness.ctx.tex_manager();
    let textures = textures.read();
    textures
        .allocated()
        .filter(|(_, meta)| meta.name == "pdf-tile")
        .map(|(_, meta)| meta.size)
        .collect()
}

#[test]
fn a_large_pdf_keeps_the_window_drawing_while_its_pages_render() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = workspace(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("large.pdf", &pdf("large.pdf"))],
    );
    let hold = harness.state().app().hold_pdf_rendering_for_testing();

    open(&mut harness, "large.pdf");
    // The page renders on the PDF's own thread, which the test holds: the
    // window keeps drawing frames meanwhile and says the page is coming.
    wait_until(&mut harness, "the first page says it renders", |harness| {
        harness.query_by_label("Rendering page 1\u{2026}").is_some()
    });
    for _ in 0..20 {
        let started = Instant::now();
        harness.step();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "a frame stalled"
        );
    }
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

    // Another zoom asks for its drawing again.
    drop(failing);
    harness
        .get_by_role_and_label(Role::Button, "Zoom in")
        .click();
    wait_until(&mut harness, "page 1 draws", |harness| {
        shows_page(harness, "Page 1 of 3")
    });
}

#[test]
fn the_pages_scroll_with_the_wheel_and_the_keyboard_and_the_toolbar_has_no_page_switcher() {
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
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());
    // A kept tab stays open beside the next file you open.
    harness.state_mut().app_mut().keep_tab(tab);
    harness.get_by_role_and_label(Role::Toolbar, "PDF navigation");
    for label in [
        "Thumbnails",
        "Zoom out",
        "Zoom in",
        "Fit width",
        "Previous match",
        "Next match",
    ] {
        harness.get_by_role_and_label(Role::Button, label);
    }
    for label in ["Previous page", "Next page"] {
        assert!(
            harness
                .query_by_role_and_label(Role::Button, label)
                .is_none()
        );
    }
    assert!(harness.query_by_label("Page number").is_none());
    assert!(harness.query_by_label("of 3").is_none());

    // The wheel scrolls from one page into the next.
    wheel(&mut harness, -300.0, 4);
    assert_eq!(page(&harness), 2);
    wait_until(&mut harness, "page 2 draws", |harness| {
        shows_page(harness, "Page 2 of 3")
    });
    assert!(pages_shown(&harness).contains(&2));

    // With the pages clicked, so no control has the keyboard, the keys
    // scroll them: End to the end, Home to the start, Page Down down.
    click_at(&mut harness, egui::pos2(600.0, 400.0));
    harness.run_steps(2);
    press(&mut harness, Key::End);
    assert_eq!(page(&harness), 3);
    wait_until(&mut harness, "page 3 draws", |harness| {
        shows_page(harness, "Page 3 of 3")
    });
    press(&mut harness, Key::Home);
    assert_eq!(page(&harness), 1);
    // Page 1 may have dropped its tiles while page 3 showed, and draws
    // again.
    wait_until(&mut harness, "page 1 draws at the top", |harness| {
        shows_page_at_top(harness, "Page 1 of 3")
    });
    press(&mut harness, Key::PageDown);
    press(&mut harness, Key::PageDown);
    assert_eq!(page(&harness), 2);

    harness
        .get_by_role_and_label(Role::Button, "Zoom in")
        .focus();
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    harness.run_steps(2);
    let zoom = harness.state().app().pdf(tab).unwrap().zoom();
    assert!(matches!(zoom, PdfZoom::Scale(_)), "{zoom:?}");
    // The keys leave the pages alone while a control has the keyboard.
    press(&mut harness, Key::End);
    assert_eq!(page(&harness), 2);
    let place = page(&harness);

    harness
        .get_by_role_and_label(Role::TreeItem, "Plan")
        .click();
    harness.run_steps(4);
    assert_ne!(active(&harness), tab);
    harness.get_by_role_and_label(Role::Tab, "text.pdf").click();
    harness.run_steps(4);
    assert_eq!(active(&harness), tab);
    assert_eq!(page(&harness), place);
    assert_eq!(harness.state().app().pdf(tab).unwrap().zoom(), zoom);
    assert!(pages_shown(&harness).contains(&place));

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
fn a_search_from_the_field_scrolls_between_matches() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = workspace(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("text.pdf", &pdf("text.pdf"))],
    );
    open(&mut harness, "text.pdf");
    wait_until(&mut harness, "the PDF opens", |harness| {
        shows_page(harness, "Page 1 of 3")
    });

    // The field searches as you type and shows the first match. An Enter
    // before the matches arrive would find none to move to, and one after
    // them moves on, so the test waits for them before it presses Enter.
    let field = harness.get_by_role_and_label(Role::TextInput, "Search PDF");
    field.focus();
    field.type_text("apple");
    let deadline = Instant::now() + Duration::from_secs(30);
    while harness.query_by_label("1 of 3").is_none() {
        if Instant::now() >= deadline {
            let app = harness.state().app();
            let pdf = app.pdf(active(&harness)).unwrap();
            panic!(
                "the matches never showed: state {:?}, search {:?}",
                pdf.state(),
                pdf.search()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
        harness.step();
    }
    assert_eq!(page(&harness), 1);
    harness.key_press(Key::Enter);
    harness.run_steps(2);
    assert_eq!(page(&harness), 3);
    harness.get_by_label("2 of 3");
    wait_until(&mut harness, "page 3 draws", |harness| {
        shows_page(harness, "Page 3 of 3")
    });
    harness
        .get_by_role_and_label(Role::Button, "Previous match")
        .click();
    harness.run_steps(2);
    assert_eq!(page(&harness), 1);
    wait_until(&mut harness, "page 1 draws at the top", |harness| {
        shows_page_at_top(harness, "Page 1 of 3")
    });
}

/// Mod on `platform`: ⌘ on macOS, Ctrl elsewhere.
fn command_on(platform: Platform) -> Modifiers {
    match platform {
        Platform::MacOs => Modifiers::MAC_CMD | Modifiers::COMMAND,
        Platform::Windows | Platform::Linux => Modifiers::CTRL | Modifiers::COMMAND,
    }
}

fn search_field(harness: &Ui) -> egui_kittest::Node<'_> {
    harness.get_by_role_and_label(Role::TextInput, "Search PDF")
}

/// Run frames until the search's status says `status`.
fn wait_for_status(harness: &mut Ui, status: &str) {
    wait_until(harness, &format!("the search says {status}"), |harness| {
        harness.query_by_label(status).is_some()
    });
}

#[test]
fn mod_f_gives_the_search_field_the_keyboard_and_selects_what_it_holds() {
    for platform in [Platform::MacOs, Platform::Linux, Platform::Windows] {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let mut harness = workspace_on(
            app_data.path(),
            &parent.path().join("Garden"),
            &[("text.pdf", &pdf("text.pdf"))],
            FakeSystem::new(platform),
        );
        open(&mut harness, "text.pdf");
        wait_until(&mut harness, "the PDF opens", |harness| {
            shows_page(harness, "Page 1 of 3")
        });
        let command = command_on(platform);

        // From the pages, Mod-F moves the keyboard to the field.
        harness
            .get_by_role_and_label(Role::ScrollView, "Pages")
            .click();
        harness.run_steps(2);
        assert!(!search_field(&harness).is_focused(), "{platform:?}");
        harness.key_press_modifiers(command, Key::F);
        harness.run_steps(2);
        assert!(search_field(&harness).is_focused(), "{platform:?}");
        harness.event(Event::Text("apple".to_owned()));
        wait_for_status(&mut harness, "1 of 3");

        // With the field holding a search, Mod-F selects it, so what you
        // type next replaces it.
        harness
            .get_by_role_and_label(Role::ScrollView, "Pages")
            .click();
        harness.run_steps(2);
        assert!(!search_field(&harness).is_focused(), "{platform:?}");
        harness.key_press_modifiers(command, Key::F);
        harness.run_steps(2);
        assert!(search_field(&harness).is_focused(), "{platform:?}");
        harness.event(Event::Text("third".to_owned()));
        harness.run_steps(2);
        assert_eq!(
            search_field(&harness).value().as_deref(),
            Some("third"),
            "{platform:?}"
        );
        wait_for_status(&mut harness, "1 of 1");

        // Mod-F in the field selects its text too.
        harness.key_press_modifiers(command, Key::F);
        harness.run_steps(2);
        harness.event(Event::Text("apple".to_owned()));
        harness.run_steps(2);
        assert_eq!(search_field(&harness).value().as_deref(), Some("apple"));
        wait_for_status(&mut harness, "1 of 3");
    }
}

/// The marks over the search's matches that show, each with whether it's
/// the current match, in the order they draw.
fn marks(harness: &Ui) -> Vec<(egui::Rect, bool)> {
    harness
        .query_all_by_role_and_label(Role::Mark, "Match")
        .map(|mark| {
            let selected = mark.accesskit_node().is_selected() == Some(true);
            (mark.rect(), selected)
        })
        .collect()
}

/// `mark` on the page `page` lays out at, in the page's own points: its
/// left, top, width and height on a page 612 points wide.
fn on_page(mark: egui::Rect, page: egui::Rect) -> [f32; 4] {
    let scale = 612.0 / page.width();
    [
        (mark.left() - page.left()) * scale,
        (mark.top() - page.top()) * scale,
        mark.width() * scale,
        mark.height() * scale,
    ]
}

fn near(actual: [f32; 4], expected: [f32; 4]) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(actual, expected)| (actual - expected).abs() <= 2.0)
}

#[test]
fn the_pages_mark_every_match_and_the_current_one_apart_at_any_zoom() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = workspace(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("text.pdf", &pdf("text.pdf"))],
    );
    open(&mut harness, "text.pdf");
    wait_until(&mut harness, "the PDF opens", |harness| {
        shows_page(harness, "Page 1 of 3")
    });
    assert!(marks(&harness).is_empty());

    let field = search_field(&harness);
    field.focus();
    field.type_text("apple");
    wait_for_status(&mut harness, "1 of 3");
    harness.run_steps(2);

    // "apple" on the first page's second line: 24-point Helvetica 108
    // points in, on a baseline 122 points down.
    let page = harness
        .get_by_role_and_label(Role::Image, "Page 1 of 3")
        .rect();
    let shown = marks(&harness);
    assert_eq!(shown.len(), 1, "{shown:?}");
    let (mark, current) = shown[0];
    assert!(current);
    assert!(page.contains_rect(mark));
    assert!(
        near(on_page(mark, page), [108.0, 102.8, 58.7, 24.0]),
        "{:?}",
        on_page(mark, page)
    );

    // The third page holds the next two: the current one, then the
    // other.
    harness.key_press(Key::Enter);
    wait_for_status(&mut harness, "2 of 3");
    wait_until(&mut harness, "page 3 draws", |harness| {
        shows_page(harness, "Page 3 of 3")
    });
    let page = harness
        .get_by_role_and_label(Role::Image, "Page 3 of 3")
        .rect();
    let on_three: Vec<(egui::Rect, bool)> = marks(&harness)
        .into_iter()
        .filter(|(mark, _)| page.contains_rect(*mark))
        .collect();
    let selected: Vec<bool> = on_three.iter().map(|(_, current)| *current).collect();
    assert_eq!(selected, [true, false]);
    assert!(near(
        on_page(on_three[0].0, page),
        [72.0, 102.8, 61.4, 24.0]
    ));
    assert!(near(
        on_page(on_three[1].0, page),
        [186.7, 102.8, 58.7, 24.0]
    ));
    harness
        .get_by_role_and_label(Role::Button, "Next match")
        .click();
    harness.run_steps(2);
    let selected: Vec<bool> = marks(&harness)
        .into_iter()
        .filter(|(mark, _)| page.contains_rect(*mark))
        .map(|(_, current)| current)
        .collect();
    assert_eq!(selected, [false, true]);

    // A step smaller, the marks shrink with the page and stay on their
    // words.
    let before = on_three[1].0.width();
    harness
        .get_by_role_and_label(Role::Button, "Zoom out")
        .click();
    harness.run_steps(4);
    let page = harness
        .get_by_role_and_label(Role::Image, "Page 3 of 3")
        .rect();
    let on_three: Vec<egui::Rect> = marks(&harness)
        .into_iter()
        .map(|(mark, _)| mark)
        .filter(|mark| page.contains_rect(*mark))
        .collect();
    assert_eq!(on_three.len(), 2);
    assert!(on_three[1].width() < before);
    assert!(near(on_page(on_three[1], page), [186.7, 102.8, 58.7, 24.0]));

    // Clearing the search clears the marks.
    let field = search_field(&harness);
    field.focus();
    harness.key_press_modifiers(command_on(Platform::current()), Key::A);
    harness.key_press(Key::Backspace);
    harness.run_steps(4);
    assert!(marks(&harness).is_empty());
}

#[test]
fn mod_f_leaves_the_keyboard_with_another_text_field() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let platform = Platform::current();
    let mut harness = workspace_on(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("text.pdf", &pdf("text.pdf"))],
        FakeSystem::new(platform),
    );
    open(&mut harness, "text.pdf");
    wait_until(&mut harness, "the PDF opens", |harness| {
        shows_page(harness, "Page 1 of 3")
    });

    // F2 on the file's row renames it, in a field of the tree's own.
    harness
        .get_by_role_and_label(Role::TreeItem, "text.pdf")
        .focus();
    harness.run_steps(2);
    press(&mut harness, Key::F2);
    let renaming = |harness: &Ui| {
        harness
            .get_by_role_and_label(Role::TextInput, "New name for text.pdf")
            .is_focused()
    };
    assert!(renaming(&harness));
    harness.key_press_modifiers(command_on(platform), Key::F);
    harness.run_steps(2);
    assert!(renaming(&harness));
    assert!(!search_field(&harness).is_focused());
}

#[test]
fn mod_f_leaves_a_locked_pdfs_password_field_the_keyboard() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let platform = Platform::current();
    let mut harness = workspace_on(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("locked.pdf", &pdf("locked.pdf"))],
        FakeSystem::new(platform),
    );
    open(&mut harness, "locked.pdf");
    wait_until(&mut harness, "the PDF asks for its password", |harness| {
        harness
            .query_by_role_and_label(Role::PasswordInput, "PDF password")
            .is_some()
    });
    harness.run_steps(2);
    let password = |harness: &Ui| {
        harness
            .get_by_role_and_label(Role::PasswordInput, "PDF password")
            .is_focused()
    };
    assert!(password(&harness));

    harness.key_press_modifiers(command_on(platform), Key::F);
    harness.run_steps(2);
    assert!(password(&harness));
    assert!(
        harness
            .query_by_role_and_label(Role::TextInput, "Search PDF")
            .is_none()
    );
    // What you type still lands in the password field.
    harness.event(Event::Text("open sesame".to_owned()));
    harness.key_press(Key::Enter);
    let tab = active(&harness);
    wait_until(&mut harness, "the PDF opens", |harness| {
        harness.state().app().pdf(tab).unwrap().state() == &PdfState::Ready
    });
}

#[test]
fn the_thumbnails_panel_lists_one_button_per_page_and_scrolls_to_the_page_you_pick() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = workspace(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("text.pdf", &pdf("text.pdf"))],
    );
    open(&mut harness, "text.pdf");
    wait_until(&mut harness, "the PDF opens", |harness| {
        shows_page(harness, "Page 1 of 3")
    });
    assert!(harness.query_by_label("Page thumbnails").is_none());

    harness
        .get_by_role_and_label(Role::Button, "Thumbnails")
        .click();
    harness.run_steps(2);
    harness.get_by_label("Page thumbnails");
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());
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
    wait_until(&mut harness, "page 2 draws at the top", |harness| {
        shows_page_at_top(harness, "Page 2 of 3")
    });
}

/// The thumbnail of page `page` shows whole in the thumbnails panel, which
/// runs as far down as the pages beside it.
fn shows_thumbnail(harness: &Ui, page: usize) -> bool {
    let label = format!("Go to page {page}");
    let Some(thumbnail) = harness.query_by_role_and_label(Role::Button, &label) else {
        return false;
    };
    let thumbnail = thumbnail.rect();
    let pages = harness
        .get_by_role_and_label(Role::ScrollView, "Pages")
        .rect();
    thumbnail.top() >= pages.top() - 0.5 && thumbnail.bottom() <= pages.bottom() + 0.5
}

/// The middle of the thumbnails panel, which shows the thumbnail of page
/// `page`.
fn thumbnails_middle(harness: &Ui, page: usize) -> egui::Pos2 {
    let pages = harness
        .get_by_role_and_label(Role::ScrollView, "Pages")
        .rect();
    let thumbnail = harness
        .get_by_role_and_label(Role::Button, &format!("Go to page {page}"))
        .rect();
    egui::pos2(thumbnail.center().x, pages.center().y)
}

#[test]
fn the_thumbnails_panel_follows_the_page_that_shows_and_leaves_your_own_scroll_alone() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = workspace(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("large.pdf", &pdf("large.pdf"))],
    );
    open(&mut harness, "large.pdf");
    let tab = active(&harness);
    wait_until(&mut harness, "the first page draws", |harness| {
        shows_page(harness, "Page 1 of 400")
    });
    harness
        .get_by_role_and_label(Role::Button, "Thumbnails")
        .click();
    harness.run_steps(4);
    assert!(shows_thumbnail(&harness, 1));

    // Scrolling the pages to the end brings the last thumbnail into view.
    click_at(&mut harness, egui::pos2(600.0, 400.0));
    harness.run_steps(2);
    press(&mut harness, Key::End);
    assert_eq!(page(&harness), 400);
    assert!(shows_thumbnail(&harness, 400));
    assert!(!shows_thumbnail(&harness, 1));

    // Scrolling the panel yourself moves it away from the page that shows,
    // and it stays there while that page does.
    let panel = thumbnails_middle(&harness, 400);
    wheel_at(&mut harness, panel, 2000.0, 6);
    harness.run_steps(10);
    assert_eq!(page(&harness), 400);
    assert!(!shows_thumbnail(&harness, 400));
    let scrolled = (1..400)
        .find(|page| shows_thumbnail(&harness, *page))
        .expect("a thumbnail shows");
    harness.run_steps(10);
    assert!(shows_thumbnail(&harness, scrolled), "the panel moved back");

    // Scrolling the pages to another page brings its thumbnail back.
    wheel(&mut harness, 400.0, 4);
    let now = page(&harness);
    assert!(now < 400, "the pages didn't scroll");
    assert!(shows_thumbnail(&harness, now));

    // So does a jump to a page, as a link makes.
    harness.state_mut().app_mut().pdf_go_to(tab, 120);
    harness.run_steps(4);
    assert_eq!(page(&harness), 120);
    assert!(shows_thumbnail(&harness, 120));

    // And Fit width, after you scrolled the panel away.
    harness
        .get_by_role_and_label(Role::Button, "Zoom in")
        .click();
    harness.run_steps(4);
    wheel_at(&mut harness, panel, -3000.0, 6);
    harness.run_steps(4);
    assert!(!shows_thumbnail(&harness, page(&harness)));
    harness
        .get_by_role_and_label(Role::Button, "Fit width")
        .click();
    harness.run_steps(4);
    assert!(shows_thumbnail(&harness, page(&harness)));
}

#[test]
fn a_tall_page_draws_a_texture_pixel_for_each_screen_pixel_on_a_2x_screen() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), &folder);
    std::fs::write(folder.join("tall.pdf"), pdf("tall.pdf")).unwrap();
    let app = App::launch_with(app_data.path(), FakeSystem::new(Platform::current()));
    let ui = NouraUi::new(app, Box::new(FakePicker::default()));
    let mut harness = support::builder()
        .with_pixels_per_point(2.0)
        .allow_missing_glyphs()
        .build_ui_state(|ui, state: &mut NouraUi| state.ui(ui), ui);
    settle(&mut harness);

    open(&mut harness, "tall.pdf");
    let tab = active(&harness);
    wait_until(&mut harness, "what shows draws", |harness| {
        let app = harness.state().app();
        let pdf = app.pdf(tab).unwrap();
        shows_page(harness, "Page 1 of 2")
            && pdf
                .tiles(1)
                .filter(|tile| tile.key.density() == pdf.density())
                .count()
                >= 2
    });
    harness.run_steps(2);

    // The page shows hundreds of points wide and 14,000 × its scale tall.
    let page = harness
        .get_by_role_and_label(Role::Image, "Page 1 of 2")
        .rect();
    assert!(page.width() > 400.0, "{page:?}");
    let textures = tile_textures(&harness);
    assert!(!textures.is_empty());
    for [width, height] in &textures {
        // Two texture pixels across each point, as the screen has.
        let across = *width as f32 / page.width();
        assert!((across - 2.0).abs() < 0.01, "{width} pixels for {page:?}");
        assert!(*height <= TILE_HEIGHT as usize);
    }
    // Only the tiles near the view upload, a few of the page's 30 rows.
    assert!(textures.len() <= 4, "{textures:?}");
}

/// A window with `text.pdf` and `Plan.md` open in kept tabs, the PDF's
/// active and drawn. Returns the PDF's tab and the note's.
fn pdf_and_note(app_data: &Path, folder: &Path) -> (Ui, TabId, TabId) {
    let mut harness = workspace(
        app_data,
        folder,
        &[("text.pdf", &pdf("text.pdf")), ("Plan.md", b"# Plan\n")],
    );
    let app = harness.state_mut().app_mut();
    app.open_file("Plan.md", app_core::OpenAs::Kept);
    let note = app.active_tab().unwrap().id();
    app.open_file("text.pdf", app_core::OpenAs::Kept);
    let tab = app.active_tab().unwrap().id();
    wait_until(&mut harness, "the first page draws", |harness| {
        let pdf = harness.state().app().pdf(tab).unwrap();
        shows_page(harness, "Page 1 of 3")
            && pdf.tiles(1).any(|tile| tile.key.density() == pdf.density())
    });
    (harness, tab, note)
}

fn zoom(harness: &Ui, tab: TabId) -> PdfZoom {
    harness.state().app().pdf(tab).unwrap().zoom()
}

#[test]
fn a_pinch_over_the_pages_zooms_them_at_the_pointer() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let (mut harness, tab, _) = pdf_and_note(app_data.path(), &parent.path().join("Garden"));
    let before = harness
        .get_by_role_and_label(Role::Image, "Page 1 of 3")
        .rect();
    // The pointer rests a third of the way across page 1 and two fifths
    // of the way down.
    let pointer = before.min + before.size() * egui::vec2(0.3, 0.4);
    harness.hover_at(pointer);
    harness.run_steps(2);

    // The fingers spread over six frames, and the pages grow smoothly.
    let mut widths = vec![before.width()];
    for _ in 0..6 {
        harness.event(Event::Zoom(1.05));
        harness.step();
        harness.step();
        widths.push(
            harness
                .get_by_role_and_label(Role::Image, "Page 1 of 3")
                .rect()
                .width(),
        );
    }
    harness.run_steps(2);
    for pair in widths.windows(2) {
        let grew = pair[1] / pair[0];
        assert!((grew - 1.05).abs() < 0.001, "{widths:?}");
    }
    let PdfZoom::Scale(_) = zoom(&harness, tab) else {
        panic!("a pinch zooms to a scale");
    };
    // The place on the page under the pointer stays under it.
    let after = harness
        .get_by_role_and_label(Role::Image, "Page 1 of 3")
        .rect();
    let place = after.min + after.size() * egui::vec2(0.3, 0.4);
    assert!((place - pointer).length() < 1.0, "{place:?} {pointer:?}");
    // The interface keeps its size.
    assert_eq!(harness.ctx.zoom_factor(), 1.0);

    // Once the pinch settles, the page draws a texture pixel for each of
    // the screen's at its new size.
    wait_until(&mut harness, "the new size draws", |harness| {
        let pdf = harness.state().app().pdf(tab).unwrap();
        pdf.settles_in().is_none() && pdf.tiles(1).any(|tile| tile.key.density() == pdf.density())
    });
    harness.run_steps(2);
    let page = harness
        .get_by_role_and_label(Role::Image, "Page 1 of 3")
        .rect();
    let widest = tile_textures(&harness)
        .iter()
        .map(|[width, _]| *width)
        .max()
        .unwrap();
    assert!(
        (widest as f32 - page.width()).abs() <= 1.0,
        "{widest} for {page:?}"
    );
}

#[test]
fn command_equals_minus_and_0_zoom_a_pdf_tab_and_the_interface_elsewhere() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let (mut harness, tab, note) = pdf_and_note(app_data.path(), &parent.path().join("Garden"));
    let key = |harness: &mut Ui, key| {
        harness.key_press_modifiers(Modifiers::COMMAND, key);
        harness.run_steps(2);
    };

    // As in Preview: Command-0 shows the pages at their own size, and
    // Command-= and Command-minus step their zoom.
    key(&mut harness, Key::Num0);
    assert_eq!(zoom(&harness, tab), PdfZoom::Scale(1.0));
    assert!(harness.query_by_label("100%").is_some());
    key(&mut harness, Key::Equals);
    assert_eq!(zoom(&harness, tab), PdfZoom::Scale(1.1));
    key(&mut harness, Key::Minus);
    key(&mut harness, Key::Minus);
    assert_eq!(zoom(&harness, tab), PdfZoom::Scale(0.9));
    assert!(harness.query_by_label("90%").is_some());
    assert_eq!(harness.ctx.zoom_factor(), 1.0);

    // In a note, they zoom the interface, and leave the PDF alone.
    harness.state_mut().app_mut().activate_tab(note);
    harness.run_steps(2);
    key(&mut harness, Key::Equals);
    assert_eq!(harness.ctx.zoom_factor(), 1.1);
    key(&mut harness, Key::Num0);
    assert_eq!(harness.ctx.zoom_factor(), 1.0);
    assert_eq!(zoom(&harness, tab), PdfZoom::Scale(0.9));
}

#[test]
fn the_interface_zoom_leaves_a_pdf_pages_size_on_the_screen_alone() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let (mut harness, tab, note) = pdf_and_note(app_data.path(), &parent.path().join("Garden"));
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Num0);
    harness.run_steps(4);
    // The page's width on the screen, in pixels.
    let pixels = |harness: &Ui| {
        harness
            .get_by_role_and_label(Role::Image, "Page 1 of 3")
            .rect()
            .width()
            * harness.ctx.pixels_per_point()
    };
    let before = pixels(&harness);
    assert_eq!(before, 612.0 * harness.ctx.pixels_per_point());

    // Zoom the interface to 125% from the note, then come back.
    harness.state_mut().app_mut().activate_tab(note);
    harness.run_steps(2);
    for _ in 0..2 {
        harness.key_press_modifiers(Modifiers::COMMAND, Key::Equals);
        harness.run_steps(2);
    }
    assert_eq!(harness.ctx.zoom_factor(), 1.25);
    harness.state_mut().app_mut().activate_tab(tab);
    harness.run_steps(4);

    // The page shows as many pixels wide, at 100%, from the same tiles,
    // each still a texture pixel to a screen pixel.
    assert!(
        (pixels(&harness) - before).abs() < 0.01,
        "{}",
        pixels(&harness)
    );
    assert!(harness.query_by_label("100%").is_some());
    let widest = tile_textures(&harness)
        .iter()
        .map(|[width, _]| *width)
        .max()
        .unwrap();
    assert!((widest as f32 - before).abs() <= 1.0, "{widest}");
}

#[test]
fn a_long_pdf_draws_only_the_pages_near_the_view() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = workspace(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("large.pdf", &pdf("large.pdf"))],
    );
    open(&mut harness, "large.pdf");
    let tab = active(&harness);
    wait_until(&mut harness, "the first page draws", |harness| {
        shows_page(harness, "Page 1 of 400")
    });

    click_at(&mut harness, egui::pos2(600.0, 400.0));
    harness.run_steps(2);
    press(&mut harness, Key::End);
    assert_eq!(page(&harness), 400);
    wait_until(&mut harness, "the last page draws", |harness| {
        shows_page(harness, "Page 400 of 400")
    });
    harness.run_steps(2);
    let shown = pages_shown(&harness);
    assert!(shown.iter().all(|page| *page >= 398), "{shown:?}");
    let app = harness.state().app();
    let pdf = app.pdf(tab).unwrap();
    let kept: Vec<usize> = (1..=400)
        .filter(|page| pdf.tiles(*page).next().is_some())
        .collect();
    assert!(kept.iter().all(|page| *page >= 390), "{kept:?}");
    assert!(tile_textures(&harness).len() <= 8);
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
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());
    field.type_text("sesame");
    harness.key_press(Key::Enter);
    wait_until(
        &mut harness,
        "the PDF says the password is wrong",
        |harness| harness.query_by_label("Wrong password").is_some(),
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
    wait_until(&mut harness, "the first page draws", |harness| {
        shows_page(harness, "Page 1 of 2")
    });
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
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());
    harness
        .get_by_role_and_label(Role::Button, "Open text.pdf in a tab")
        .click();
    harness.run_steps(4);

    let tab = active(&harness);
    assert_ne!(tab, note_tab);
    wait_until(&mut harness, "the PDF opens at page 2", |harness| {
        shows_page_at_top(harness, "Page 2 of 3")
    });
    assert_eq!(page(&harness), 2);
    assert_eq!(
        std::fs::read_to_string(folder.join("Reading.md")).unwrap(),
        note
    );
}

/// The textures a note's PDF embeds uploaded: each one's size in pixels.
fn embed_textures(harness: &Ui) -> Vec<[usize; 2]> {
    let textures = harness.ctx.tex_manager();
    let textures = textures.read();
    textures
        .allocated()
        .filter(|(_, meta)| meta.name == "pdf-embed-tile")
        .map(|(_, meta)| meta.size)
        .collect()
}

#[test]
fn a_tall_page_embedded_in_a_note_draws_a_texture_pixel_for_each_screen_pixel_on_a_2x_screen() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), &folder);
    std::fs::write(folder.join("tall.pdf"), pdf("tall.pdf")).unwrap();
    std::fs::write(
        folder.join("Saved page.md"),
        "# Saved page\n\nThe article, as saved.\n\n![[tall.pdf]]\n\nAfter the page.\n",
    )
    .unwrap();
    let app = App::launch_with(app_data.path(), FakeSystem::new(Platform::current()));
    let ui = NouraUi::new(app, Box::new(FakePicker::default()));
    let mut harness = support::builder()
        .with_pixels_per_point(2.0)
        .allow_missing_glyphs()
        .build_ui_state(|ui, state: &mut NouraUi| state.ui(ui), ui);
    settle(&mut harness);

    open(&mut harness, "Saved page");
    wait_until(&mut harness, "the embed's tiles upload", |harness| {
        !embed_textures(harness).is_empty()
    });
    harness.run_steps(2);

    // The page lays out as wide as an A4 page would in the view, far
    // taller than the view, rather than shrinking to fit it.
    let view = harness.ctx.viewport_rect();
    let embed = harness
        .get_by_role_and_label(Role::Image, "tall.pdf")
        .rect();
    assert!(embed.width() > 300.0, "{embed:?}");
    let shape = embed.height() / embed.width();
    assert!((shape - 14000.0 / 612.0).abs() < 0.01, "{embed:?}");
    assert!(embed.height() > 10.0 * view.height(), "{embed:?}");
    let textures = embed_textures(&harness);
    for [width, height] in &textures {
        // Two texture pixels across each point, as the screen has.
        let across = *width as f32 / embed.width();
        assert!((across - 2.0).abs() < 0.01, "{width} pixels for {embed:?}");
        assert!(*height <= TILE_HEIGHT as usize);
    }
    // Only the tiles in view upload.
    assert!(textures.len() <= 3, "{textures:?}");

    // Scroll the note far down the page: the textures stay a few, and
    // `app-core` keeps the tiles near the view alone.
    let middle = egui::pos2(view.center().x, view.center().y + 100.0);
    for _ in 0..30 {
        wheel_at(&mut harness, middle, -400.0, 1);
    }
    // The note scrolled to the end of the page, thousands of points down.
    let embed = harness
        .get_by_role_and_label(Role::Image, "tall.pdf")
        .rect();
    assert!(embed.top() < -5000.0, "{embed:?}");
    assert!(embed.bottom() > 0.0, "{embed:?}");
    wait_until(&mut harness, "the tiles far down upload", |harness| {
        let embed = harness
            .get_by_role_and_label(Role::Image, "tall.pdf")
            .rect();
        let textures = embed_textures(harness);
        !textures.is_empty()
            && textures
                .iter()
                .all(|[width, _]| (*width as f32 / embed.width() - 2.0).abs() < 0.01)
    });
    harness.run_steps(4);
    assert!(embed_textures(&harness).len() <= 3);
    let tab = active(&harness);
    let links = harness.state().app().note_links(tab).unwrap();
    let bytes = links.pdf_embed_tile_bytes_for_testing();
    // Five views of the page, 2 pixels a point, at most.
    let five_views = 5.0 * view.height() * 2.0 * embed.width() * 2.0 * 4.0;
    assert!(bytes as f32 <= five_views, "{bytes} bytes");
}

#[test]
fn an_embed_of_a_locked_pdf_says_so_and_opens_in_a_tab_that_asks_for_the_password() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let note = "# Reading\n\n![[locked.pdf#page=2]]\n";
    let mut harness = workspace(
        app_data.path(),
        &folder,
        &[
            ("locked.pdf", &pdf("locked.pdf")),
            ("Reading.md", note.as_bytes()),
        ],
    );
    open(&mut harness, "Reading");
    let note_tab = active(&harness);

    wait_until(
        &mut harness,
        "the embed says the PDF is locked",
        |harness| {
            harness
                .query_by_label("Locked PDF. Open it in a tab to unlock it.")
                .is_some()
        },
    );
    harness
        .get_by_role_and_label(Role::Button, "Open locked.pdf in a tab")
        .click();
    harness.run_steps(4);

    assert_ne!(active(&harness), note_tab);
    wait_until(&mut harness, "the tab asks for the password", |harness| {
        harness
            .query_by_role_and_label(Role::PasswordInput, "PDF password")
            .is_some()
    });
    assert_eq!(
        std::fs::read_to_string(folder.join("Reading.md")).unwrap(),
        note
    );
}

#[test]
fn a_click_on_a_page_link_in_a_note_opens_the_pdf_at_that_page() {
    let note = "See [the summary](Docs/text.pdf#page=2), [the end](text.pdf#page=9) \
                and [the start](text.pdf#page=x).\n";
    for (needle, number) in [("the summary", 2), ("the end", 3), ("the start", 1)] {
        let label = &format!("Page {number} of 3");
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Garden");
        let mut harness = workspace(
            app_data.path(),
            &folder,
            &[
                ("Docs/text.pdf", &pdf("text.pdf")),
                ("Reading.md", note.as_bytes()),
            ],
        );
        open(&mut harness, "Reading");

        let pos = support::point_at(&harness, needle, 1);
        harness.event(Event::PointerMoved(pos));
        harness.run_steps(3);
        for pressed in [true, false] {
            harness.event(Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
            harness.run_steps(1);
        }
        harness.run_steps(4);

        let app = harness.state().app();
        let tab = app.active_tab().expect("the link opened a tab");
        assert_eq!(
            tab.file().map(|file| file.relative_path.as_str()),
            Some("Docs/text.pdf"),
            "{needle:?}"
        );
        // The PDF opens scrolled to the page's top.
        wait_until(&mut harness, label, |harness| {
            shows_page_at_top(harness, label)
        });
        assert_eq!(page(&harness), number, "{needle:?}");
        assert_eq!(
            std::fs::read_to_string(folder.join("Reading.md")).unwrap(),
            note
        );
    }
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
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());
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
        // It keeps its shape, 900 to 1, fit to the tab, and at least a
        // pixel across.
        let (long, short) = (
            shown.width().max(shown.height()),
            shown.width().min(shown.height()),
        );
        assert!(long > 400.0 && short <= 1.0, "{name} drew at {shown:?}");
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
