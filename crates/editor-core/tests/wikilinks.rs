//! Seam 2: wikilinks of the form `[[target#heading|alias]]` draw their
//! alias, or their target, hide their brackets away from the caret, and
//! show whether the target exists by asking the embedding app.

use editor_core::MdDoc;
use editor_core::constructs::{Construct, ConstructKind};
use editor_core::offset_types::{Grapheme, RangeExt as _};
use editor_core::resolvers::{LinkResolver, LinkState, ResolvedLink};

/// Knows the notes `target` and `plain`.
struct TwoNotes;

impl LinkResolver for TwoNotes {
    fn resolve_link(&self, _url: &str) -> Option<ResolvedLink> {
        None
    }

    fn resolve_wikilink(&self, title: &str) -> Option<String> {
        let path = title.split('#').next().unwrap_or_default();
        matches!(path, "target" | "plain").then(|| format!("{path}.md"))
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
}

/// The source text of `range` in an ASCII note.
fn source(text: &str, range: (Grapheme, Grapheme)) -> &str {
    &text[range.0.0..range.1.0]
}

fn wikilinks(text: &str) -> Vec<Construct> {
    let mut doc = MdDoc::new(text);
    doc.link_resolver = Box::new(TwoNotes);
    doc.constructs()
        .into_iter()
        .filter(|construct| construct.kind == ConstructKind::WikiLink)
        .collect()
}

#[test]
fn a_wikilink_draws_its_alias_or_its_target_and_hides_its_brackets() {
    let text = "[[target#heading|alias]] and [[plain]] and [[folder/note]]\n";

    let drawn: Vec<(&str, Vec<&str>)> = wikilinks(text)
        .iter()
        .map(|link| {
            let shown = (link.syntax[0].end(), link.syntax[1].start());
            (
                source(text, shown),
                link.syntax
                    .iter()
                    .map(|range| source(text, *range))
                    .collect(),
            )
        })
        .collect();

    assert_eq!(
        drawn,
        vec![
            ("alias", vec!["[[target#heading|", "]]"]),
            ("plain", vec!["[[", "]]"]),
            ("folder/note", vec!["[[", "]]"]),
        ]
    );
}

#[test]
fn a_wikilink_shows_whether_its_target_exists() {
    let text = "[[target#heading|alias]] and [[plain]] and [[folder/note]]\n";

    let links: Vec<(String, bool)> = wikilinks(text)
        .into_iter()
        .map(|link| {
            let link = link.link.expect("a wikilink names a target");
            (link.target, link.exists)
        })
        .collect();

    assert_eq!(
        links,
        vec![
            ("target#heading".to_string(), true),
            ("plain".to_string(), true),
            ("folder/note".to_string(), false),
        ]
    );
}
