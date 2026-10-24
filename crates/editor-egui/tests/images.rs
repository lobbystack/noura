//! Images draw inline from `![alt](path)` and `![[image.png]]`, and
//! `![[image.png|300]]` sets the width. The embedding app loads them.

mod support;

use std::sync::{Arc, Mutex};

use editor_egui::resolvers::{EmbedResolver, EmbedState};
use egui::accesskit::Role;
use egui::{CornerRadius, Rect, Ui, Vec2};
use egui_kittest::kittest::Queryable as _;
use support::{editor, find, harness_with};

/// Has every image at 600 by 400 pixels, except `missing.png`.
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

#[test]
fn images_draw_inline_at_their_size_or_the_width_an_embed_asks_for() {
    let note =
        "intro\n\n![Alt](pic.png)\n\n![[photo.jpg|300]]\n\n![[photo.jpg]] and ![[missing.png]]\n";
    let harness = harness_with(note, |editor| {
        editor.set_embed_resolver(Box::new(Pictures));
    });
    let editor = editor(&harness);
    let ppp = harness.ctx.pixels_per_point();

    let markdown = drawn_image(editor, "![Alt](pic.png)");
    assert_eq!(markdown.width(), 600.0 / ppp);
    let narrow = drawn_image(editor, "![[photo.jpg|300]]");
    assert_eq!((narrow.width(), narrow.height()), (300.0, 200.0));
    let natural = drawn_image(editor, "![[photo.jpg]]");
    assert_eq!(natural.width(), 600.0 / ppp);
    drawn_image(editor, "![[missing.png]]");
    assert_eq!(editor.text(), note);
}
