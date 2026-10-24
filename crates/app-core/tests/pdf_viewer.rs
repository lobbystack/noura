//! PDF tabs, seam 1: a PDF from the tree opens in a tab with its page
//! count, and the tab moves through its pages, zooms, searches its text,
//! lists its thumbnails and unlocks it, while the work runs off the
//! caller's thread.

mod support;

use std::time::{Duration, Instant};

use app_core::{App, EMBED_WIDTH, EmbedProblem, OpenAs, PdfEmbedState, PdfState, PdfZoom, TabId};
use support::{Fixture, open_from_tree};

/// The bytes of the fixture PDF `name`.
pub fn pdf(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pdf")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{name} can't be read: {error}"))
}

/// Poll until `done` holds.
fn wait_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll();
        if done(app) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn state(app: &App, tab: TabId) -> PdfState {
    app.pdf(tab).expect("the tab shows a PDF").state().clone()
}

/// Open the PDF at `path` from the tree and wait until it shows.
fn open_ready(app: &mut App, path: &str) -> TabId {
    let tab = open_from_tree(app, path);
    wait_until(app, "the PDF opens", |app| {
        app.pdf(tab).map(|pdf| pdf.state().clone()) == Some(PdfState::Ready)
    });
    tab
}

fn page(app: &App, tab: TabId) -> usize {
    app.pdf(tab).unwrap().page()
}

#[test]
fn a_pdf_from_the_tree_opens_in_a_tab_with_its_page_count() {
    let fixture = Fixture::new(&[
        ("Docs/text.pdf", &pdf("text.pdf")),
        ("large.pdf", &pdf("large.pdf")),
    ]);
    let mut app = fixture.app();

    let tab = open_from_tree(&mut app, "Docs/text.pdf");
    // Opening reads and parses nothing on this thread.
    assert_eq!(state(&app, tab), PdfState::Opening);
    wait_until(&mut app, "the PDF opens", |app| {
        app.pdf(tab).unwrap().state() == &PdfState::Ready
    });
    let view = app.pdf(tab).unwrap();
    assert_eq!(view.page_count(), 3);
    assert_eq!(view.page(), 1);
    assert_eq!(view.page_size(1), Some([612.0, 792.0]));

    let large = open_ready(&mut app, "large.pdf");
    assert_eq!(app.pdf(large).unwrap().page_count(), 400);
}

#[test]
fn a_page_renders_off_the_callers_thread_at_the_size_the_view_asks_for() {
    let fixture = Fixture::new(&[("large.pdf", &pdf("large.pdf"))]);
    let mut app = fixture.app();
    let tab = open_ready(&mut app, "large.pdf");

    let hold = app.hold_pdf_rendering_for_testing();
    app.pdf_fit_width(tab);
    let asked = Instant::now();
    app.size_pdf(tab, 306.0, 2.0);
    // The page renders on the PDF's own thread, which the test holds, so
    // the call comes back at once with nothing drawn.
    assert!(asked.elapsed() < Duration::from_secs(1));
    for _ in 0..20 {
        app.poll();
        assert!(app.pdf(tab).unwrap().picture().is_none());
        std::thread::sleep(Duration::from_millis(5));
    }

    drop(hold);
    wait_until(&mut app, "the page renders", |app| {
        app.pdf(tab).unwrap().picture().is_some()
    });
    let picture = app.pdf(tab).unwrap().picture().unwrap();
    assert_eq!(picture.page, 1);
    // Fit width: 306 points for a 612-point page is half size, at two
    // pixels a point.
    assert_eq!((picture.width, picture.height), (612, 792));
    assert_eq!(picture.rgba.len(), 612 * 792 * 4);
}

#[test]
fn a_file_that_is_no_pdf_says_noura_cant_read_it() {
    let fixture = Fixture::new(&[("scan.pdf", b"%PDF-1.7\nnot really")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "scan.pdf");

    wait_until(&mut app, "the PDF fails", |app| {
        matches!(app.pdf(tab).unwrap().state(), PdfState::Failed { .. })
    });
    assert_eq!(
        state(&app, tab),
        PdfState::Failed {
            message: "Noura couldn't read this PDF. It may be damaged.".to_owned()
        }
    );
}

#[test]
fn a_pdf_with_no_pages_says_so() {
    let fixture = Fixture::new(&[("empty.pdf", &pdf("empty.pdf"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "empty.pdf");

    wait_until(&mut app, "the PDF fails", |app| {
        matches!(app.pdf(tab).unwrap().state(), PdfState::Failed { .. })
    });
    assert_eq!(
        state(&app, tab),
        PdfState::Failed {
            message: "This PDF has no pages.".to_owned()
        }
    );
}

/// The picture `picture` holds at least a pixel on each side, and a pixel
/// of bytes for each.
fn assert_drawn(picture: &app_core::PdfPicture, what: &str) {
    assert!(
        picture.width > 0 && picture.height > 0,
        "{what} drew {} by {} pixels",
        picture.width,
        picture.height
    );
    assert_eq!(
        picture.rgba.len(),
        picture.width as usize * picture.height as usize * 4,
        "{what}"
    );
}

#[test]
fn a_page_thinner_than_a_pixel_still_draws_a_pixel_wide_in_its_tab_thumbnail_and_embed() {
    let fixture = Fixture::new(&[
        ("slivers.pdf", &pdf("slivers.pdf")),
        ("Slivers.md", b"# Slivers\n\n![[slivers.pdf]]\n"),
    ]);
    let mut app = fixture.app();
    let tab = open_ready(&mut app, "slivers.pdf");
    let view = app.pdf(tab).unwrap();
    assert_eq!(view.page_size(1), Some([14400.0, 1.0]));
    assert_eq!(view.page_size(2), Some([1.0, 14400.0]));
    assert_eq!(view.page_size(3), Some([1.0, 1.0]));

    // Fit width in an 800-point tab on a 1x screen draws page 1 a quarter
    // of a pixel high, page 2 a quarter of a pixel wide, and page 3 at
    // 25% a quarter of a pixel each way.
    for (page, fit) in [(1, true), (2, true), (3, false)] {
        app.pdf_go_to(tab, page);
        if fit {
            app.pdf_fit_width(tab);
        } else {
            for _ in 0..20 {
                app.pdf_zoom_out(tab);
            }
        }
        app.size_pdf(tab, 800.0, 1.0);
        wait_until(&mut app, "the page draws", |app| {
            app.pdf(tab)
                .unwrap()
                .picture()
                .is_some_and(|picture| picture.page == page)
        });
        assert_drawn(app.pdf(tab).unwrap().picture().unwrap(), "the page");
    }

    app.pdf_toggle_thumbnails(tab);
    for page in 1..=3 {
        app.want_pdf_thumbnail(tab, page, 1.0);
    }
    wait_until(&mut app, "the thumbnails draw", |app| {
        (1..=3).all(|page| app.pdf(tab).unwrap().thumbnail(page).is_some())
    });
    for page in 1..=3 {
        assert_drawn(
            app.pdf(tab).unwrap().thumbnail(page).unwrap(),
            "a thumbnail",
        );
    }

    let note = open_from_tree(&mut app, "Slivers.md");
    for target in ["slivers.pdf", "slivers.pdf#page=2", "slivers.pdf#page=3"] {
        let PdfEmbedState::Drawn(picture) = embed(&mut app, note, target) else {
            panic!("{target} didn't draw");
        };
        assert_drawn(&picture, target);
    }
}

#[test]
fn previous_next_page_number_zoom_and_fit_width_move_through_the_pdf_and_stay_with_the_tab() {
    let fixture = Fixture::new(&[("text.pdf", &pdf("text.pdf")), ("Plan.md", b"# Plan\n")]);
    let mut app = fixture.app();
    let tab = open_ready(&mut app, "text.pdf");
    app.keep_tab(tab);

    app.pdf_next_page(tab);
    assert_eq!(page(&app, tab), 2);
    app.pdf_next_page(tab);
    app.pdf_next_page(tab);
    assert_eq!(page(&app, tab), 3, "the last page stays");
    app.pdf_previous_page(tab);
    assert_eq!(page(&app, tab), 2);
    app.pdf_go_to(tab, 1);
    assert_eq!(page(&app, tab), 1);
    app.pdf_previous_page(tab);
    assert_eq!(page(&app, tab), 1, "the first page stays");
    app.pdf_go_to(tab, 40);
    assert_eq!(page(&app, tab), 3, "a page past the end opens the last");
    app.pdf_go_to(tab, 0);
    assert_eq!(page(&app, tab), 1);

    // A new tab fits the page to its width.
    assert_eq!(app.pdf(tab).unwrap().zoom(), PdfZoom::FitWidth);
    app.size_pdf(tab, 612.0, 1.0);
    assert_eq!(app.pdf(tab).unwrap().scale(), 1.0);
    app.pdf_zoom_in(tab);
    assert_eq!(app.pdf(tab).unwrap().zoom(), PdfZoom::Scale(1.1));
    app.pdf_zoom_in(tab);
    assert_eq!(app.pdf(tab).unwrap().zoom(), PdfZoom::Scale(1.25));
    app.pdf_zoom_out(tab);
    app.pdf_zoom_out(tab);
    app.pdf_zoom_out(tab);
    assert_eq!(app.pdf(tab).unwrap().zoom(), PdfZoom::Scale(0.9));
    for _ in 0..30 {
        app.pdf_zoom_out(tab);
    }
    assert_eq!(app.pdf(tab).unwrap().zoom(), PdfZoom::Scale(0.25));
    app.pdf_fit_width(tab);
    assert_eq!(app.pdf(tab).unwrap().zoom(), PdfZoom::FitWidth);
    app.size_pdf(tab, 306.0, 1.0);
    assert_eq!(app.pdf(tab).unwrap().scale(), 0.5);
    // Zooming in from a fitted page goes to the next step past its size.
    app.pdf_zoom_in(tab);
    assert_eq!(app.pdf(tab).unwrap().zoom(), PdfZoom::Scale(0.67));

    app.pdf_go_to(tab, 2);
    app.open_file("Plan.md", OpenAs::Kept);
    assert_ne!(app.active_tab().unwrap().id(), tab);
    app.activate_tab(tab);
    let view = app.pdf(tab).unwrap();
    assert_eq!((view.page(), view.zoom()), (2, PdfZoom::Scale(0.67)));
}

#[test]
fn a_search_lists_matches_in_page_order_and_moves_between_them() {
    let fixture = Fixture::new(&[("text.pdf", &pdf("text.pdf"))]);
    let mut app = fixture.app();
    let tab = open_ready(&mut app, "text.pdf");
    app.pdf_go_to(tab, 2);

    app.search_pdf(tab, "APPLE");
    wait_until(&mut app, "the search ends", |app| {
        !app.pdf(tab).unwrap().search().searching
    });
    let search = app.pdf(tab).unwrap().search();
    let pages: Vec<usize> = search.matches.iter().map(|found| found.page).collect();
    assert_eq!(pages, [1, 3, 3]);
    assert_eq!(search.matches[0].snippet, "An apple a day.");
    assert_eq!(search.matches[1].snippet, "Apple and apple again.");
    // The first match shows.
    assert_eq!(search.current, Some(0));
    assert_eq!(page(&app, tab), 1);

    let mut seen = Vec::new();
    for _ in 0..4 {
        app.pdf_next_match(tab);
        seen.push((app.pdf(tab).unwrap().search().current, page(&app, tab)));
    }
    assert_eq!(
        seen,
        [(Some(1), 3), (Some(2), 3), (Some(0), 1), (Some(1), 3)],
        "next wraps around to the first match"
    );
    app.pdf_previous_match(tab);
    app.pdf_previous_match(tab);
    assert_eq!(app.pdf(tab).unwrap().search().current, Some(2));
    assert_eq!(page(&app, tab), 3);

    app.search_pdf(tab, "pear");
    wait_until(&mut app, "the search ends", |app| {
        !app.pdf(tab).unwrap().search().searching
    });
    let search = app.pdf(tab).unwrap().search();
    assert!(search.matches.is_empty());
    assert_eq!(search.current, None);
    app.pdf_next_match(tab);
    assert_eq!(page(&app, tab), 3, "with no match, the page stays");

    app.search_pdf(tab, "  ");
    let search = app.pdf(tab).unwrap().search();
    assert!(!search.searching);
    assert!(search.matches.is_empty());
}

#[test]
fn the_thumbnails_list_one_page_each_and_open_the_page_you_pick() {
    let fixture = Fixture::new(&[("text.pdf", &pdf("text.pdf"))]);
    let mut app = fixture.app();
    let tab = open_ready(&mut app, "text.pdf");
    assert!(!app.pdf(tab).unwrap().thumbnails_shown());

    app.pdf_toggle_thumbnails(tab);
    let view = app.pdf(tab).unwrap();
    assert!(view.thumbnails_shown());
    assert_eq!(view.thumbnail_pages().collect::<Vec<_>>(), [1, 2, 3]);

    app.want_pdf_thumbnail(tab, 2, 2.0);
    wait_until(&mut app, "the thumbnail renders", |app| {
        app.pdf(tab).unwrap().thumbnail(2).is_some()
    });
    let thumbnail = app.pdf(tab).unwrap().thumbnail(2).unwrap();
    assert_eq!(thumbnail.page, 2);
    // 100 points wide at two pixels a point.
    assert_eq!(thumbnail.width, 200);
    assert!(app.pdf(tab).unwrap().thumbnail(1).is_none());

    app.pdf_go_to(tab, 2);
    assert_eq!(page(&app, tab), 2);
    app.pdf_toggle_thumbnails(tab);
    assert!(!app.pdf(tab).unwrap().thumbnails_shown());
}

#[test]
fn a_locked_pdf_asks_for_its_password_again_when_its_wrong_and_opens_with_the_right_one() {
    let fixture = Fixture::new(&[("locked.pdf", &pdf("locked.pdf"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "locked.pdf");
    wait_until(&mut app, "the PDF asks for its password", |app| {
        app.pdf(tab).unwrap().state() == &PdfState::Locked { wrong: false }
    });

    app.pdf_unlock(tab, "sesame".to_owned());
    wait_until(&mut app, "the PDF refuses the password", |app| {
        app.pdf(tab).unwrap().state() == &PdfState::Locked { wrong: true }
    });

    app.pdf_unlock(tab, "open sesame".to_owned());
    wait_until(&mut app, "the PDF opens", |app| {
        app.pdf(tab).unwrap().state() == &PdfState::Ready
    });
    assert_eq!(app.pdf(tab).unwrap().page_count(), 2);
    app.search_pdf(tab, "second");
    wait_until(&mut app, "the search ends", |app| {
        !app.pdf(tab).unwrap().search().searching
    });
    let pages: Vec<usize> = app
        .pdf(tab)
        .unwrap()
        .search()
        .matches
        .iter()
        .map(|found| found.page)
        .collect();
    assert_eq!(pages, [2]);
}

/// Every file under `folder` whose bytes hold `secret`.
fn files_holding(folder: &std::path::Path, secret: &[u8], found: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_holding(&path, secret, found);
        } else if std::fs::read(&path)
            .unwrap_or_default()
            .windows(secret.len())
            .any(|window| window == secret)
        {
            found.push(path);
        }
    }
}

/// A log that keeps every line the app writes while a test runs.
#[derive(Clone, Default)]
struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn the_password_reaches_no_file_setting_or_log() {
    const PASSWORD: &str = "open sesame";
    let log = Captured::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    // Every thread logs here, the PDF's own included. No other test in
    // this file sets a subscriber.
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let fixture = Fixture::new(&[("locked.pdf", &pdf("locked.pdf"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "locked.pdf");
    wait_until(&mut app, "the PDF asks for its password", |app| {
        matches!(app.pdf(tab).unwrap().state(), PdfState::Locked { .. })
    });
    app.pdf_unlock(tab, "wrong one".to_owned());
    wait_until(&mut app, "the PDF refuses the password", |app| {
        app.pdf(tab).unwrap().state() == &PdfState::Locked { wrong: true }
    });
    app.pdf_unlock(tab, PASSWORD.to_owned());
    wait_until(&mut app, "the PDF opens", |app| {
        app.pdf(tab).unwrap().state() == &PdfState::Ready
    });
    app.pdf_go_to(tab, 2);
    app.close_tab(tab);
    drop(app);

    let mut found = Vec::new();
    for folder in [fixture.app_data.path(), fixture.root.as_path()] {
        for secret in [PASSWORD, "wrong one"] {
            files_holding(folder, secret.as_bytes(), &mut found);
        }
    }
    assert_eq!(found, Vec::<std::path::PathBuf>::new());
    let log = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(!log.contains(PASSWORD), "{log}");
    assert!(!log.contains("wrong one"), "{log}");
}

#[test]
fn replacing_an_open_pdf_on_disk_shows_the_new_file() {
    let fixture = Fixture::new(&[("text.pdf", &pdf("text.pdf"))]);
    let mut app = fixture.app();
    let tab = open_ready(&mut app, "text.pdf");
    app.pdf_go_to(tab, 3);

    support::write(&fixture.root, "text.pdf", &pdf("large.pdf"));

    wait_until(&mut app, "the new file shows", |app| {
        let view = app.pdf(tab).unwrap();
        view.state() == &PdfState::Ready && view.page_count() == 400
    });
    // The page you were on stays.
    assert_eq!(page(&app, tab), 3);

    support::write(&fixture.root, "text.pdf", &pdf("locked.pdf"));
    wait_until(&mut app, "the locked file asks for its password", |app| {
        app.pdf(tab).unwrap().state() == &PdfState::Locked { wrong: false }
    });
}

/// A note with links and embeds of the fixture PDF in `Docs`.
const READING: &[u8] = b"# Reading\n\n[Second](Docs/text.pdf#page=2)\n\n![[text.pdf#page=2]]\n";

#[test]
fn a_link_with_a_page_number_opens_the_pdf_at_that_page_and_leaves_the_note_alone() {
    let fixture = Fixture::new(&[("Docs/text.pdf", &pdf("text.pdf")), ("Reading.md", READING)]);
    let mut app = fixture.app();
    let note = open_from_tree(&mut app, "Reading.md");
    app.keep_tab(note);

    // The PDF opens on its thread and shows the page once it can.
    app.follow_note_link(note, "Docs/text.pdf#page=2", OpenAs::Preview);
    let tab = app.active_tab().unwrap().id();
    assert_ne!(tab, note);
    assert_eq!(app.pdf(tab).unwrap().path(), "Docs/text.pdf");
    wait_until(&mut app, "the PDF opens", |app| {
        app.pdf(tab).unwrap().state() == &PdfState::Ready
    });
    assert_eq!(page(&app, tab), 2);

    for (target, expected) in [
        ("Docs/text.pdf#page=9", 3),
        ("Docs/text.pdf#page=99999999999", 3),
        ("text.pdf#page=3", 3),
        ("Docs/text.pdf#page=x", 1),
        ("Docs/text.pdf#page=0", 1),
        ("Docs/text.pdf#page=2", 2),
        ("text.pdf", 1),
    ] {
        app.activate_tab(note);
        app.follow_note_link(note, target, OpenAs::Preview);
        assert_eq!(app.active_tab().unwrap().id(), tab, "{target}");
        assert_eq!(page(&app, tab), expected, "{target}");
    }

    app.activate_tab(note);
    app.follow_note_link(note, "Missing.pdf#page=2", OpenAs::Preview);
    assert_eq!(app.active_tab().unwrap().id(), note);
    support::settle_saves(&mut app);
    assert_eq!(fixture.read("Reading.md"), READING);
}

/// Ask the note in `note` about its embed of `target` until the page
/// draws or the embed finds a problem.
fn embed(app: &mut App, note: TabId, target: &str) -> PdfEmbedState {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll();
        let links = app.note_links(note).unwrap();
        match links.pdf_embed(target, || {}) {
            Some(PdfEmbedState::Drawing) => {}
            Some(state) => return state,
            None => panic!("{target} names no PDF"),
        }
        assert!(
            Instant::now() < deadline,
            "the embed of {target} never drew"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn an_embed_of_a_pdf_shows_the_page_its_fragment_names_and_leaves_the_note_alone() {
    let fixture = Fixture::new(&[
        ("Docs/text.pdf", &pdf("text.pdf")),
        ("Docs/locked.pdf", &pdf("locked.pdf")),
        ("Reading.md", READING),
    ]);
    let mut app = fixture.app();
    let note = open_from_tree(&mut app, "Reading.md");
    let links = app.note_links(note).unwrap();
    assert_eq!(
        links.pdf_link("text.pdf#page=2"),
        Some(("Docs/text.pdf".to_owned(), Some(2)))
    );
    assert_eq!(links.pdf_link("Reading"), None);
    assert!(links.pdf_embed("Missing.pdf", || {}).is_none());
    assert!(links.pdf_embed("Reading", || {}).is_none());

    for (target, expected) in [
        ("text.pdf#page=2", 2),
        ("Docs/text.pdf", 1),
        ("text.pdf#page=40", 3),
        ("text.pdf#page=99999999999", 3),
        ("text.pdf#page=nope", 1),
    ] {
        let PdfEmbedState::Drawn(picture) = embed(&mut app, note, target) else {
            panic!("{target} didn't draw");
        };
        assert_eq!(picture.page, expected, "{target}");
        assert_eq!(picture.width, EMBED_WIDTH as u32, "{target}");
    }
    assert_eq!(
        embed(&mut app, note, "locked.pdf"),
        PdfEmbedState::Problem(EmbedProblem::Locked)
    );

    // Another app replaces the PDF: the embed draws the new file.
    support::write(&fixture.root, "Docs/text.pdf", &pdf("large.pdf"));
    wait_until(&mut app, "the embed draws the new file", |app| {
        let links = app.note_links(note).unwrap();
        matches!(
            links.pdf_embed("text.pdf#page=40", || {}),
            Some(PdfEmbedState::Drawn(picture)) if picture.page == 40
        )
    });

    app.close_tab(note);
    support::settle_saves(&mut app);
    assert_eq!(fixture.read("Reading.md"), READING);
}
