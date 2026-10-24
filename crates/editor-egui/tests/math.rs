//! Math draws as its source with the dollar signs hidden away from the
//! caret, and prices stay prose.

mod support;

use support::{drew_text, editor, find, find_after, harness, hid_text};

#[test]
fn math_draws_its_source_and_hides_its_dollar_signs() {
    let note =
        "intro\n\nUse $x^2 + y^2$ and $$E = mc^2$$.\n\n$$\na*b*c\n$$\n\nIt costs $5 or $10.\n";
    let harness = harness(note);
    let editor = editor(&harness);

    for source in ["x^2 + y^2", "E = mc^2", "a*b*c", "It costs $5 or $10."] {
        assert!(drew_text(editor, find(editor, source)), "{source:?}");
    }
    let inline = find(editor, "$x^2");
    assert!(hid_text(editor, (inline.0, inline.0 + 1)));
    let display = find(editor, "$$E");
    assert!(hid_text(editor, (display.0, display.0 + 2)));
    let block = find_after(editor, "$$\n", 30);
    assert!(hid_text(editor, (block.0, block.0 + 2)));
}
