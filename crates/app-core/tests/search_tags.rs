//! Seam 1: the search tag filter, and the AGENTS.md storage matrix for
//! the tag index behind it: malformed tags, round trips, outside edits,
//! stale indexes, interrupted index updates, moves, duplicate IDs, path
//! traversal in the folder filter, and rebuilds.

mod support;

use std::path::PathBuf;

use app_core::{App, SearchQuery, Workspace, WorkspaceService};
use support::Fixture;

fn open(fixture: &Fixture) -> Workspace {
    WorkspaceService::new(fixture.app_data.path())
        .open_folder(&fixture.root)
        .unwrap()
}

/// The paths of the notes tagged `tag` that hold "saffron", sorted.
fn tagged(workspace: &Workspace, tag: &str) -> Vec<String> {
    let mut paths: Vec<String> = workspace
        .search(&SearchQuery {
            text: "saffron".to_owned(),
            tag: tag.to_owned(),
            ..SearchQuery::default()
        })
        .unwrap()
        .into_iter()
        .map(|hit| hit.relative_path)
        .collect();
    paths.sort();
    paths
}

/// A managed note with `id_suffix`, these frontmatter lines and body.
fn managed(id_suffix: &str, frontmatter: &str, body: &str) -> Vec<u8> {
    format!(
        "---\nid: note_{id_suffix}\ntype: note\n{frontmatter}created: 2026-10-20T10:00:00Z\n\
         updated: 2026-10-20T10:00:00Z\n---\n\n{body}"
    )
    .into_bytes()
}

const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";

#[test]
fn the_tag_filter_reads_a_list_or_a_single_value() {
    let fixture = Fixture::new(&[
        (
            "List.md",
            b"---\ntags:\n  - recipe\n  - dinner\n---\nsaffron\n",
        ),
        ("Single.md", b"---\ntags: recipe\n---\nsaffron\n"),
        ("Managed.md", &managed(A, "tags: [recipe]\n", "saffron\n")),
        ("Other.md", b"---\ntags: [dinner]\n---\nsaffron\n"),
        ("Untagged.md", b"saffron\n"),
        ("Mention.md", b"saffron recipe\n"),
        ("Longer.md", b"---\ntags: [recipes]\n---\nsaffron\n"),
    ]);
    let workspace = open(&fixture);

    let recipes = ["List.md", "Managed.md", "Single.md"];
    assert_eq!(tagged(&workspace, "recipe"), recipes);
    // As Obsidian reads tags, a leading # and the case don't matter.
    assert_eq!(tagged(&workspace, "#recipe"), recipes);
    assert_eq!(tagged(&workspace, " Recipe "), recipes);
    assert_eq!(tagged(&workspace, "dinner"), ["List.md", "Other.md"]);
    assert!(tagged(&workspace, "missing").is_empty());
    // No tag filter finds every note.
    assert_eq!(tagged(&workspace, "").len(), 7);
}

/// What the open app's search finds for "saffron" with `tag`.
fn app_tagged(app: &App, tag: &str) -> Vec<String> {
    tagged(app.workspace().unwrap(), tag)
}

/// The workspace's index files in the app data folder.
fn index_files(fixture: &Fixture, workspace_id: &str) -> Vec<PathBuf> {
    let folder = fixture
        .app_data
        .path()
        .join("workspaces")
        .join(workspace_id);
    ["index.sqlite", "index.sqlite-wal", "index.sqlite-shm"]
        .into_iter()
        .map(|name| folder.join(name))
        .collect()
}

fn remove_present(files: &[PathBuf]) {
    for file in files {
        if file.exists() {
            std::fs::remove_file(file).unwrap();
        }
    }
}

#[test]
fn malformed_tags_hold_no_tag_and_never_fail_a_search() {
    let fixture = Fixture::new(&[
        ("Number.md", b"---\ntags: 42\n---\nsaffron\n"),
        (
            "Mixed.md",
            b"---\ntags: [ok, 3, null, {a: b}, [nested], '', '  ', '#']\n---\nsaffron\n",
        ),
        ("Map.md", b"---\ntags: {ok: yes}\n---\nsaffron\n"),
        ("Empty.md", b"---\ntags:\n---\nsaffron\n"),
        ("Broken.md", b"---\ntags: [ok\n---\nsaffron\n"),
        ("Unclosed.md", b"---\ntags: [ok]\nsaffron\n"),
        ("Bytes.md", b"---\ntags: [ok]\n---\nsaffron \xff\xfe\n"),
    ]);
    let workspace = open(&fixture);

    assert_eq!(tagged(&workspace, "ok"), ["Mixed.md"]);
    for tag in ["42", "3", "null", "a", "nested", "yes"] {
        assert!(tagged(&workspace, tag).is_empty(), "{tag}");
    }
    // Broken frontmatter stays searchable as text.
    let all = tagged(&workspace, "");
    assert!(all.contains(&"Broken.md".to_owned()), "{all:?}");
    assert!(all.contains(&"Unclosed.md".to_owned()), "{all:?}");
    // A # alone names no tag yet, so it narrows nothing.
    assert_eq!(tagged(&workspace, "#"), all);
}

#[test]
fn a_tagged_note_renamed_and_moved_there_and_back_keeps_its_tag_and_bytes() {
    let bytes = managed(A, "tags: [recipe]\n", "saffron\n");
    let fixture = Fixture::new(&[("Paella.md", &bytes), ("Archive/Old.md", b"old")]);
    let mut app = fixture.app();
    let before = fixture.snapshot();

    app.start_rename("Paella.md");
    assert!(app.commit_rename("Risotto"));
    assert_eq!(app_tagged(&app, "recipe"), ["Risotto.md"]);
    app.move_into("Risotto.md", "Archive");
    assert_eq!(app_tagged(&app, "recipe"), ["Archive/Risotto.md"]);
    let in_archive = app
        .workspace()
        .unwrap()
        .search(&SearchQuery {
            text: "saffron".to_owned(),
            folder: "Archive".to_owned(),
            tag: "recipe".to_owned(),
            ..SearchQuery::default()
        })
        .unwrap();
    assert_eq!(in_archive.len(), 1);
    app.move_into("Archive/Risotto.md", "");
    app.start_rename("Risotto.md");
    assert!(app.commit_rename("Paella"));

    assert_eq!(app_tagged(&app, "recipe"), ["Paella.md"]);
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn tags_follow_edits_made_outside_noura() {
    let fixture = Fixture::new(&[("Paella.md", b"---\ntags: [recipe]\n---\nsaffron\n")]);
    let mut app = fixture.app();
    assert_eq!(app_tagged(&app, "recipe"), ["Paella.md"]);

    support::write(
        &fixture.root,
        "Paella.md",
        b"---\ntags: [dinner]\n---\nsaffron\n",
    );
    support::write(
        &fixture.root,
        "Risotto.md",
        b"---\ntags: recipe\n---\nsaffron\n",
    );
    // The watcher (ticket 10) reads the folder again after outside changes.
    app.rescan();

    assert_eq!(app_tagged(&app, "recipe"), ["Risotto.md"]);
    assert_eq!(app_tagged(&app, "dinner"), ["Paella.md"]);
}

#[test]
fn tags_changed_while_noura_was_closed_show_at_the_next_open() {
    let fixture = Fixture::new(&[("Paella.md", b"---\ntags: [recipe]\n---\nsaffron\n")]);
    let app = fixture.app();
    assert_eq!(app_tagged(&app, "recipe"), ["Paella.md"]);
    drop(app);

    // The index still says recipe.
    support::write(
        &fixture.root,
        "Paella.md",
        b"---\ntags: dinner\n---\nsaffron\n",
    );
    let app = fixture.app();

    assert!(app_tagged(&app, "recipe").is_empty());
    assert_eq!(app_tagged(&app, "dinner"), ["Paella.md"]);
}

#[test]
fn an_index_from_before_tags_reads_every_file_again() {
    let fixture = Fixture::new(&[
        ("Paella.md", b"---\ntags: [recipe]\n---\nsaffron\n"),
        ("Ideas.md", b"---\nstatus: brainstorming\n---\nsaffron\n"),
    ]);
    let app = fixture.app();
    let id = app.workspace().unwrap().id().to_owned();
    drop(app);
    // An older Noura indexed these files: it kept no tags and no version 2.
    let index = rusqlite::Connection::open(&index_files(&fixture, &id)[0]).unwrap();
    index
        .execute_batch(
            "DELETE FROM object_tags; DELETE FROM schema_migrations WHERE version=2; \
             UPDATE object_fts SET metadata='';",
        )
        .unwrap();
    drop(index);

    let app = fixture.app();
    assert_eq!(app_tagged(&app, "recipe"), ["Paella.md"]);
    let workspace = app.workspace().unwrap();
    let found = workspace
        .search(&SearchQuery {
            text: "brainstorming".to_owned(),
            ..SearchQuery::default()
        })
        .unwrap();
    assert_eq!(found.len(), 1);
}

#[test]
fn a_tag_update_lost_after_the_file_change_heals_on_the_next_open() {
    let fixture = Fixture::new(&[("Paella.md", b"---\ntags: [recipe]\n---\nsaffron\n")]);
    let app = fixture.app();
    let id = app.workspace().unwrap().id().to_owned();
    drop(app);
    let index = index_files(&fixture, &id);
    let saved = tempfile::tempdir().unwrap();
    for file in index.iter().filter(|file| file.exists()) {
        std::fs::copy(file, saved.path().join(file.file_name().unwrap())).unwrap();
    }

    // The file changes and the index records it; then the index goes back
    // to how it was before, as if its update never reached the disk.
    let mut app = fixture.app();
    support::write(
        &fixture.root,
        "Paella.md",
        b"---\ntags: [dinner]\n---\nsaffron\n",
    );
    app.rescan();
    assert_eq!(app_tagged(&app, "dinner"), ["Paella.md"]);
    drop(app);
    remove_present(&index);
    for file in &index {
        let saved = saved.path().join(file.file_name().unwrap());
        if saved.exists() {
            std::fs::copy(saved, file).unwrap();
        }
    }

    let app = fixture.app();
    assert!(app_tagged(&app, "recipe").is_empty());
    assert_eq!(app_tagged(&app, "dinner"), ["Paella.md"]);
}

#[test]
fn files_that_share_an_id_keep_their_own_tags_and_a_copy_gets_its_own() {
    let fixture = Fixture::new(&[
        ("One.md", &managed(A, "tags: [first]\n", "saffron\n")),
        ("Two.md", &managed(A, "tags: [second]\n", "saffron\n")),
    ]);
    let mut app = fixture.app();

    assert_eq!(app_tagged(&app, "first"), ["One.md"]);
    assert_eq!(app_tagged(&app, "second"), ["Two.md"]);
    app.duplicate("One.md");
    assert_eq!(app_tagged(&app, "first"), ["One 1.md", "One.md"]);
    app.trash("One.md");
    assert_eq!(app_tagged(&app, "first"), ["One 1.md"]);
    assert_eq!(app_tagged(&app, "second"), ["Two.md"]);
}

#[test]
fn the_folder_filter_never_reaches_outside_the_workspace() {
    let fixture = Fixture::new(&[("Notes/Paella.md", b"---\ntags: [recipe]\n---\nsaffron\n")]);
    let outside = fixture.outside();
    std::fs::write(
        outside.join("Secret.md"),
        b"---\ntags: [recipe]\n---\nsaffron\n",
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, fixture.root.join("Linked")).unwrap();
    let app = fixture.app();
    let workspace = app.workspace().unwrap();
    let before = fixture.snapshot();

    for folder in [
        "..",
        "../Outside",
        "Notes/../../Outside",
        outside.to_str().unwrap(),
    ] {
        let result = workspace.search(&SearchQuery {
            text: "saffron".to_owned(),
            folder: folder.to_owned(),
            tag: "recipe".to_owned(),
            ..SearchQuery::default()
        });
        assert!(result.is_err(), "{folder}: {result:?}");
    }
    let linked = workspace
        .search(&SearchQuery {
            text: "saffron".to_owned(),
            folder: "Linked".to_owned(),
            tag: "recipe".to_owned(),
            ..SearchQuery::default()
        })
        .unwrap();
    assert!(linked.is_empty(), "{linked:?}");
    assert_eq!(app_tagged(&app, "recipe"), ["Notes/Paella.md"]);
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn deleting_the_index_rebuilds_the_same_results() {
    let fixture = Fixture::new(&[
        (
            "Notes/Paella.md",
            &managed(A, "tags: [recipe]\n", "saffron rice\n"),
        ),
        ("Notes/Sub/Risotto.md", b"---\ntags: recipe\n---\nsaffron\n"),
        ("Pantry.md", b"---\ntags: [pantry]\n---\nsaffron jar\n"),
    ]);
    let queries = [
        SearchQuery {
            text: "saffron".to_owned(),
            ..SearchQuery::default()
        },
        SearchQuery {
            text: "saff".to_owned(),
            tag: "recipe".to_owned(),
            ..SearchQuery::default()
        },
        SearchQuery {
            text: "saffron".to_owned(),
            folder: "Notes".to_owned(),
            object_type: Some("note".to_owned()),
            ..SearchQuery::default()
        },
    ];
    let mut app = fixture.app();
    app.duplicate("Pantry.md");
    let id = app.workspace().unwrap().id().to_owned();
    let results = |app: &App| -> Vec<_> {
        let workspace = app.workspace().unwrap();
        queries
            .iter()
            .map(|query| workspace.search(query).unwrap())
            .collect()
    };
    let before = results(&app);
    let types = app.workspace().unwrap().search_types().unwrap();
    drop(app);

    let index = index_files(&fixture, &id);
    remove_present(&index);
    let app = fixture.app();

    assert_eq!(results(&app), before);
    assert_eq!(app.workspace().unwrap().search_types().unwrap(), types);
    assert!(index[0].exists());
}
