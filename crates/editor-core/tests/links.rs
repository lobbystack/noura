//! Seam 2: link lookups go through the embedding app's resolver, and a link
//! that leads nowhere changes nothing.

use std::fs;

use editor_core::MdDoc;
use editor_core::input::{Event, Location, NoLayout, Region};
use editor_core::offset_types::Grapheme;
use editor_core::resolvers::{LinkResolver, LinkState, OpenedLink, ResolvedLink};

/// Selects `range` and asks the document which link the selection is on.
fn link_target_at(doc: &mut MdDoc, range: (usize, usize)) -> String {
    doc.apply(
        [Event::Select {
            region: Region::BetweenLocations {
                start: Location::Grapheme(Grapheme(range.0)),
                end: Location::Grapheme(Grapheme(range.1)),
            },
        }],
        &NoLayout,
    );
    doc.selection_open_target()
        .expect("the selection is on a link")
}

#[test]
fn a_link_to_another_note_resolves_to_not_found_and_writes_no_file() {
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
            },
            Event::OpenLink {
                url: link,
                wikilink: false,
            },
        ],
        &NoLayout,
    );

    assert_eq!(
        doc.opened_links,
        vec![
            OpenedLink::NotFound {
                target: "Other note".into()
            },
            OpenedLink::NotFound {
                target: "Draft.md".into()
            },
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

/// Resolves wikilinks to a fixed workspace file and web links to themselves.
struct OneNote;

impl LinkResolver for OneNote {
    fn resolve_link(&self, url: &str) -> Option<ResolvedLink> {
        url.starts_with("https://")
            .then(|| ResolvedLink::External(url.into()))
    }

    fn resolve_wikilink(&self, title: &str) -> Option<String> {
        (title == "Other note").then(|| "Other note.md".into())
    }

    fn link_state(&self, _url: &str) -> LinkState {
        LinkState::Normal
    }

    fn wikilink_state(&self, _title: &str) -> LinkState {
        LinkState::Normal
    }
}

#[test]
fn links_the_resolver_finds_report_what_to_open() {
    let mut doc = MdDoc::new("[[Other note]] <https://example.com>\n");
    doc.link_resolver = Box::new(OneNote);

    doc.apply(
        [
            Event::OpenLink {
                url: "Other note".into(),
                wikilink: true,
            },
            Event::OpenLink {
                url: "https://example.com".into(),
                wikilink: false,
            },
        ],
        &NoLayout,
    );

    assert_eq!(
        doc.opened_links,
        vec![
            OpenedLink::File("Other note.md".into()),
            OpenedLink::External("https://example.com".into()),
        ]
    );
}
