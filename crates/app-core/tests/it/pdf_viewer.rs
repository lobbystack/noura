//! PDF tabs, seam 1: a PDF from the tree opens in a tab with its page
//! count, and the tab scrolls through its pages, draws the tiles near the
//! view at the screen's density, zooms, searches its text, lists its
//! thumbnails and unlocks it, while the work runs off the caller's thread.

use crate::support;
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

use app_core::{
    App, EmbedProblem, MAX_EMBED_TILE_BYTES, MAX_THUMBNAILS, MAX_TILE_BYTES, NoteLinks, OpenAs,
    PAGE_GAP, PdfArea, PdfEmbedArea, PdfEmbedState, PdfEmbedViewer, PdfState, PdfTile, PdfView,
    PdfZoom, TILE_HEIGHT, TILE_WIDTH, TabId,
};
use support::{Fixture, open_from_tree};

/// The bytes of the fixture PDF `name`.
pub fn pdf(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pdf")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{name} can't be read: {error}"))
}

/// A PDF of `count` Letter pages, each saying its number.
fn numbered_pages(count: usize) -> Vec<u8> {
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {count} >>",
            (0..count)
                .map(|index| format!("{} 0 R", 5 + 2 * index))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    ];
    for index in 0..count {
        let text = format!("BT /F1 24 Tf 72 700 Td (Page {}) Tj ET", index + 1);
        objects.push(format!(
            "<< /Length {} >>\nstream\n{text}\nendstream",
            text.len()
        ));
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            4 + 2 * index
        ));
    }
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{body}\nendobj\n", index + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for offset in offsets {
        out.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .bytes(),
    );
    out
}

/// Poll until `done` holds.
fn wait_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
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

fn view(app: &App, tab: TabId) -> &PdfView {
    app.pdf(tab).unwrap()
}

/// Show `height` points of the pages of `tab` from `top` down, in a view
/// as wide as the pages, as a client does: after the jump the tab asks
/// for, if any, and no further than the pages reach. Returns the top it
/// showed.
fn show(app: &mut App, tab: TabId, top: f32, height: f32) -> f32 {
    show_at(app, tab, 0.0, top, height)
}

/// [`show`], scrolled `left` points across.
fn show_at(app: &mut App, tab: TabId, left: f32, top: f32, height: f32) -> f32 {
    let pdf = view(app, tab);
    let jump = pdf.jump();
    let total = pdf.content_size()[1];
    let top = jump
        .map_or(top, |jump| jump.top)
        .clamp(0.0, (total - height).max(0.0));
    app.show_pdf_area(
        tab,
        PdfArea {
            left,
            top,
            width: VIEW_WIDTH,
            height,
            jump: jump.map_or(0, |jump| jump.seq),
        },
    );
    top
}

/// Whether the tiles at the newest density cover every part of the pages
/// between `top` and `top + height` points down.
fn covered(pdf: &PdfView, top: f32, height: f32) -> bool {
    let density = pdf.density();
    pdf.pages_between(top, top + height).all(|page| {
        let [_, page_top, _, _] = pdf.page_rect(page).unwrap();
        let rows = (pdf.page_size(page).unwrap()[1] * density).ceil() as u32;
        let points = pdf.scale() / density;
        // The rows of pixels that show.
        let from = (((top - page_top).max(0.0) / points) as u32).min(rows - 1);
        let to = (((top + height - page_top) / points).ceil() as u32).clamp(from + 1, rows);
        // The rows the tiles hold, joined from the first that shows down.
        let mut held: Vec<(u32, u32)> = pdf
            .tiles(page)
            .filter(|tile| tile.key.density() == density)
            .map(|tile| (tile.y, tile.y + tile.picture.height))
            .collect();
        held.sort_unstable();
        let mut reached = from;
        for (start, end) in held {
            if start <= reached {
                reached = reached.max(end);
            }
        }
        reached >= to
    })
}

/// Show `height` points from `top` down and wait until they draw. Returns
/// the top it showed.
fn show_drawn(app: &mut App, tab: TabId, top: f32, height: f32) -> f32 {
    let top = show(app, tab, top, height);
    wait_until(app, "what shows draws", |app| {
        covered(view(app, tab), top, height)
    });
    top
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
    app.size_pdf(tab, 306.0, 2.0);
    let asked = Instant::now();
    show(&mut app, tab, 0.0, 400.0);
    // The page renders on the PDF's own thread, which the test holds, so
    // the call comes back at once with nothing drawn.
    assert!(asked.elapsed() < Duration::from_secs(1));
    for _ in 0..20 {
        app.poll();
        assert_eq!(view(&app, tab).tile_count(), 0);
        std::thread::sleep(Duration::from_millis(5));
    }

    drop(hold);
    wait_until(&mut app, "the page renders", |app| {
        view(app, tab).tiles(1).next().is_some()
    });
    let tile = view(&app, tab).tiles(1).next().unwrap();
    assert_eq!((tile.key.page, tile.x, tile.y), (1, 0, 0));
    // Fit width: 306 points for a 612-point page is half size, at two
    // pixels a point.
    assert_eq!(tile.key.density(), 1.0);
    assert_eq!((tile.picture.width, tile.picture.height), (612, 792));
    assert_eq!(tile.picture.rgba.len(), 612 * 792 * 4);
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
    let pdf = view(&app, tab);
    assert_eq!(pdf.page_size(1), Some([14400.0, 1.0]));
    assert_eq!(pdf.page_size(2), Some([1.0, 14400.0]));
    assert_eq!(pdf.page_size(3), Some([1.0, 1.0]));

    // Fit width in an 800-point tab on a 1x screen draws at 25%: page 1 a
    // quarter of a pixel high, page 2 a quarter of a pixel wide, and page
    // 3 a quarter of a pixel each way. Pages 2 and 3 line up on the
    // middle of page 1, 3,600 points wide, so the view scrolls across to
    // them.
    app.size_pdf(tab, 800.0, 1.0);
    for page in 1..=3 {
        app.pdf_go_to(tab, page);
        let left = (view(&app, tab).page_rect(page).unwrap()[0] - 100.0).max(0.0);
        show_at(&mut app, tab, left, 0.0, 700.0);
        wait_until(&mut app, "the page draws", |app| {
            view(app, tab).tiles(page).next().is_some()
        });
        for tile in view(&app, tab).tiles(page) {
            assert_drawn(&tile.picture, "a tile");
        }
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
    let links = app.note_links(note).unwrap();
    let viewer = links.pdf_embed_viewer(|| {});
    for target in ["slivers.pdf", "slivers.pdf#page=2", "slivers.pdf#page=3"] {
        let PdfEmbedState::Page { size, .. } = embed(&mut app, note, target) else {
            panic!("{target} didn't open");
        };
        // The top of the page, at its own size on a 2x screen.
        let tiles = embed_tiles(&links, &viewer, target, 2.0, 0.0, size[1].min(700.0));
        assert!(!tiles.is_empty(), "{target}");
        for tile in &tiles {
            assert_drawn(&tile.picture, target);
        }
    }
}

#[test]
fn going_to_a_page_zoom_and_fit_width_scroll_the_pdf_and_stay_with_the_tab() {
    let fixture = Fixture::new(&[("text.pdf", &pdf("text.pdf")), ("Plan.md", b"# Plan\n")]);
    let mut app = fixture.app();
    let tab = open_ready(&mut app, "text.pdf");
    app.keep_tab(tab);
    app.size_pdf(tab, 612.0, 1.0);

    // Going to a page scrolls its top a gap below the view's top.
    app.pdf_go_to(tab, 2);
    assert_eq!(page(&app, tab), 2);
    let top_of = |app: &App, page: usize| view(app, tab).page_rect(page).unwrap()[1];
    assert_eq!(
        view(&app, tab).jump().unwrap().top,
        top_of(&app, 2) - PAGE_GAP
    );
    app.pdf_go_to(tab, 40);
    assert_eq!(page(&app, tab), 3, "a page past the end goes to the last");
    app.pdf_go_to(tab, 0);
    assert_eq!(page(&app, tab), 1);
    assert_eq!(view(&app, tab).jump().unwrap().top, 0.0);

    // A new tab fits the pages to its width.
    assert_eq!(app.pdf(tab).unwrap().zoom(), PdfZoom::FitWidth);
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
    let pdf = app.pdf(tab).unwrap();
    assert_eq!((pdf.page(), pdf.zoom()), (2, PdfZoom::Scale(0.67)));
    assert!(pdf.jump().is_some(), "the jump waits for the tab to draw");
}

#[test]
fn the_page_that_shows_follows_the_scroll_and_a_jump_keeps_its_page_until_you_scroll() {
    let fixture = Fixture::new(&[("text.pdf", &pdf("text.pdf"))]);
    let mut app = fixture.app();
    let tab = open_ready(&mut app, "text.pdf");
    // At full size, page n spans 12 + 804 (n - 1) to 792 points below.
    app.size_pdf(tab, 612.0, 1.0);
    assert_eq!(
        view(&app, tab).content_size(),
        [636.0, 4.0 * 12.0 + 3.0 * 792.0]
    );

    // The page at the middle of the view shows.
    for (top, expected) in [(0.0, 1), (700.0, 2), (1200.0, 2), (1500.0, 3)] {
        show(&mut app, tab, top, 400.0);
        assert_eq!(page(&app, tab), expected, "at {top}");
    }

    // A view 2,000 points tall scrolls 424 points at most, so the middle
    // of the view never reaches page 3. Going there shows page 3 until
    // you scroll.
    app.pdf_go_to(tab, 3);
    assert_eq!(show(&mut app, tab, 0.0, 400.0), 1608.0);
    assert!(view(&app, tab).jump().is_none(), "the jump landed");
    assert_eq!(page(&app, tab), 3);
    app.pdf_go_to(tab, 3);
    let landed = show(&mut app, tab, 0.0, 2000.0);
    assert_eq!(landed, 424.0);
    assert_eq!(page(&app, tab), 3);
    show(&mut app, tab, 424.0, 2000.0);
    assert_eq!(page(&app, tab), 3, "the view stays where the jump landed");
    show(&mut app, tab, 400.0, 2000.0);
    assert_eq!(page(&app, tab), 2, "the middle of the view is on page 2");

    // Until the client makes a jump, what it showed before doesn't count.
    app.pdf_go_to(tab, 1);
    app.show_pdf_area(
        tab,
        PdfArea {
            left: 0.0,
            top: 1700.0,
            width: 636.0,
            height: 400.0,
            jump: 0,
        },
    );
    assert_eq!(page(&app, tab), 1);
    assert!(view(&app, tab).jump().is_some());
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
    // The second line's baseline sits 122 points down the page, and its
    // 24-point text reaches 24 points above it.
    assert!((search.matches[0].top - 98.0).abs() < 1.0);
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

#[test]
fn the_password_reaches_no_file_setting_or_log() {
    const PASSWORD: &str = "open sesame";
    // Every thread logs here, the app's own included.
    let log = support::logs::Capture::all();

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
    let log = String::from_utf8(log.bytes()).unwrap();
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
    // The view scrolls to page 2's top.
    let pdf = view(&app, tab);
    assert_eq!(
        pdf.jump().unwrap().top,
        pdf.page_rect(2).unwrap()[1] - PAGE_GAP
    );

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

/// Ask the note in `note` about its embed of `target` until the PDF opens
/// or the embed finds a problem.
fn embed(app: &mut App, note: TabId, target: &str) -> PdfEmbedState {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll();
        let links = app.note_links(note).unwrap();
        match links.pdf_embed(target) {
            Some(PdfEmbedState::Opening) => {}
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
    assert!(links.pdf_embed("Missing.pdf").is_none());
    assert!(links.pdf_embed("Reading").is_none());

    for (target, expected) in [
        ("text.pdf#page=2", 2),
        ("Docs/text.pdf", 1),
        ("text.pdf#page=40", 3),
        ("text.pdf#page=99999999999", 3),
        ("text.pdf#page=nope", 1),
    ] {
        assert_eq!(
            embed(&mut app, note, target),
            PdfEmbedState::Page {
                number: expected,
                size: [612.0, 792.0]
            },
            "{target}"
        );
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
            links.pdf_embed("text.pdf#page=40"),
            Some(PdfEmbedState::Page { number: 40, .. })
        )
    });

    app.close_tab(note);
    support::settle_saves(&mut app);
    assert_eq!(fixture.read("Reading.md"), READING);
}

/// Show the part of the page the embed of `target` names from `top` down,
/// `height` points of the page tall and as wide as it, drawn at `density`
/// pixels a point, frame after frame until the tiles that show have drawn
/// at that density. Returns the tiles kept of the page.
fn embed_tiles(
    links: &NoteLinks,
    viewer: &PdfEmbedViewer,
    target: &str,
    density: f32,
    top: f32,
    height: f32,
) -> Vec<PdfTile> {
    let deadline = Instant::now() + Duration::from_secs(60);
    let Some(PdfEmbedState::Page { size, .. }) = links.pdf_embed(target) else {
        panic!("{target} isn't open");
    };
    let area = PdfEmbedArea {
        left: 0.0,
        top,
        width: size[0],
        height,
        density,
    };
    loop {
        let tiles = links.pdf_embed_tiles(target, viewer, area);
        viewer.frame_done();
        // The rows of tiles that cover the part that shows.
        let pixels = (size[1] * density - 1e-3).ceil().max(1.0);
        let first = ((top * density).max(0.0) / TILE_HEIGHT as f32).floor() as u32;
        let end = (((top + height) * density).min(pixels) / TILE_HEIGHT as f32).ceil() as u32;
        let drawn = (first..end).all(|row| {
            tiles.iter().any(|tile| {
                tile.key.row == row && tile.key.density().to_bits() == density.to_bits()
            })
        });
        if drawn {
            return tiles;
        }
        assert!(Instant::now() < deadline, "{target} never drew at {top}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A note that embeds the tall page twice, as a web page saved as PDF.
const SAVED_PAGE: &[u8] = b"# Saved page\n\n![[tall.pdf]]\n\n![[tall.pdf#page=1]]\n";

#[test]
fn an_embedded_tall_page_draws_a_pixel_for_each_of_a_2x_screens_where_it_shows() {
    let fixture = Fixture::new(&[
        ("tall.pdf", &pdf("tall.pdf")),
        ("Saved page.md", SAVED_PAGE),
    ]);
    let mut app = fixture.app();
    let note = open_from_tree(&mut app, "Saved page.md");
    assert_eq!(
        embed(&mut app, note, "tall.pdf"),
        PdfEmbedState::Page {
            number: 1,
            size: [612.0, 14000.0]
        }
    );
    let links = app.note_links(note).unwrap();
    let viewer = links.pdf_embed_viewer(|| {});

    // The embed shows 422 points wide on a screen with two pixels a
    // point, so the page draws 844 pixels wide, as a tab that showed it
    // 422 points wide would, however tall the page is. 700 points of the
    // view are 1,016 points of the page.
    let density = 2.0 * 422.0 / 612.0;
    let tiles = embed_tiles(&links, &viewer, "tall.pdf", density, 0.0, 1016.0);
    let sharp: Vec<&PdfTile> = tiles
        .iter()
        .filter(|tile| tile.key.density().to_bits() == density.to_bits())
        .collect();
    assert!(!sharp.is_empty());
    for tile in &sharp {
        assert_eq!(tile.picture.width, 844);
        assert!(tile.picture.height <= TILE_HEIGHT);
    }
    // Only the tiles near the view drew: a few of the page's 12 rows.
    assert!(sharp.len() <= 4, "{} tiles", sharp.len());

    // Both embeds of the page share one thread, which reads the PDF once.
    assert_eq!(links.pdf_embed_threads_for_testing(), 1);
    assert!(links.pdf_embed("tall.pdf#page=1").is_some());
    assert_eq!(links.pdf_embed_threads_for_testing(), 1);
}

#[test]
fn an_embed_scrolled_far_down_keeps_only_the_tiles_near_the_view() {
    let fixture = Fixture::new(&[
        ("tall.pdf", &pdf("tall.pdf")),
        ("Saved page.md", SAVED_PAGE),
    ]);
    let mut app = fixture.app();
    let note = open_from_tree(&mut app, "Saved page.md");
    embed(&mut app, note, "tall.pdf");
    let links = app.note_links(note).unwrap();
    let viewer = links.pdf_embed_viewer(|| {});
    // The page as it shows 612 points wide on a 1x screen, 612 pixels
    // wide, in a view 700 points tall.
    let density = 1.0;
    let height = 700.0;

    // Scroll down the page a view at a time to near its end.
    let mut top = 0.0;
    let mut most = 0;
    while top < 13000.0 {
        embed_tiles(&links, &viewer, "tall.pdf", density, top, height);
        // The frame after the tiles drew keeps those near the view.
        let area = PdfEmbedArea {
            left: 0.0,
            top,
            width: 612.0,
            height,
            density,
        };
        let tiles = links.pdf_embed_tiles("tall.pdf", &viewer, area);
        viewer.frame_done();
        most = most.max(links.pdf_embed_tile_bytes_for_testing());
        // Nothing past two views above or below what shows stays.
        for tile in &tiles {
            let tile_top = tile.y as f32 / tile.key.density();
            let tile_bottom = tile_top + tile.picture.height as f32 / tile.key.density();
            assert!(tile_bottom > top - 2.0 * height, "{top}: {:?}", tile.key);
            assert!(tile_top < top + 3.0 * height, "{top}: {:?}", tile.key);
        }
        top += height;
    }
    // The tiles of five views at most, and a row of tiles past each end,
    // each 612 pixels wide: about 9 MB.
    let row = 612 * TILE_HEIGHT as usize * 4;
    let bound = (5.0 * height * density) as usize * 612 * 4 + 2 * row;
    assert!(most <= bound, "{most} bytes, past {bound}");
    assert!(most <= MAX_EMBED_TILE_BYTES);

    // Once no viewer shows the embed, as when its note closes, its tiles
    // and its thread go.
    assert_eq!(links.pdf_embed_threads_for_testing(), 1);
    drop(viewer);
    assert_eq!(links.pdf_embed_tile_bytes_for_testing(), 0);
    assert_eq!(links.pdf_embed_threads_for_testing(), 0);
}

#[test]
fn a_tall_page_draws_a_pixel_for_each_of_a_2x_screens_at_every_zoom() {
    let fixture = Fixture::new(&[("tall.pdf", &pdf("tall.pdf"))]);
    let mut app = fixture.app();
    let tab = open_ready(&mut app, "tall.pdf");
    assert_eq!(view(&app, tab).page_size(1), Some([612.0, 14000.0]));

    // Fit width: the page shows 840 points wide on a screen with two
    // pixels a point, so it draws 1,680 pixels wide however tall it is.
    app.size_pdf(tab, 840.0, 2.0);
    let top = show_drawn(&mut app, tab, 0.0, 700.0);
    let pdf = view(&app, tab);
    assert_eq!(pdf.page_rect(1).unwrap()[2], 840.0);
    assert!((pdf.density() - 2.0 * 840.0 / 612.0).abs() < 1e-4);
    let shown: Vec<_> = pdf
        .tiles(1)
        .filter(|tile| tile.key.density() == pdf.density())
        .collect();
    assert!(!shown.is_empty());
    for tile in &shown {
        assert_eq!(tile.picture.width, 1680);
        assert!(tile.picture.height <= TILE_HEIGHT);
    }
    assert_eq!(top, 0.0);

    // Every zoom draws at twice its scale, in tiles no larger than the
    // largest.
    for target in [0.5, 1.0, 2.0, 4.0] {
        while view(&app, tab).scale() > target + 0.001 {
            app.pdf_zoom_out(tab);
        }
        while view(&app, tab).scale() < target - 0.001 {
            app.pdf_zoom_in(tab);
        }
        let top = show_drawn(&mut app, tab, 0.0, 700.0);
        let pdf = view(&app, tab);
        assert_eq!(pdf.density(), 2.0 * target);
        let wide = (612.0 * 2.0 * target).ceil() as u32;
        let shows = |tile: &&app_core::PdfTile| tile.key.density() == pdf.density();
        assert!(pdf.tiles(1).any(|tile| shows(&tile)), "at {target}");
        for tile in pdf.tiles(1).filter(shows) {
            assert!(tile.picture.width <= TILE_WIDTH && tile.picture.height <= TILE_HEIGHT);
            assert!(tile.x + tile.picture.width <= wide);
        }
        assert!(pdf.tile_bytes() <= MAX_TILE_BYTES);
        eprintln!(
            "at {target}: {} tiles, {} MB, view at {top}",
            pdf.tile_count(),
            pdf.tile_bytes() >> 20
        );
    }

    // Far down the page, the tiles at its top go.
    app.pdf_fit_width(tab);
    show(&mut app, tab, 0.0, 700.0);
    let bottom = view(&app, tab).content_size()[1];
    let top = show_drawn(&mut app, tab, bottom, 700.0);
    assert_eq!(top, bottom - 700.0);
    show(&mut app, tab, top, 700.0);
    let pdf = view(&app, tab);
    assert!(pdf.tiles(1).all(|tile| tile.y > 0), "the top row stayed");
    assert!(covered(pdf, top, 700.0));
}

/// How wide the tests' view is, in points.
const VIEW_WIDTH: f32 = 864.0;

#[test]
fn a_long_pdf_opens_at_once_and_draws_only_the_pages_near_the_view() {
    let fixture = Fixture::new(&[("long.pdf", &numbered_pages(600))]);
    let mut app = fixture.app();
    let opening = Instant::now();
    let tab = open_ready(&mut app, "long.pdf");
    let opened = opening.elapsed();
    assert_eq!(view(&app, tab).page_count(), 600);
    // Nothing draws before the view shows.
    assert_eq!(view(&app, tab).tile_count(), 0);

    app.size_pdf(tab, 600.0, 2.0);
    let height = 700.0;
    let total = view(&app, tab).content_size()[1];
    let mut drawn = BTreeSet::new();
    for share in [0.0, 0.25, 0.5, 0.9, 1.0] {
        let asked = Instant::now();
        let top = show_drawn(&mut app, tab, (total - height) * share, height);
        let took = asked.elapsed();
        // A frame later, as the window draws the next.
        show(&mut app, tab, top, height);
        let pdf = view(&app, tab);
        let near = pdf.pages_between(top - 2.0 * height, top + 3.0 * height);
        let kept: Vec<usize> = (1..=600)
            .filter(|page| pdf.tiles(*page).next().is_some())
            .collect();
        assert!(
            kept.iter().all(|page| near.contains(page)),
            "{kept:?} kept around {near:?}"
        );
        assert!(pdf.tile_bytes() <= MAX_TILE_BYTES);
        let middle = top + height / 2.0;
        assert!(
            pdf.pages_between(middle - PAGE_GAP, middle + PAGE_GAP)
                .contains(&pdf.page()),
            "page {} shows at {middle}",
            pdf.page()
        );
        eprintln!(
            "at {:.0}%: page {} shows after {took:?}; {} tiles of pages {kept:?} kept, {} MB",
            share * 100.0,
            pdf.page(),
            pdf.tile_count(),
            pdf.tile_bytes() >> 20
        );
        drawn.extend(kept);
    }
    eprintln!(
        "600 pages opened in {opened:?}; {} pages drawn in all",
        drawn.len()
    );
    assert!(drawn.len() <= 40, "{} pages drew", drawn.len());
}

#[test]
fn zooming_keeps_what_shows_at_the_middle_of_the_view_in_place() {
    let fixture = Fixture::new(&[("tall.pdf", &pdf("tall.pdf"))]);
    let mut app = fixture.app();
    let tab = open_ready(&mut app, "tall.pdf");
    // Fit width: 840 points for a page 612 wide.
    app.size_pdf(tab, VIEW_WIDTH - 2.0 * PAGE_GAP, 1.0);
    let scale = view(&app, tab).scale();
    show(&mut app, tab, 5000.0, 600.0);

    // The middle of the view is 5,300 points down the pages, which lands
    // where it was on page 1 once the page draws one step larger.
    let down = (5300.0 - PAGE_GAP) / scale;
    app.pdf_zoom_in(tab);
    assert_eq!(view(&app, tab).zoom(), PdfZoom::Scale(1.5));
    let pdf = view(&app, tab);
    let jump = pdf.jump().unwrap();
    let middle = pdf.page_rect(1).unwrap()[1] + down * 1.5;
    assert!((jump.top + 300.0 - middle).abs() < 0.5, "{jump:?}");
    // Across, the middle of the view stays the middle of the pages.
    let left = jump.left.unwrap();
    assert!((left + VIEW_WIDTH / 2.0 - pdf.content_size()[0] / 2.0).abs() < 0.5);

    show(&mut app, tab, 0.0, 600.0);
    app.pdf_fit_width(tab);
    let jump = view(&app, tab).jump().unwrap();
    assert!((jump.top - 5000.0).abs() < 0.5, "{jump:?}");
}

#[test]
fn find_scrolls_to_the_line_with_the_match_far_down_a_tall_page() {
    let fixture = Fixture::new(&[("tall.pdf", &pdf("tall.pdf"))]);
    let mut app = fixture.app();
    let tab = open_ready(&mut app, "tall.pdf");
    app.size_pdf(tab, 612.0, 2.0);
    show(&mut app, tab, 0.0, 600.0);

    app.search_pdf(tab, "needle");
    wait_until(&mut app, "the search ends", |app| {
        !view(app, tab).search().searching
    });
    let found = view(&app, tab).search().matches.clone();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].page, 1);
    // Line 800's baseline lies 48 + 799 × 16 points down the page, and
    // its 12-point text reaches 12 points above it.
    assert!((found[0].top - 12_820.0).abs() < 1.0, "{}", found[0].top);

    // The line lands a third of the way down the view, and draws.
    let top = show_drawn(&mut app, tab, 0.0, 600.0);
    let line = view(&app, tab).page_rect(1).unwrap()[1] + found[0].top;
    assert!(
        (line - top - 200.0).abs() < 1.0,
        "the line is {} points down the view",
        line - top
    );
    assert_eq!(page(&app, tab), 1);
}

#[test]
fn the_thumbnails_keep_those_wanted_last_up_to_their_cap() {
    let fixture = Fixture::new(&[("long.pdf", &numbered_pages(600))]);
    let mut app = fixture.app();
    let tab = open_ready(&mut app, "long.pdf");
    app.pdf_toggle_thumbnails(tab);

    for page in 1..=600 {
        app.want_pdf_thumbnail(tab, page, 1.0);
        app.poll();
    }
    wait_until(&mut app, "the last thumbnail draws", |app| {
        view(app, tab).thumbnail(600).is_some()
    });
    let kept = (1..=600)
        .filter(|page| view(&app, tab).thumbnail(*page).is_some())
        .count();
    assert!(kept <= MAX_THUMBNAILS, "{kept} thumbnails kept");
    assert!(view(&app, tab).thumbnail(1).is_none(), "the first stayed");

    // One that went draws again when the panel wants it.
    app.want_pdf_thumbnail(tab, 1, 1.0);
    wait_until(&mut app, "the first thumbnail draws again", |app| {
        view(app, tab).thumbnail(1).is_some()
    });
}

#[test]
fn a_new_width_at_fit_width_keeps_what_shows_at_the_middle_of_the_view_in_place() {
    let fixture = Fixture::new(&[("tall.pdf", &pdf("tall.pdf"))]);
    let mut app = fixture.app();
    let tab = open_ready(&mut app, "tall.pdf");
    app.size_pdf(tab, 612.0, 1.0);
    show(&mut app, tab, 5000.0, 600.0);
    assert!(view(&app, tab).jump().is_none());

    // The thumbnails panel or a smaller window leaves the pages half the
    // width: the middle of the view, 5,288 points into page 1, stays in
    // the middle.
    app.size_pdf(tab, 306.0, 1.0);
    assert_eq!(view(&app, tab).scale(), 0.5);
    let jump = view(&app, tab).jump().unwrap();
    let middle = PAGE_GAP + 5288.0 * 0.5;
    assert!((jump.top + 300.0 - middle).abs() < 0.5, "{jump:?}");

    // The same width again, as each frame says, scrolls nowhere.
    show(&mut app, tab, 0.0, 600.0);
    app.size_pdf(tab, 306.0, 1.0);
    assert!(view(&app, tab).jump().is_none());

    // At a zoom of its own, the pages keep their size and the view its
    // place.
    app.pdf_zoom_in(tab);
    show(&mut app, tab, 0.0, 600.0);
    app.size_pdf(tab, 612.0, 1.0);
    assert!(view(&app, tab).jump().is_none());
}
