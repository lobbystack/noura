//! Seam 2: a GFM table draws as a grid while the caret is outside it and
//! as its raw rows while the caret is anywhere inside.

use editor_core::MdDoc;
use editor_core::constructs::{Construct, ConstructKind};
use editor_core::offset_types::Grapheme;

fn tables_with_caret(text: &str, caret: usize) -> Vec<Construct> {
    let mut doc = MdDoc::new(text);
    doc.reveal_selection = Some((Grapheme(caret), Grapheme(caret)));
    doc.bump_reveal_seq();
    doc.constructs()
        .into_iter()
        .filter(|construct| construct.kind == ConstructKind::Table)
        .collect()
}

#[test]
fn a_table_shows_its_raw_rows_while_the_caret_is_anywhere_inside() {
    let text = "intro\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\nafter\n";
    let start = text.find('|').unwrap();
    let end = text.rfind('|').unwrap() + 1;

    for caret in 0..=text.len() {
        let tables = tables_with_caret(text, caret);
        assert_eq!(tables.len(), 1);
        assert_eq!((tables[0].range.0.0, tables[0].range.1.0), (start, end));
        assert_eq!(
            tables[0].revealed,
            (start..=end).contains(&caret),
            "caret at {caret}"
        );
    }
}

#[test]
fn a_table_hides_its_pipes_and_delimiter_row_in_the_grid() {
    let text = "| A | B |\n| :- | -: |\n| 1 | 2 |\n";

    let table = tables_with_caret(text, text.len()).remove(0);
    let syntax: Vec<&str> = table
        .syntax
        .iter()
        .map(|range| &text[range.0.0..range.1.0])
        .collect();

    assert_eq!(syntax, vec!["|", "|", "|", "| :- | -: |", "|", "|", "|"]);
}
