//! Images draw inline from `![alt](path)` and `![[image.png]]`, and
//! `![[image.png|300]]` sets the width. The embedding app loads them.

mod support;

use editor_egui::resolvers::{EmbedResolver, EmbedState};
use egui::{CornerRadius, Rect, Ui, Vec2};
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
