//! A GFM table draws as a grid of cells, each a cell in the accessibility
//! tree, while the caret is outside it. Clicking a cell puts the caret in
//! that cell's source, and the table then draws its raw rows.

mod support;

use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use support::{drew_text, editor, find, harness, hid_text};

const TABLE: &str = "intro\n\n| Name | Count |\n| --- | --- |\n| pears | 12 |\n\nafter\n";

/// The header cells, then the other cells, that the accessibility tree
/// shows.
fn cells(harness: &support::EditorHarness) -> Vec<String> {
    harness
        .query_all_by_role(Role::ColumnHeader)
        .chain(harness.query_all_by_role(Role::Cell))
        .filter_map(|cell| cell.accesskit_node().label())
        .collect()
}

#[test]
fn a_table_draws_as_a_grid_of_labeled_cells_while_the_caret_is_outside() {
    let harness = harness(TABLE);

    assert_eq!(cells(&harness), vec!["Name", "Count", "pears", "12"]);
    let editor = editor(&harness);
    assert!(hid_text(editor, find(editor, "| --- | --- |")));
    assert!(drew_text(editor, find(editor, "pears")));
}

#[test]
fn clicking_a_cell_puts_the_caret_in_its_source_and_shows_the_raw_rows() {
    let mut harness = harness(TABLE);

    harness.get_by_role_and_label(Role::Cell, "pears").click();
    harness.run_steps(2);

    let editor = editor(&harness);
    let pears = find(editor, "pears");
    let caret = editor.selection().1;
    assert!(
        pears.0 <= caret && caret <= pears.1,
        "the caret at {caret:?} isn't in {pears:?}"
    );
    assert!(drew_text(editor, find(editor, "| --- | --- |")));
    assert!(drew_text(editor, find(editor, "| pears | 12 |")));
    assert!(cells(&harness).is_empty(), "raw rows aren't cells");
}
