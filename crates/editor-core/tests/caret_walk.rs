//! Seam 2: the caret walks through every position of every note in the
//! syntax corpus, unknown syntax included. At each position the editor
//! works out what it draws without a panic, and the note keeps its bytes.

mod corpus;

use editor_core::MdDoc;
use editor_core::input::{Event, Location, NoLayout, Region};
use editor_core::offset_types::Grapheme;
use editor_core::unicode_segs;

#[test]
fn the_caret_walks_every_note_without_changing_it() {
    for (name, note) in corpus::NOTES {
        let mut doc = MdDoc::new(note);
        let positions = unicode_segs::calc(note).last_cursor_position().0;
        for offset in 0..=positions {
            let caret = Grapheme(offset);
            doc.apply(
                [Event::Select {
                    region: Region::Location(Location::Grapheme(caret)),
                }],
                &NoLayout,
            );
            doc.reveal_selection = Some((caret, caret));
            doc.bump_reveal_seq();
            let _ = doc.constructs();
            assert_eq!(
                doc.text(),
                *note,
                "{name}: the caret at {offset} changed the note"
            );
        }
    }
}
