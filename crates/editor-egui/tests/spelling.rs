//! Spell check in the editor: the words the spell checker the app hands it
//! flags get a wavy underline, in prose only, and not while you're still
//! typing the word. Without a spell checker nothing is marked.

use std::ops::Range;
use std::sync::Arc;

use editor_core::offset_types::Grapheme;
use editor_egui::input::{Event, Location, Region};
use editor_egui::{Editor, MdConfig, SpellChecker};
use egui_kittest::Harness;

const NOTE: &str =
    "# Teh heading\n\nI recieve teh `teh` code and [teh link](https://teh.example).\n";

/// Flags every "teh" and "recieve", as a real spell checker would.
struct Fake;

impl SpellChecker for Fake {
    fn misspelled(&self, text: &str) -> Vec<Range<usize>> {
        let mut found = Vec::new();
        for word in ["teh", "Teh", "recieve"] {
            found.extend(
                text.match_indices(word)
                    .map(|(at, word)| at..at + word.len()),
            );
        }
        found.sort_by_key(|range| range.start);
        found
    }
}

type EditorHarness = Harness<'static, Option<Editor>>;

fn harness(checker: Option<Arc<dyn SpellChecker>>) -> EditorHarness {
    harness_on(NOTE, checker)
}

fn harness_on(note: &'static str, checker: Option<Arc<dyn SpellChecker>>) -> EditorHarness {
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
                        editor.set_spell_checker(checker.clone());
                        editor
                    })
                    .show(ui);
            },
            None,
        );
    harness.run_steps(2);
    harness
}

fn editor(harness: &EditorHarness) -> &Editor {
    harness
        .state()
        .as_ref()
        .expect("the first frame created it")
}

/// The source text of every span the last frame marked as misspelled.
fn marked(harness: &EditorHarness) -> Vec<String> {
    let editor = editor(harness);
    let text = editor.text();
    editor
        .painted_spans()
        .into_iter()
        .filter(|span| span.misspelled)
        .map(|span| {
            let (start, end) = span.source_range;
            // The note is ASCII: graphemes are bytes.
            text[start.0..end.0].to_owned()
        })
        .collect()
}

fn place_caret(harness: &mut EditorHarness, offset: usize) {
    harness
        .state_mut()
        .as_mut()
        .unwrap()
        .push_event(Event::Select {
            region: Region::Location(Location::Grapheme(Grapheme(offset))),
        });
    harness.run_steps(1);
}

#[test]
fn misspelled_words_in_prose_are_marked() {
    let mut harness = harness(Some(Arc::new(Fake)));
    // Away from every word.
    place_caret(&mut harness, NOTE.len() - 1);

    assert_eq!(marked(&harness), ["Teh", "recieve", "teh", "teh"]);
}

#[test]
fn the_word_being_typed_is_not_marked() {
    let mut harness = harness(Some(Arc::new(Fake)));
    let after = NOTE.find("recieve").unwrap() + "recieve".len();

    place_caret(&mut harness, after);

    assert_eq!(marked(&harness), ["Teh", "teh", "teh"]);
}

#[test]
fn without_a_spell_checker_nothing_is_marked() {
    let mut harness = harness(None);
    place_caret(&mut harness, NOTE.len() - 1);

    assert_eq!(marked(&harness), Vec::<String>::new());
}

#[test]
fn turning_spell_check_off_clears_the_marks() {
    let mut harness = harness(Some(Arc::new(Fake)));
    place_caret(&mut harness, NOTE.len() - 1);
    assert!(!marked(&harness).is_empty());

    harness
        .state_mut()
        .as_mut()
        .unwrap()
        .set_spell_checker(None);
    harness.run_steps(1);

    assert_eq!(marked(&harness), Vec::<String>::new());
}

/// Bare URLs and email addresses are links whose text is their address, so
/// the editor leaves them unmarked, angle brackets or not; a labeled
/// link's text is still prose.
#[test]
fn urls_and_email_addresses_are_not_marked() {
    const LINKS: &str = "See https://teh.example/teh/path and www.teh.example, \
        write to teh@teh.example or <https://teh.example/a>, \
        [teh label](https://teh.example) and teh word.\n";
    let mut harness = harness_on(LINKS, Some(Arc::new(Fake)));
    place_caret(&mut harness, LINKS.len() - 1);

    assert_eq!(marked(&harness), ["teh", "teh"]);
    let editor = editor(&harness);
    let marked_at: Vec<usize> = editor
        .painted_spans()
        .into_iter()
        .filter(|span| span.misspelled)
        .map(|span| span.source_range.0.0)
        .collect();
    assert_eq!(
        marked_at,
        [
            LINKS.find("[teh").unwrap() + 1,
            LINKS.find("teh word").unwrap()
        ]
    );
}

/// Tags and block IDs are names, and a comment hides its text away from
/// the caret, so none of them is marked. A word inside `<u>` tags or a
/// highlight is prose.
#[test]
fn tags_block_ids_and_hidden_comments_are_not_marked() {
    const SYNTAX: &str = "Teh #teh tag, <u>teh</u> and ==teh== and %%teh%% end ^teh\n\nlast\n";
    let mut harness = harness_on(SYNTAX, Some(Arc::new(Fake)));
    place_caret(&mut harness, SYNTAX.len() - 1);

    // The text drawn marked; a highlight's padding shares its scope.
    let marked_at: Vec<usize> = editor(&harness)
        .painted_spans()
        .into_iter()
        .filter(|span| span.misspelled && span.glyphs)
        .map(|span| span.source_range.0.0)
        .collect();
    assert_eq!(
        marked_at,
        [
            0,
            SYNTAX.find("<u>teh").unwrap() + 3,
            SYNTAX.find("==teh").unwrap() + 2
        ]
    );
}
