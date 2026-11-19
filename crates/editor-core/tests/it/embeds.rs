//! Seam 2: a `![[note]]` embed alone on its line draws as a block with the
//! target's title and the start of its text, which the embedding app
//! supplies. An inline or unresolved embed draws as a styled link. Drawing
//! an embed writes nothing.

use editor_core::MdDoc;
use editor_core::constructs::{Construct, ConstructKind};
use editor_core::offset_types::Grapheme;
use editor_core::resolvers::{EmbeddedNote, LinkResolver, LinkState, ResolvedLink};

/// Knows one note, `Other note`, and the folder note `Notes/index`.
struct Notes;

impl LinkResolver for Notes {
    fn resolve_link(&self, _url: &str) -> Option<ResolvedLink> {
        None
    }

    fn resolve_wikilink(&self, title: &str) -> Option<String> {
        matches!(title, "Other note" | "Notes/index").then(|| format!("{title}.md"))
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
            title: target.rsplit('/').next().unwrap_or(target).to_string(),
            text: format!("The text of {target}."),
        })
    }
}

fn embeds_with_caret(text: &str, caret: Option<usize>) -> Vec<Construct> {
    let mut doc = MdDoc::new(text);
    doc.link_resolver = Box::new(Notes);
    doc.reveal_selection = caret.map(|caret| (Grapheme(caret), Grapheme(caret)));
    doc.bump_reveal_seq();
    let constructs = doc
        .constructs()
        .into_iter()
        .filter(|construct| matches!(construct.kind, ConstructKind::Embed { .. }))
        .collect();
    assert_eq!(doc.text(), text, "drawing an embed writes nothing");
    constructs
}

/// The source text of `range` in an ASCII note.
fn source(text: &str, range: (Grapheme, Grapheme)) -> &str {
    &text[range.0.0..range.1.0]
}

/// Each embed: its source, whether it draws as a block, its label and the
/// text it previews.
fn described(text: &str) -> Vec<(&str, bool, Option<String>, Option<String>)> {
    embeds_with_caret(text, None)
        .into_iter()
        .map(|embed| {
            let ConstructKind::Embed { block } = embed.kind else {
                unreachable!()
            };
            assert_eq!(embed.syntax, vec![embed.range], "the whole embed hides");
            (source(text, embed.range), block, embed.label, embed.preview)
        })
        .collect()
}

#[test]
fn a_note_embed_alone_on_its_line_draws_as_a_block_with_its_title_and_text() {
    let text = "![[Notes/index]]\n\n  ![[Other note]]  \n";

    assert_eq!(
        described(text),
        vec![
            (
                "![[Notes/index]]",
                true,
                Some("index".to_string()),
                Some("The text of Notes/index.".to_string())
            ),
            (
                "![[Other note]]",
                true,
                Some("Other note".to_string()),
                Some("The text of Other note.".to_string())
            ),
        ]
    );
}

#[test]
fn inline_and_unresolved_embeds_draw_as_links() {
    let text = "prose ![[Other note]] and ![[Other note|Shown]] tail\n\n![[Missing]]\n\n- ![[Other note]]\n";

    assert_eq!(
        described(text),
        vec![
            (
                "![[Other note]]",
                false,
                Some("Other note".to_string()),
                None
            ),
            (
                "![[Other note|Shown]]",
                false,
                Some("Shown".to_string()),
                None
            ),
            ("![[Missing]]", false, Some("Missing".to_string()), None),
            (
                "![[Other note]]",
                false,
                Some("Other note".to_string()),
                None
            ),
        ]
    );
    let exists: Vec<(String, bool)> = embeds_with_caret(text, None)
        .into_iter()
        .map(|embed| {
            let link = embed.link.expect("an embed names a target");
            (link.target, link.exists)
        })
        .collect();
    assert_eq!(
        exists,
        vec![
            ("Other note".to_string(), true),
            ("Other note".to_string(), true),
            ("Missing".to_string(), false),
            ("Other note".to_string(), true),
        ]
    );
}

#[test]
fn embeds_in_code_stay_literal() {
    let text = "Inline `![[Note]]` code\n\n```\n![[Note]]\n```\n";

    assert_eq!(described(text), vec![]);
}

#[test]
fn an_embed_shows_its_source_while_the_caret_is_inside_or_at_its_edges() {
    let text = "intro\n\n![[Other note]]\n\nprose ![[Missing]] tail\n";
    for caret in 0..=text.len() {
        for embed in embeds_with_caret(text, Some(caret)) {
            assert_eq!(
                embed.revealed,
                (embed.range.0.0..=embed.range.1.0).contains(&caret),
                "{:?} with the caret at {caret}",
                source(text, embed.range)
            );
        }
    }
}
