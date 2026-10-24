//! Seam 2: opening a link reports its target for the embedding app to
//! follow, and changes nothing. A click says whether it asks for a new
//! tab, and Alt-Enter opens the link at the caret.

use std::fs;

use editor_core::MdDoc;
use editor_core::input::{Event, Location, NoLayout, Region};
use editor_core::keymap::{self, Command, Key, Shortcut};
use editor_core::offset_types::Grapheme;
use editor_core::resolvers::OpenedLink;

fn select(doc: &mut MdDoc, range: (usize, usize)) {
    doc.apply(
        [Event::Select {
            region: Region::BetweenLocations {
                start: Location::Grapheme(Grapheme(range.0)),
                end: Location::Grapheme(Grapheme(range.1)),
            },
        }],
        &NoLayout,
    );
}

/// Selects `range` and asks the document which link the selection is on.
fn link_target_at(doc: &mut MdDoc, range: (usize, usize)) -> String {
    select(doc, range);
    doc.selection_open_target()
        .expect("the selection is on a link")
}

fn opened(target: &str, wikilink: bool, new_tab: bool) -> OpenedLink {
    OpenedLink {
        target: target.into(),
        wikilink,
        new_tab,
    }
}

#[test]
fn opening_a_link_reports_its_target_and_writes_no_file() {
    let workspace = tempfile::tempdir().unwrap();
    let note = workspace.path().join("Plan.md");
    let original = "See [[Other note]] and [the draft](Draft.md).\n";
    fs::write(&note, original).unwrap();

    let mut doc = MdDoc::new(&fs::read_to_string(&note).unwrap());
    let wikilink = link_target_at(&mut doc, (6, 16));
    let link = link_target_at(&mut doc, (25, 30));
    assert_eq!(
        (wikilink.as_str(), link.as_str()),
        ("Other note", "Draft.md")
    );

    doc.apply(
        [
            Event::OpenLink {
                url: wikilink,
                wikilink: true,
                new_tab: false,
            },
            Event::OpenLink {
                url: link,
                wikilink: false,
                new_tab: true,
            },
        ],
        &NoLayout,
    );

    assert_eq!(
        doc.opened_links,
        vec![
            opened("Other note", true, false),
            opened("Draft.md", false, true),
        ]
    );
    assert_eq!(doc.text(), original);
    let files: Vec<_> = fs::read_dir(workspace.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(files, vec!["Plan.md"]);
    assert_eq!(fs::read_to_string(&note).unwrap(), original);
}

#[test]
fn alt_enter_opens_the_link_at_the_caret() {
    for apple in [true, false] {
        assert_eq!(
            keymap::command_for(Shortcut::new(Key::Enter).with_alt(), apple),
            Some(Command::OpenLink)
        );
    }
    let open = Command::OpenLink.event().expect("it's an editor event");
    let text = "See [[Other note]], [the draft](Draft.md) and <https://example.com>.\n";

    for (caret, target, wikilink) in [
        (8, "Other note", true),
        (6, "Other note", true),
        (22, "Draft.md", false),
        (34, "Draft.md", false),
        (50, "https://example.com", false),
    ] {
        let mut doc = MdDoc::new(text);
        select(&mut doc, (caret, caret));
        doc.apply([open.clone()], &NoLayout);
        assert_eq!(
            doc.opened_links,
            [opened(target, wikilink, false)],
            "caret at {caret}"
        );
        assert_eq!(doc.text(), text);
    }

    // Away from links, and in a read-only note, as anywhere.
    let mut doc = MdDoc::new(text);
    select(&mut doc, (1, 1));
    doc.apply([open.clone()], &NoLayout);
    assert_eq!(doc.opened_links, []);
    assert_eq!(doc.text(), text);

    let mut doc = MdDoc::new(text);
    doc.readonly = true;
    select(&mut doc, (8, 8));
    doc.apply([open], &NoLayout);
    assert_eq!(doc.opened_links, [opened("Other note", true, false)]);
}
