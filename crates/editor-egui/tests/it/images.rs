//! Images draw inline from `![alt](path)` and `![[image.png]]`, and
//! `![[image.png|300]]` sets the width. The embedding app loads them.

use crate::support;
use std::sync::{Arc, Mutex};

use editor_egui::resolvers::{EmbedResolver, EmbedState};
use egui::accesskit::Role;
use egui::{CornerRadius, Rect, Ui, Vec2};
use egui_kittest::kittest::Queryable as _;
use support::{editor, find, harness_with};

/// Has every image at 600 by 400 points, except `missing.png`.
struct Pictures;

impl EmbedResolver for Pictures {
    fn state(&self, url: &str) -> EmbedState {
        if url == "missing.png" {
            EmbedState::NotFound
        } else {
            EmbedState::Loaded(Vec2::new(600.0, 400.0))
        }
    }

    fn show(&self, _ui: &mut Ui, _url: &str, _rect: Rect, _rounding: CornerRadius) {}

    fn prefetch(&self, _url: &str) {}

    fn seq(&self) -> u64 {
        1
    }
}

/// Where the last frame drew the image whose source is `source`.
fn drawn_image(editor: &editor_egui::Editor, source: &str) -> Rect {
    let range = find(editor, source);
    editor
        .painted_spans()
        .into_iter()
        .find(|span| !span.glyphs && span.source_range == range)
        .unwrap_or_else(|| panic!("{source:?} wasn't drawn as an image"))
        .rect
}

/// Remote images that load only when the editor asks, recording each ask.
#[derive(Clone, Default)]
struct Remote {
    loads: Arc<Mutex<Vec<String>>>,
}

impl EmbedResolver for Remote {
    fn state(&self, url: &str) -> EmbedState {
        if url.ends_with("bad.png") {
            EmbedState::Failed
        } else if self.loads.lock().unwrap().iter().any(|load| load == url) {
            EmbedState::Loaded(Vec2::new(600.0, 400.0))
        } else {
            EmbedState::NotLoaded
        }
    }

    fn show(&self, _ui: &mut Ui, _url: &str, _rect: Rect, _rounding: CornerRadius) {}

    fn prefetch(&self, _url: &str) {}

    fn load(&self, url: &str) {
        self.loads.lock().unwrap().push(url.to_string());
    }

    fn seq(&self) -> u64 {
        self.loads.lock().unwrap().len() as u64
    }
}

#[test]
fn a_remote_image_waits_for_load_image_and_a_failed_one_offers_a_retry() {
    let note = "intro\n\n![remote](https://example.com/pic.png) and ![other](https://example.com/bad.png)\n";
    let remote = Remote::default();
    let resolver = remote.clone();
    let mut harness = harness_with(note, move |editor| {
        editor.set_embed_resolver(Box::new(resolver.clone()));
    });

    assert_eq!(
        support::drawn_in_place(editor(&harness)),
        vec!["Load image", "Couldn\u{2019}t load image. Try again"]
    );
    assert!(
        remote.loads.lock().unwrap().is_empty(),
        "nothing loads by itself"
    );

    harness
        .get_by_role_and_label(Role::Button, "Load image")
        .click();
    harness.run_steps(3);
    assert_eq!(
        *remote.loads.lock().unwrap(),
        vec!["https://example.com/pic.png".to_string()]
    );
    drawn_image(editor(&harness), "![remote](https://example.com/pic.png)");

    harness
        .get_by_role_and_label(Role::Button, "Couldn\u{2019}t load image. Try again")
        .click();
    harness.run_steps(3);
    assert_eq!(remote.loads.lock().unwrap().len(), 2);
    assert_eq!(editor(&harness).text(), note);
}

/// Has every document's page 1,200 points wide: `letter.pdf` and
/// `a4.pdf` as tall as those papers, `tall.pdf` 14,000 by 612 points, as
/// a web page saved as PDF. `letter.png` is an image of the Letter page.
struct Pages;

impl EmbedResolver for Pages {
    fn state(&self, url: &str) -> EmbedState {
        let tall = |height: f32| Vec2::new(1200.0, 1200.0 * height);
        match url {
            "letter.png" => EmbedState::Loaded(tall(11.0 / 8.5)),
            "letter.pdf" => EmbedState::Page(tall(11.0 / 8.5)),
            "a4.pdf" => EmbedState::Page(tall(std::f32::consts::SQRT_2)),
            "tall.pdf" => EmbedState::Page(tall(14000.0 / 612.0)),
            _ => EmbedState::NotFound,
        }
    }

    fn show(&self, _ui: &mut Ui, _url: &str, _rect: Rect, _rounding: CornerRadius) {}

    fn prefetch(&self, _url: &str) {}

    fn seq(&self) -> u64 {
        1
    }
}

/// Where a note that embeds only `source`, below the line the caret is on,
/// draws it.
fn drawn_alone(source: &'static str) -> Rect {
    let note = format!("intro\n\n{source}\n").leak();
    let harness = harness_with(note, |editor| {
        editor.set_embed_resolver(Box::new(Pages));
    });
    drawn_image(editor(&harness), source)
}

#[test]
fn a_page_taller_than_a4_lays_out_as_wide_as_a4_and_runs_past_the_view() {
    let view = 600.0;
    // A paper page fits the view as an image does.
    let letter = drawn_alone("![[letter.pdf]]");
    assert_eq!(letter, drawn_alone("![[letter.png]]"));
    assert!(letter.height() < view, "{letter:?}");
    let a4 = drawn_alone("![[a4.pdf]]");
    assert!(a4.height() < view, "{a4:?}");

    // A longer page keeps A4's width and its own shape.
    let tall = drawn_alone("![[tall.pdf]]");
    assert!((tall.width() - a4.width()).abs() < 0.01, "{tall:?} {a4:?}");
    let shape = tall.height() / tall.width();
    assert!((shape - 14000.0 / 612.0).abs() < 0.01, "{tall:?}");
    assert!(tall.height() > 10.0 * view, "{tall:?}");

    // A width written in the note still sets it.
    let narrow = drawn_alone("![[tall.pdf|300]]");
    assert!((narrow.width() - 300.0).abs() < 0.01, "{narrow:?}");
    assert!((narrow.height() - 300.0 * 14000.0 / 612.0).abs() < 1.0);
}

#[test]
fn images_draw_inline_at_their_size_or_the_width_an_embed_asks_for() {
    let note =
        "intro\n\n![Alt](pic.png)\n\n![[photo.jpg|300]]\n\n![[photo.jpg]] and ![[missing.png]]\n";
    let harness = harness_with(note, |editor| {
        editor.set_embed_resolver(Box::new(Pictures));
    });
    let editor = editor(&harness);

    let markdown = drawn_image(editor, "![Alt](pic.png)");
    assert_eq!(markdown.width(), 600.0);
    let narrow = drawn_image(editor, "![[photo.jpg|300]]");
    assert_eq!((narrow.width(), narrow.height()), (300.0, 200.0));
    let natural = drawn_image(editor, "![[photo.jpg]]");
    assert_eq!(natural.width(), 600.0);
    drawn_image(editor, "![[missing.png]]");
    assert_eq!(editor.text(), note);
}
