//! Links by title through the app (ADR 0005): a rename in the file tree
//! rewrites the links that led to the file, an open note keeps the
//! rewritten text, and a file Noura couldn't rewrite shows among the
//! workspace's problems in the Inbox.

use crate::support;
use app_core::OpenAs;
use support::{Fixture, note};

const GARDEN_ID: &str = "01jbcdefghjkmnpqrstvwxyz0h";

#[test]
fn a_rename_in_the_tree_rewrites_links_and_a_file_it_cant_rewrite_is_a_problem() {
    let garden = note(GARDEN_ID, "Beds.\n");
    let fixture = Fixture::new(&[
        ("Garden.md", &garden),
        ("Plan.md", b"Start with [[Garden]].\n"),
        // A title with a quote can't stand in this single-quoted value as
        // it is, and the file has no ID, so Noura leaves it alone.
        ("Quoted.md", b"---\nsee: '[[Garden]]'\n---\nBody.\n"),
    ]);
    let mut app = fixture.app();
    app.open_file("Plan.md", OpenAs::Preview);
    app.start_rename("Garden.md");

    assert!(app.commit_rename("Bob's garden"));
    app.rescan();

    assert_eq!(fixture.read("Plan.md"), b"Start with [[Bob's garden]].\n");
    assert_eq!(
        fixture.read("Quoted.md"),
        b"---\nsee: '[[Garden]]'\n---\nBody.\n"
    );
    support::plugins::wait_until(&mut app, "the problem shows", |app| {
        app.problems()
            .iter()
            .any(|problem| problem.code == "stale_link" && problem.paths == ["Quoted.md"])
    });
    let problem = app
        .problems()
        .iter()
        .find(|problem| problem.code == "stale_link")
        .unwrap();
    assert_eq!(
        problem.message,
        "Noura couldn't update the links in Quoted.md, so check them."
    );

    // Once you fix the link, the problem goes.
    support::write(
        &fixture.root,
        "Quoted.md",
        b"---\nsee: \"[[Bob's garden]]\"\n---\nBody.\n",
    );
    app.rescan();
    support::plugins::wait_until(&mut app, "the problem goes", |app| {
        app.problems()
            .iter()
            .all(|problem| problem.code != "stale_link")
    });
}
