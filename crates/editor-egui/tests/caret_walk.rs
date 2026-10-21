//! The caret walks through every position of every note in `editor-core`'s
//! syntax corpus while the editor draws a frame at each. Nothing panics and
//! each note keeps its bytes.

#[path = "../../editor-core/tests/corpus/mod.rs"]
mod corpus;

use editor_core::offset_types::Grapheme;
use editor_core::unicode_segs;
use editor_egui::input::{Event, Location, Region};
use editor_egui::resolvers::{EmbeddedNote, LinkResolver, LinkState, ResolvedLink};
use editor_egui::{Editor, MdConfig};
use egui_kittest::Harness;

/// Finds every link and embeds every note, so embeds draw as blocks.
struct Everything;

impl LinkResolver for Everything {
    fn resolve_link(&self, url: &str) -> Option<ResolvedLink> {
        Some(ResolvedLink::File(url.into()))
    }

    fn resolve_wikilink(&self, title: &str) -> Option<String> {
        Some(format!("{title}.md"))
    }

    fn link_state(&self, _url: &str) -> LinkState {
        LinkState::Normal
    }

    fn wikilink_state(&self, _title: &str) -> LinkState {
        LinkState::Normal
    }

    fn embed(&self, target: &str) -> Option<EmbeddedNote> {
        Some(EmbeddedNote {
            title: target.into(),
            text: "Some **text** of the note\nover two lines.".into(),
        })
    }
}

#[test]
fn the_editor_draws_every_caret_position_without_changing_the_note() {
    for (name, note) in corpus::NOTES {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(800.0, 600.0))
            .with_context_setup(|ctx| {
                editor_egui::register_font_system(ctx, editor_egui::fonts::default_font_database());
            })
            .build_ui_state(
                move |ui, editor: &mut Option<Editor>| {
                    editor
                        .get_or_insert_with(|| {
                            let mut editor = Editor::new(ui.ctx(), note, MdConfig::default());
                            editor.set_link_resolver(Box::new(Everything));
                            editor
                        })
                        .show(ui);
                },
                None,
            );
        harness.run_steps(2);
        let positions = unicode_segs::calc(note).last_cursor_position().0;
        for offset in 0..=positions {
            harness
                .state_mut()
                .as_mut()
                .expect("the first frame created the editor")
                .push_event(Event::Select {
                    region: Region::Location(Location::Grapheme(Grapheme(offset))),
                });
            harness.run_steps(1);
            let editor = harness.state().as_ref().unwrap();
            assert_eq!(editor.selection().1, Grapheme(offset), "{name}");
            assert_eq!(
                editor.text(),
                *note,
                "{name}: the caret at {offset} changed it"
            );
        }
    }
}
