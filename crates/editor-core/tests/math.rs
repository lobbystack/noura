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

#[test]
fn display_math_keeps_lines_that_look_like_block_syntax() {
    // A long equation often continues on a line that starts with `+`, `-`
    // or `=`. Inside `$$` none of these start a list, quote, heading or
    // code block, as in today's editor.
    for body in [
        "a\n+ b\n- c\n* d",
        "x\n---",
        "y\n===",
        "> q\n# h\n1. i\n2) j",
        "```\n~~~\n<div>\n| t |",
        "a\n   \n\tb",
    ] {
        let text = format!("intro\n\n$$\n{body}\n$$\n\nafter *em*\n");
        let block = format!("$$\n{body}\n$$");
        assert_eq!(
            math(&text),
            vec![(true, block.as_str(), vec!["$$", "$$"])],
            "{body:?}"
        );
        let kinds: Vec<ConstructKind> = constructs_with_caret(&text, None)
            .into_iter()
            .map(|construct| construct.kind)
            .filter(|kind| !matches!(kind, ConstructKind::Math { .. }))
            .collect();
        assert_eq!(kinds, vec![ConstructKind::Emphasis], "{body:?}");
    }
}

#[test]
fn a_dollar_line_in_code_opens_no_math() {
    let text = "```sh\n$$\n- a\n$$\n```\n\n    $$\n    - b\n    $$\n";

    assert_eq!(math(text), vec![]);
}

#[test]
fn display_math_opened_inside_a_paragraph_still_holds_its_lines() {
    let text = "Let\n$$\nx\n- y\n$$\nso\n";

    assert_eq!(math(text), vec![(true, "$$\nx\n- y\n$$", vec!["$$", "$$"])]);
}

#[test]
fn display_math_with_an_empty_line_inside_leaves_the_lines_around_it_alone() {
    let text = "$$\nx\n\n# Heading\n- item\ny $$\n";

    let kinds: Vec<ConstructKind> = constructs_with_caret(text, None)
        .into_iter()
        .map(|construct| construct.kind)
        .collect();
    assert!(
        kinds
            .iter()
            .any(|kind| matches!(kind, ConstructKind::Heading { .. })),
        "{kinds:?}"
    );
}
