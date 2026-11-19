//! Seam 2: the reveal rules decide which syntax shows, from the caret alone.

use editor_core::MdDoc;
use editor_core::comrak::Arena;
use editor_core::comrak::nodes::NodeValue;
use editor_core::offset_types::Grapheme;

/// Whether the first node of `kind` reveals its syntax with the caret at
/// `caret`.
fn revealed_with_caret_at(text: &str, caret: usize, kind: fn(&NodeValue) -> bool) -> bool {
    let mut doc = MdDoc::new(text);
    doc.reveal_selection = Some((Grapheme(caret), Grapheme(caret)));
    let arena = Arena::new();
    let root = doc.reparse(&arena);
    let node = root
        .descendants()
        .find(|node| kind(&node.data.borrow().value))
        .expect("the construct parses");
    doc.node_revealed(node)
}

#[test]
fn emphasis_reveals_its_markers_while_the_caret_is_inside() {
    let strong = |value: &NodeValue| matches!(value, NodeValue::Strong);
    assert!(revealed_with_caret_at("a **bold** word", 5, strong));
    assert!(!revealed_with_caret_at("a **bold** word", 13, strong));
}

#[test]
fn a_link_reveals_its_destination_while_the_caret_is_inside() {
    let link = |value: &NodeValue| matches!(value, NodeValue::Link(_));
    assert!(revealed_with_caret_at(
        "see [docs](https://example.com) now",
        7,
        link
    ));
    assert!(!revealed_with_caret_at(
        "see [docs](https://example.com) now",
        0,
        link
    ));
}

#[test]
fn nothing_reveals_without_a_caret() {
    let mut doc = MdDoc::new("a **bold** word");
    doc.reveal_selection = None;
    let arena = Arena::new();
    let root = doc.reparse(&arena);
    assert!(root.descendants().all(|node| !doc.node_revealed(node)));
}
