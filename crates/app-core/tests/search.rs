//! Seam 1: full-text search over a real workspace folder. Every query is
//! plain text: each word must match, a partial word matches as a prefix,
//! and accents don't matter.

mod support;

use app_core::{SearchHit, SearchQuery, Workspace, WorkspaceService};
use support::Fixture;

fn open(fixture: &Fixture) -> Workspace {
    WorkspaceService::new(fixture.app_data.path())
        .open_folder(&fixture.root)
        .unwrap()
}

fn search(workspace: &Workspace, text: &str) -> Vec<SearchHit> {
    workspace
        .search(&SearchQuery {
            text: text.to_owned(),
            ..SearchQuery::default()
        })
        .unwrap()
}

/// The titles of the hits for `text`, sorted, for checks that don't care
/// about the order.
fn found(workspace: &Workspace, text: &str) -> Vec<String> {
    let mut titles: Vec<String> = search(workspace, text)
        .into_iter()
        .map(|hit| hit.title)
        .collect();
    titles.sort();
    titles
}

#[test]
fn search_finds_words_in_titles_file_names_properties_and_bodies() {
    let fixture = Fixture::new(&[
        ("Recipes/Risotto.md", b"Add the saffron last\n"),
        ("Café notes.md", b"Opening hours\n"),
        ("Groceries.md", b"# Weekly shopping\n\nMilk and bread\n"),
        (
            "Ideas.md",
            b"---\nstatus: brainstorming\n---\nA loose idea\n",
        ),
        (
            "Plan.md",
            "---\nid: note_01jbcdefghjkmnpqrstvwxyz0a\ntype: note\ncolor: turquoise\n\
             created: 2026-10-20T10:00:00Z\nupdated: 2026-10-20T10:00:00Z\n---\n\nUn café noir\n"
                .as_bytes(),
        ),
    ]);
    let workspace = open(&fixture);

    // File names, a heading, a body, and properties with and without an ID.
    assert_eq!(found(&workspace, "groceries"), ["Groceries"]);
    assert_eq!(found(&workspace, "weekly"), ["Groceries"]);
    assert_eq!(found(&workspace, "bread"), ["Groceries"]);
    assert_eq!(found(&workspace, "turquoise"), ["Plan"]);
    assert_eq!(found(&workspace, "brainstorming"), ["Ideas"]);
    // Every word must match.
    assert_eq!(found(&workspace, "saffron last"), ["Risotto"]);
    assert!(found(&workspace, "saffron missing").is_empty());
    // A partial word matches as a prefix, but not from the middle.
    assert_eq!(found(&workspace, "saff"), ["Risotto"]);
    assert_eq!(found(&workspace, "brain"), ["Ideas"]);
    assert!(found(&workspace, "affron").is_empty());
    // Accents don't matter, either way round.
    assert_eq!(found(&workspace, "cafe"), ["Café notes", "Plan"]);
    assert_eq!(found(&workspace, "CAFÉ"), ["Café notes", "Plan"]);
    // Nothing typed finds nothing.
    assert!(found(&workspace, "   ").is_empty());
}

#[test]
fn a_match_in_the_title_ranks_above_a_match_in_the_body_alone() {
    let fixture = Fixture::new(&[
        // Short notes that say "saffron" again and again, which a ranking
        // by body text alone would put first.
        ("Paella.md", b"saffron saffron saffron\n"),
        ("Rice.md", b"Saffron rice, saffron broth\n"),
        (
            "Saffron.md",
            b"A spice from crocus flowers, picked by hand in the autumn and dried \
              slowly over a low fire before it reaches the market stalls.\n",
        ),
    ]);
    let workspace = open(&fixture);

    let titles: Vec<String> = search(&workspace, "saffron")
        .into_iter()
        .map(|hit| hit.title)
        .collect();
    assert_eq!(titles[0], "Saffron");
    assert_eq!(titles.len(), 3);
}

#[test]
fn a_heading_ranks_as_body_text_below_the_title() {
    let fixture = Fixture::new(&[
        // The title is the file name; a heading inside the file is
        // ordinary text.
        (
            "Paella.md",
            b"# Saffron rice\n\nA rice dish from crocus country, cooked by hand in the \
              autumn and served slowly.\n",
        ),
        (
            "Saffron.md",
            b"A spice from crocus flowers, picked by hand in the autumn and dried slowly.\n",
        ),
    ]);
    let workspace = open(&fixture);

    let hits = search(&workspace, "saffron");
    let titles: Vec<&str> = hits.iter().map(|hit| hit.title.as_str()).collect();
    assert_eq!(titles, ["Saffron", "Paella"]);
    // The heading is the text that matches, so the hit shows it.
    assert!(
        hits[1].snippet.contains("Saffron rice"),
        "{}",
        hits[1].snippet
    );
    assert!(!hits[1].snippet.contains('\u{2}'), "{:?}", hits[1].snippet);
}

#[test]
fn each_hit_shows_its_title_type_folder_and_the_matching_text() {
    let fixture = Fixture::new(&[
        (
            "Projects/Launch/Kickoff.md",
            b"---\nid: task_01jbcdefghjkmnpqrstvwxyz0a\ntype: task\nstatus: todo\n\
              created: 2026-10-20T10:00:00Z\nupdated: 2026-10-20T10:00:00Z\n---\n\n\
              Book the room. Order saffron buns for the team. Send the agenda.\n",
        ),
        ("Buns.md", b"Plain buns, no saffron\n"),
    ]);
    let workspace = open(&fixture);

    let mut hits = search(&workspace, "saffron buns");
    hits.sort_by(|left, right| left.title.cmp(&right.title));
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].title, "Buns");
    assert_eq!(hits[0].object_type, None);
    assert_eq!(hits[0].folder, "");
    assert_eq!(hits[0].relative_path, "Buns.md");
    assert_eq!(hits[1].title, "Kickoff");
    assert_eq!(hits[1].object_type.as_deref(), Some("task"));
    assert_eq!(hits[1].folder, "Projects/Launch");
    assert_eq!(hits[1].relative_path, "Projects/Launch/Kickoff.md");
    assert!(
        hits[1].snippet.contains("Order saffron buns"),
        "{}",
        hits[1].snippet
    );
}

fn search_in(workspace: &Workspace, folder: &str) -> Vec<String> {
    let mut paths: Vec<String> = workspace
        .search(&SearchQuery {
            text: "saffron".to_owned(),
            folder: folder.to_owned(),
            ..SearchQuery::default()
        })
        .unwrap()
        .into_iter()
        .map(|hit| hit.relative_path)
        .collect();
    paths.sort();
    paths
}

#[test]
fn the_type_filter_offers_the_types_the_index_holds() {
    let fixture = Fixture::new(&[
        (
            "Work/Launch.md",
            b"---\nid: task_01jbcdefghjkmnpqrstvwxyz0a\ntype: task\n\
              created: 2026-10-20T10:00:00Z\nupdated: 2026-10-20T10:00:00Z\n---\n\nLaunch day\n",
        ),
        (
            "Launch notes.md",
            b"---\nid: note_01jbcdefghjkmnpqrstvwxyz0b\ntype: note\n\
              created: 2026-10-20T10:00:00Z\nupdated: 2026-10-20T10:00:00Z\n---\n\nLaunch plan\n",
        ),
        ("Launch draft.md", b"Launch ideas\n"),
    ]);
    let workspace = open(&fixture);

    assert_eq!(workspace.search_types().unwrap(), ["note", "task"]);
    let tasks: Vec<String> = workspace
        .search(&SearchQuery {
            text: "launch".to_owned(),
            object_type: Some("task".to_owned()),
            ..SearchQuery::default()
        })
        .unwrap()
        .into_iter()
        .map(|hit| hit.title)
        .collect();
    assert_eq!(tasks, ["Launch"]);
}

#[test]
fn the_folder_filter_returns_the_folder_and_its_subfolders_as_written() {
    let saffron = b"saffron\n".as_slice();
    let fixture = Fixture::new(&[
        ("Notes/a.md", saffron),
        ("Notes/Sub/b.md", saffron),
        ("Notes old/c.md", saffron),
        ("Notesy/d.md", saffron),
        ("under_score/e.md", saffron),
        ("underXscore/f.md", saffron),
        ("100%/g.md", saffron),
        ("100 and more/h.md", saffron),
        ("Top.md", saffron),
    ]);
    let workspace = open(&fixture);

    assert_eq!(
        search_in(&workspace, "Notes"),
        ["Notes/Sub/b.md", "Notes/a.md"]
    );
    assert_eq!(
        search_in(&workspace, "Notes/"),
        ["Notes/Sub/b.md", "Notes/a.md"]
    );
    assert_eq!(search_in(&workspace, "Notes/Sub"), ["Notes/Sub/b.md"]);
    assert_eq!(search_in(&workspace, "Notes old"), ["Notes old/c.md"]);
    // An underscore and a percent sign match only themselves.
    assert_eq!(search_in(&workspace, "under_score"), ["under_score/e.md"]);
    assert_eq!(search_in(&workspace, "100%"), ["100%/g.md"]);
    assert!(search_in(&workspace, "Missing").is_empty());
    assert_eq!(search_in(&workspace, "").len(), 9);
}

#[test]
fn a_folder_outside_the_workspace_is_refused() {
    let fixture = Fixture::new(&[("Notes/a.md", b"saffron\n")]);
    let workspace = open(&fixture);
    let absolute = fixture.root.join("Notes");

    for folder in [
        "/etc",
        absolute.to_str().unwrap(),
        "..",
        "../Garden/Notes",
        "Notes/../..",
        "Notes/../../Garden/Notes",
    ] {
        let error = workspace
            .search(&SearchQuery {
                text: "saffron".to_owned(),
                folder: folder.to_owned(),
                ..SearchQuery::default()
            })
            .unwrap_err();
        assert!(
            ["absolute_path", "unsafe_path"].contains(&error.code.as_str()),
            "{folder}: {error:?}"
        );
        assert!(!error.message.is_empty());
    }
}
