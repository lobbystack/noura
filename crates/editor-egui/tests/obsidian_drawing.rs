//! The editor draws Obsidian-style syntax: callouts labeled with their type,
//! comments hidden and `<u>` tags hidden away from the caret, and tags and
//! block IDs as written.

mod support;

use editor_core::offset_types::Grapheme;
use editor_egui::resolvers::{EmbeddedNote, LinkResolver, LinkState, ResolvedLink};
use support::{caret_at, drawn_in_place, drew_text, editor, find, harness, hid_text};

const CALLOUT: &str = "intro\n\n> [!warning]- Careful now\n> body text\n\nafter\n";

#[test]
fn a_callout_draws_its_label_in_place_of_its_markers() {
    let harness = harness(CALLOUT);
    let editor = editor(&harness);

    assert_eq!(drawn_in_place(editor), vec!["Warning".to_string()]);
    assert!(hid_text(editor, find(editor, "[!warning]-")));
    assert!(hid_text(editor, find(editor, "> [!")));
    let quote = find(editor, "> body").0;
    assert!(hid_text(editor, (quote, quote + 2)));
    assert!(drew_text(editor, find(editor, "Careful now")));
    assert!(drew_text(editor, find(editor, "body text")));
}

#[test]
fn a_callout_draws_its_source_while_the_caret_is_inside() {
    let mut harness = harness(CALLOUT);
    let body = find(editor(&harness), "body");
    caret_at(&mut harness, Grapheme(body.0.0 + 2));
    let editor = editor(&harness);

    assert!(drawn_in_place(editor).is_empty());
    assert!(drew_text(editor, find(editor, "[!warning]- Careful now")));
    assert!(drew_text(editor, find(editor, "> body")));
}

#[test]
fn comments_and_underline_tags_hide_away_from_the_caret() {
    let mut harness = harness("intro\n\nbefore %%secret **bold**%% and <u>under</u> after\n");
    let comment = find(editor(&harness), "%%secret **bold**%%");
    {
        let editor = editor(&harness);
        assert!(hid_text(editor, comment));
        assert!(hid_text(editor, find(editor, "<u>")));
        assert!(hid_text(editor, find(editor, "</u>")));
        for shown in ["before ", " and ", "under", " after"] {
            assert!(drew_text(editor, find(editor, shown)), "{shown:?}");
        }
    }

    caret_at(&mut harness, Grapheme(comment.0.0 + 4));
    let editor = editor(&harness);
    assert!(drew_text(editor, find(editor, "%%secret ")));
    assert!(drew_text(editor, find(editor, "bold")));
}

/// Knows one note, `Other note`.
struct OneNote;

impl LinkResolver for OneNote {
    fn resolve_link(&self, _url: &str) -> Option<ResolvedLink> {
        None
    }

    fn resolve_wikilink(&self, title: &str) -> Option<String> {
        (title == "Other note").then(|| "Other note.md".into())
    }

    fn link_state(&self, _url: &str) -> LinkState {
        LinkState::Normal
    }

    fn wikilink_state(&self, title: &str) -> LinkState {
        match self.resolve_wikilink(title) {
            Some(_) => LinkState::Normal,
            None => LinkState::Broken {
                message: "missing".into(),
            },
        }
    }

    fn embed(&self, target: &str) -> Option<EmbeddedNote> {
        self.resolve_wikilink(target).map(|_| EmbeddedNote {
            title: "Other note".into(),
            text: "What the other note says.".into(),
        })
    }
}

const EMBEDS: &str =
    "intro\n\n![[Other note]]\n\nprose ![[Other note|alias]] and ![[Missing]] tail\n";

#[test]
fn a_note_embed_alone_on_its_line_draws_as_a_block_and_others_as_links() {
    let harness = support::harness_with(EMBEDS, |editor| {
        editor.set_link_resolver(Box::new(OneNote));
    });
    let editor = editor(&harness);

    assert_eq!(
        drawn_in_place(editor),
        vec![
            "Other note".to_string(),
            "What the other note says.".to_string(),
            "alias".to_string(),
            "Missing".to_string(),
        ]
    );
    for hidden in ["![[Other note]]", "![[Other note|alias]]", "![[Missing]]"] {
        assert!(hid_text(editor, find(editor, hidden)), "{hidden:?}");
    }
    assert_eq!(editor.text(), EMBEDS, "drawing an embed writes nothing");
}

#[test]
fn tags_and_block_ids_draw_as_written() {
    let harness = harness("intro\n\nA #tag and #nested/tag here ^block-1\n");
    let editor = editor(&harness);

    for shown in ["#tag", "#nested/tag", "^block-1"] {
        assert!(drew_text(editor, find(editor, shown)), "{shown:?}");
    }
}
