//! Seam 2: Obsidian's inline syntax. `==highlight==` draws highlighted,
//! `%%comment%%` hides away from the caret, `#tags` and end-of-line
//! `^block-ids` draw styled, and `<u>` draws underlined. None of them
//! match inside code spans, code blocks, math or URLs.

use editor_core::MdDoc;
use editor_core::constructs::{Construct, ConstructKind};
use editor_core::offset_types::Grapheme;

fn constructs_with_caret(text: &str, caret: Option<usize>) -> Vec<Construct> {
    let mut doc = MdDoc::new(text);
    doc.reveal_selection = caret.map(|caret| (Grapheme(caret), Grapheme(caret)));
    doc.bump_reveal_seq();
    doc.constructs()
}

/// The source text of `range` in `text`.
fn source(text: &str, range: (Grapheme, Grapheme)) -> &str {
    let (start, end) = editor_core::unicode_segs::calc(text).range_to_byte(range);
    &text[start.0..end.0]
}

/// Each construct of `kind` in `text`: its source and its hidden syntax.
fn found(text: &str, kind: ConstructKind) -> Vec<(&str, Vec<&str>)> {
    constructs_with_caret(text, None)
        .into_iter()
        .filter(|construct| construct.kind == kind)
        .map(|construct| {
            (
                source(text, construct.range),
                construct
                    .syntax
                    .iter()
                    .map(|range| source(text, *range))
                    .collect(),
            )
        })
        .collect()
}

/// Walks the caret through `text` and checks that each construct of `kind`
/// shows its syntax exactly while the caret is inside it or at its edges.
fn walk(text: &str, kind: ConstructKind) {
    for caret in 0..=text.len() {
        for construct in constructs_with_caret(text, Some(caret))
            .into_iter()
            .filter(|construct| construct.kind == kind)
        {
            assert_eq!(
                construct.revealed,
                (construct.range.0.0..=construct.range.1.0).contains(&caret),
                "{:?} with the caret at {caret}",
                source(text, construct.range)
            );
        }
    }
}

const HIGHLIGHTS: &str = "A ==highlight== here, ==two words== there, and ==**bold** inside==.\n\n\
    Not in `==code==`, $a==b==c$ or https://example.com/a==b==c.\n";

#[test]
fn highlights_draw_highlighted_and_hide_their_markers_away_from_the_caret() {
    assert_eq!(
        found(HIGHLIGHTS, ConstructKind::Highlight),
        vec![
            ("==highlight==", vec!["==", "=="]),
            ("==two words==", vec!["==", "=="]),
            ("==**bold** inside==", vec!["==", "=="]),
        ]
    );
    walk(HIGHLIGHTS, ConstructKind::Highlight);
}

#[test]
fn comments_hide_away_from_the_caret() {
    let text = "before %%secret **bold**%% after %%two%%\n";

    assert_eq!(
        found(text, ConstructKind::Comment),
        vec![
            ("%%secret **bold**%%", vec!["%%secret **bold**%%"]),
            ("%%two%%", vec!["%%two%%"]),
        ]
    );
    walk(text, ConstructKind::Comment);
}

#[test]
fn tags_draw_styled() {
    let text = "#start, a #tag, #nested/tag-2 and #éclair but not #2024, a#b or # heading\n\n# Title #inner\n";

    assert_eq!(
        found(text, ConstructKind::Tag),
        vec![
            ("#start", vec![]),
            ("#tag", vec![]),
            ("#nested/tag-2", vec![]),
            ("#éclair", vec![]),
            ("#inner", vec![]),
        ]
    );
}

#[test]
fn block_ids_at_the_end_of_a_line_draw_styled() {
    let text = "Ends with ^abc-123\n\nnot ^mid line\n\n- item ^item1\n\n^alone\n";

    assert_eq!(
        found(text, ConstructKind::BlockId),
        vec![("^abc-123", vec![]), ("^item1", vec![]), ("^alone", vec![])]
    );
}

#[test]
fn underline_tags_draw_underlined_and_hide_away_from_the_caret() {
    let text = "A <u>plain</u> and <u>under **bold**</u> word, <span>not</span>.\n";

    assert_eq!(
        found(text, ConstructKind::Underline),
        vec![
            ("<u>plain</u>", vec!["<u>", "</u>"]),
            ("<u>under **bold**</u>", vec!["<u>", "</u>"]),
        ]
    );
    walk(text, ConstructKind::Underline);
}

#[test]
fn none_match_inside_code_math_or_urls() {
    let text = "`#tag %%c%% ==h== <u>x</u> ^id` and $#x %%y%%$ and https://example.com/#tag%%z%%\n\n\
        ```\n#tag %%c%% ==h== <u>x</u> ^id\n```\n\n\
        $$\n#tag %%c%%\n$$\n\n\
        [label](https://example.com/#tag%%c%%)\n";

    for kind in [
        ConstructKind::Highlight,
        ConstructKind::Comment,
        ConstructKind::Tag,
        ConstructKind::BlockId,
        ConstructKind::Underline,
    ] {
        assert_eq!(found(text, kind), vec![], "{kind:?}");
    }
}
