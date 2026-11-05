//! The text size from Settings reaches the editor: a note's text draws at
//! that size, and a change applies to a note that is already open.

use editor_egui::{Editor, MdConfig};
use egui_kittest::Harness;

const NOTE: &str = "# Heading\n\nSome plain text.\n";

/// The editor, once the first frame opens it, and the size it shows its
/// text at.
struct Shown {
    editor: Option<Editor>,
    points: f32,
}

type EditorHarness = Harness<'static, Shown>;

/// A harness showing an editor on [`NOTE`] with its text at `points`.
fn harness(points: f32) -> EditorHarness {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(800.0, 600.0))
        .with_context_setup(|ctx| {
            editor_egui::register_font_system(ctx, editor_egui::fonts::default_font_database());
        })
        .build_ui_state(
            |ui, shown: &mut Shown| {
                let editor = shown
                    .editor
                    .get_or_insert_with(|| Editor::new(ui.ctx(), NOTE, MdConfig::default()));
                editor.set_text_size(shown.points);
                editor.show(ui);
            },
            Shown {
                editor: None,
                points,
            },
        );
    harness.run_steps(2);
    harness
}

/// The height of the line that draws `needle`.
fn line_height(harness: &EditorHarness, needle: &str) -> f32 {
    let start = NOTE.find(needle).expect("the note holds the text");
    let editor = harness.state().editor.as_ref().expect("the editor opened");
    editor
        .painted_spans()
        .into_iter()
        .find(|span| span.glyphs && span.source_range.0.0 <= start && start < span.source_range.1.0)
        .map(|span| span.rect.height())
        .expect("the editor drew the text")
}

#[test]
fn text_and_headings_draw_at_the_chosen_size() {
    let [small, default, large] = [15.0, 16.0, 18.0].map(|points| {
        let harness = harness(points);
        (
            line_height(&harness, "Some"),
            line_height(&harness, "Heading"),
        )
    });
    assert!(
        small.0 < default.0 && default.0 < large.0,
        "{small:?} {default:?} {large:?}"
    );
    assert!(
        small.1 < default.1 && default.1 < large.1,
        "{small:?} {default:?} {large:?}"
    );
    let ratio = large.0 / small.0;
    assert!((ratio - 18.0 / 15.0).abs() < 0.05, "{ratio}");
}

#[test]
fn a_new_size_applies_to_an_open_note() {
    let mut harness = harness(15.0);
    let before = line_height(&harness, "Some");

    harness.state_mut().points = 18.0;
    harness.run_steps(2);

    let after = line_height(&harness, "Some");
    assert!(after > before, "{before} then {after}");
    assert_eq!(
        harness.state().editor.as_ref().map(Editor::text),
        Some(NOTE)
    );
}
