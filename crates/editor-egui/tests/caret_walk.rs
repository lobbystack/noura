//! The caret walks through every position of every note in `editor-core`'s
//! syntax corpus while the editor draws a frame at each. Nothing panics and
//! each note keeps its bytes.

#[path = "../../editor-core/tests/corpus/mod.rs"]
mod corpus;

use editor_core::offset_types::Grapheme;
use editor_core::unicode_segs;
use editor_egui::input::{Event, Location, Region};
use editor_egui::{Editor, MdConfig};
use egui_kittest::Harness;

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
                        .get_or_insert_with(|| Editor::new(ui.ctx(), note, MdConfig::default()))
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
