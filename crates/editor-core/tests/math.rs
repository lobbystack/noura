//! Seam 2: inline `$...$` and `$$` display math draw as formatted source,
//! their text as written with only the dollar signs hidden away from the
//! caret. Emphasis and tags inside math stay literal, and prose such as
//! "$5 and $10" stays prose.

use editor_core::MdDoc;
use editor_core::constructs::{Construct, ConstructKind};
use editor_core::offset_types::Grapheme;

fn constructs_with_caret(text: &str, caret: Option<usize>) -> Vec<Construct> {
    let mut doc = MdDoc::new(text);
    doc.reveal_selection = caret.map(|caret| (Grapheme(caret), Grapheme(caret)));
    doc.bump_reveal_seq();
    let constructs = doc.constructs();
    assert_eq!(doc.text(), text);
    constructs
}

/// Each math construct: whether it's display math, its source and its
/// hidden syntax.
fn math(text: &str) -> Vec<(bool, &str, Vec<&str>)> {
    constructs_with_caret(text, None)
        .into_iter()
        .filter_map(|construct| match construct.kind {
            ConstructKind::Math { display } => Some((
                display,
                &text[construct.range.0.0..construct.range.1.0],
                construct
                    .syntax
                    .iter()
                    .map(|range| &text[range.0.0..range.1.0])
                    .collect(),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn inline_and_display_math_hide_only_their_dollar_signs() {
    let text = "Use $x^2 + y^2$ here and $$E = mc^2$$ there.\n\n$$\nx^2 + y^2\n$$\n";

    assert_eq!(
        math(text),
        vec![
            (false, "$x^2 + y^2$", vec!["$", "$"]),
            (true, "$$E = mc^2$$", vec!["$$", "$$"]),
            (true, "$$\nx^2 + y^2\n$$", vec!["$$", "$$"]),
        ]
    );
}

#[test]
fn emphasis_and_tags_inside_math_stay_literal() {
    let text = "Inline $a*b*c$ math.\n\n$$\na*b*c #x\n$$\n\nafter\n";

    let kinds: Vec<ConstructKind> = constructs_with_caret(text, None)
        .into_iter()
        .map(|construct| construct.kind)
        .collect();

    assert_eq!(
        kinds,
        vec![
            ConstructKind::Math { display: false },
            ConstructKind::Math { display: true }
        ]
    );
}

#[test]
fn prices_stay_prose() {
    assert_eq!(
        math("It costs $5 or $10 today\n\nUSD $5 and CAD $10\n"),
        vec![]
    );
}

#[test]
fn math_shows_its_dollar_signs_while_the_caret_is_inside() {
    let text = "Use $x^2$ here and $$y$$ there.\n";
    for caret in 0..=text.len() {
        for construct in constructs_with_caret(text, Some(caret)) {
            assert_eq!(
                construct.revealed,
                (construct.range.0.0..=construct.range.1.0).contains(&caret),
                "caret at {caret}"
            );
        }
    }
}
