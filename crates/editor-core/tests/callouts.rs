//! Seam 2: a quote that starts with `[!type]` is a callout. It draws
//! labeled with its type, hides its `>` markers and its `[!type]` marker,
//! and shows its whole source while the caret is anywhere inside it.

use editor_core::MdDoc;
use editor_core::constructs::{Construct, ConstructKind};
use editor_core::offset_types::Grapheme;

fn constructs_with_caret(text: &str, caret: Option<usize>) -> Vec<Construct> {
    let mut doc = MdDoc::new(text);
    doc.reveal_selection = caret.map(|caret| (Grapheme(caret), Grapheme(caret)));
    doc.bump_reveal_seq();
    doc.constructs()
}

fn callouts(text: &str, caret: Option<usize>) -> Vec<Construct> {
    constructs_with_caret(text, caret)
        .into_iter()
        .filter(|construct| construct.kind == ConstructKind::Callout)
        .collect()
}

/// The source text of `range` in an ASCII note.
fn source(text: &str, range: (Grapheme, Grapheme)) -> &str {
    &text[range.0.0..range.1.0]
}

/// Each callout's source, hidden syntax and label.
fn described(text: &str) -> Vec<(&str, Vec<&str>, Option<String>)> {
    callouts(text, None)
        .into_iter()
        .map(|callout| {
            (
                source(text, callout.range),
                callout
                    .syntax
                    .iter()
                    .map(|range| source(text, *range))
                    .collect(),
                callout.label,
            )
        })
        .collect()
}

#[test]
fn a_quote_that_starts_with_a_type_draws_as_a_callout_labeled_with_it() {
    let text = "> [!note] Title\n> body\n\nafter\n";

    assert_eq!(
        described(text),
        vec![(
            "> [!note] Title\n> body",
            vec!["> ", "[!note]", "> "],
            Some("Note".to_string())
        )]
    );
    assert!(
        constructs_with_caret(text, None)
            .iter()
            .all(|construct| construct.kind != ConstructKind::BlockQuote),
        "a callout isn't also a plain quote"
    );
}

#[test]
fn any_type_counts_with_an_optional_fold_sign() {
    let text = "> [!info]- Folded\n> body\n\n> [!my-type]+\n> open\n\n>[!tip] tight\n";

    assert_eq!(
        described(text),
        vec![
            (
                "> [!info]- Folded\n> body",
                vec!["> ", "[!info]-", "> "],
                Some("Info".to_string())
            ),
            (
                "> [!my-type]+\n> open",
                vec!["> ", "[!my-type]+", "> "],
                Some("My-type".to_string())
            ),
            (
                ">[!tip] tight",
                vec![">", "[!tip]"],
                Some("Tip".to_string())
            ),
        ]
    );
}

#[test]
fn quotes_that_do_not_start_with_a_type_stay_quotes() {
    for text in [
        "> not [!a] callout\n",
        "> `[!note]` in code\n",
        "> [!] no type\n",
        "> # [!note] heading\n",
        "[!note] not quoted\n",
    ] {
        assert_eq!(described(text), vec![], "{text:?}");
    }
}

#[test]
fn a_callout_inside_a_quote_is_the_inner_quote() {
    let text = "> > [!tip] Inner\n> > body\n";

    assert_eq!(
        described(text),
        vec![(
            "> [!tip] Inner\n> > body",
            vec!["> ", "[!tip]", "> "],
            Some("Tip".to_string())
        )]
    );
}

/// The first callout in `doc`.
fn first_callout(doc: &mut MdDoc) -> editor_core::callouts::Callout {
    use editor_core::comrak::Arena;

    let arena = Arena::new();
    let root = doc.reparse(&arena);
    root.descendants()
        .find_map(|node| doc.callout(node))
        .expect("the note holds a callout")
}

#[test]
fn a_callout_reads_its_type_fold_and_title() {
    let text = "> [!warning]-  A title \n> body\n";
    let mut doc = MdDoc::new(text);

    let callout = first_callout(&mut doc);

    assert_eq!(callout.kind, "warning");
    assert_eq!(callout.fold, Some('-'));
    assert_eq!(source(text, callout.marker), "[!warning]-");
    assert_eq!(source(text, callout.title), "A title");
}

#[test]
fn a_callout_shows_its_whole_source_while_the_caret_is_anywhere_inside() {
    let text = "intro\n\n> [!warning]- Careful\n> body\n> more\n\nafter\n";
    let start = text.find('>').unwrap();
    let end = text.find("more").unwrap() + "more".len();

    for caret in 0..=text.len() {
        let callout = callouts(text, Some(caret)).remove(0);
        assert_eq!(
            callout.revealed,
            (start..=end).contains(&caret),
            "caret at {caret}"
        );
    }
}
