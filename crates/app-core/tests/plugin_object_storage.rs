//! Seam 1: the AGENTS.md storage matrix for objects of a plugin's own
//! types, written by the recipes test plugin through the real host:
//! malformed input, round trips that keep unknown properties, external
//! edits, stale indexes, interrupted index updates, moves, duplicate IDs,
//! path traversal and symlink escapes, and rebuilds. Every check reads the
//! disk as soon as the call returns.

mod support;

use app_core::App;
use support::{
    Fixture, index_files,
    recipes::{A, B, call, listed, object, recipe_file, revision, with_recipes},
};

/// Stop the index from changing its file list, through triggers a second
/// connection adds. Dropping them lets writes through again.
fn refuse_index_writes(fixture: &Fixture, app: &App) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(&index_files(fixture, app)[0]).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_insert BEFORE INSERT ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;
             CREATE TRIGGER refuse_update BEFORE UPDATE ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;
             CREATE TRIGGER refuse_delete BEFORE DELETE ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;",
        )
        .unwrap();
    connection
}

#[test]
fn malformed_recipe_files_stay_as_they_are_and_no_write_reaches_them() {
    let malformed = format!("---\nid: recipe_{A}\ntype: recipe\ncourse: [\n---\nBody\n");
    let mismatched = format!("---\nid: note_{B}\ntype: recipe\n---\nBody\n");
    let fixture = Fixture::new(&[
        ("recipes/Broken.md", malformed.as_bytes()),
        ("recipes/Mismatched.md", mismatched.as_bytes()),
    ]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let id = format!("recipe_{A}");
    // A plugin can't read a revision of a file that holds no object.
    let rev = "any-revision";

    assert!(listed(&mut app).is_empty());
    assert_eq!(
        call(&mut app, &format!("get|{id}")),
        [format!("error\tnot-found\t{id}")]
    );
    assert_eq!(
        call(&mut app, &format!(r#"update|{id}|{rev}|{{"servings":3}}"#)),
        [format!("error\tnot-found\t{id}")]
    );
    assert_eq!(
        call(&mut app, &format!("delete|{id}|{rev}")),
        [format!("error\tnot-found\t{id}")]
    );
    assert_eq!(fixture.read("recipes/Broken.md"), malformed.as_bytes());
    assert_eq!(fixture.read("recipes/Mismatched.md"), mismatched.as_bytes());
}

// A macOS or Linux file name may hold a control character, which a
// plugin's path can't carry. Windows file names can't hold one.
#[cfg(unix)]
#[test]
fn a_recipe_whose_file_name_a_plugin_path_cant_carry_is_left_out_of_the_list() {
    let soup = recipe_file(A, "servings: 2\n", "Simmer.\n");
    let belled = recipe_file(B, "servings: 4\n", "Ring.\n");
    let fixture = Fixture::new(&[
        ("recipes/Soup.md", &soup),
        ("recipes/Bell\u{7}.md", &belled),
    ]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };

    let listed = listed(&mut app);

    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].id, format!("recipe_{A}"));
    assert_eq!(
        call(&mut app, &format!("get|recipe_{B}")),
        ["error\tunsupported-path\t-"]
    );
    assert_eq!(fixture.read("recipes/Bell\u{7}.md"), belled);
}

#[test]
fn round_trips_keep_unknown_properties_comments_and_line_endings() {
    let bytes = format!(
        "---\r\n# from another app\r\nid: recipe_{A}\r\ntype: recipe\r\nzeta: kept\r\nservings: 2\r\n---\r\nSimmer.\r\nServe.\r\n"
    );
    let fixture = Fixture::new(&[("recipes/Soup.md", bytes.as_bytes())]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let id = format!("recipe_{A}");

    // Moving there and back keeps every byte.
    let there = object(
        &mut app,
        &format!("move|{id}|{}|Soups/Soup.md", revision(bytes.as_bytes())),
    );
    let back = object(
        &mut app,
        &format!("move|{id}|{}|recipes/Soup.md", there.revision),
    );
    assert_eq!(fixture.read("recipes/Soup.md"), bytes.as_bytes());
    assert_eq!(back.id, id);

    // A property change rewrites the frontmatter in the schema's order,
    // with the schema's defaults, and keeps the unknown property, the
    // body's bytes and the line endings.
    let updated = object(
        &mut app,
        &format!(r#"update|{id}|{}|{{"servings":4}}"#, back.revision),
    );
    let written = String::from_utf8(fixture.read("recipes/Soup.md")).unwrap();
    assert!(
        written.starts_with(&format!(
            "---\r\nid: {id}\r\ntype: recipe\r\ncourse: main\r\nservings: 4\r\n\
             vegetarian: false\r\nzeta: kept\r\nupdated: "
        )),
        "{written:?}"
    );
    assert!(
        written.ends_with("---\r\nSimmer.\r\nServe.\r\n"),
        "{written:?}"
    );
    assert_eq!(updated.properties["zeta"], "kept");
    assert_eq!(updated.revision, revision(written.as_bytes()));

    // A property removed through the plugin goes, the rest stays.
    let removed = object(&mut app, &format!("remove|{id}|{}|zeta", updated.revision));
    assert_eq!(removed.properties.get("zeta"), None);
    assert_eq!(removed.properties["servings"], 4);
}

#[test]
fn an_outside_edit_wins_over_the_index_and_stops_writes_with_the_old_revision() {
    let bytes = recipe_file(A, "servings: 2\n", "Simmer.\n");
    let fixture = Fixture::new(&[("recipes/Soup.md", &bytes)]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let id = format!("recipe_{A}");
    let edited = recipe_file(A, "servings: 7\n", "Boil.\n");

    // The write lands before the watcher tells anyone, so the index still
    // holds the old version.
    std::fs::write(fixture.root.join("recipes/Soup.md"), &edited).unwrap();
    let got = object(&mut app, &format!("get|{id}"));

    assert_eq!(got.revision, revision(&edited));
    assert_eq!(got.properties["servings"], 7);
    assert_eq!(
        call(
            &mut app,
            &format!(r#"update|{id}|{}|{{"servings":3}}"#, revision(&bytes))
        ),
        [format!("error\trevision-conflict\t{id}")]
    );
    assert_eq!(fixture.read("recipes/Soup.md"), edited);
}

#[test]
fn writes_by_id_follow_a_file_another_app_moved_before_the_index_knew() {
    let bytes = recipe_file(A, "servings: 2\n", "Simmer.\n");
    let fixture = Fixture::new(&[("recipes/Soup.md", &bytes)]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let id = format!("recipe_{A}");
    std::fs::create_dir_all(fixture.root.join("Archive")).unwrap();
    std::fs::rename(
        fixture.root.join("recipes/Soup.md"),
        fixture.root.join("Archive/Soup.md"),
    )
    .unwrap();

    let updated = object(
        &mut app,
        &format!(r#"update|{id}|{}|{{"servings":3}}"#, revision(&bytes)),
    );

    assert_eq!(updated.path, "Archive/Soup.md");
    assert!(!fixture.exists("recipes/Soup.md"));
    assert_eq!(revision(&fixture.read("Archive/Soup.md")), updated.revision);
    let moved = object(
        &mut app,
        &format!("move|{id}|{}|recipes/Soup.md", updated.revision),
    );
    assert_eq!(moved.id, id);
    assert_eq!(object_id_at(&fixture, "recipes/Soup.md"), id);
}

fn object_id_at(fixture: &Fixture, relative: &str) -> String {
    support::object_id(&fixture.read(relative))
}

#[test]
fn a_create_whose_index_update_fails_is_on_disk_and_the_index_catches_up() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let connection = refuse_index_writes(&fixture, &app);

    let created = object(&mut app, r#"create|recipe|Soup|-|{"servings":3}|Body"#);

    assert_eq!(revision(&fixture.read("recipes/Soup.md")), created.revision);
    connection
        .execute_batch(
            "DROP TRIGGER refuse_insert; DROP TRIGGER refuse_update; DROP TRIGGER refuse_delete;",
        )
        .unwrap();
    let got = object(&mut app, &format!("get|{}", created.id));
    assert_eq!(got.revision, created.revision);
    let all = listed(&mut app);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, created.id);
}

#[test]
fn moves_keep_the_id_and_the_bytes() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let created = object(&mut app, "create|recipe|Soup|-|{}|Body");
    let bytes = fixture.read("recipes/Soup.md");

    let moved = object(
        &mut app,
        &format!(
            "move|{}|{}|Winter/Hot soup.md",
            created.id, created.revision
        ),
    );

    assert_eq!(moved.id, created.id);
    assert_eq!(moved.path, "Winter/Hot soup.md");
    assert_eq!(fixture.read("Winter/Hot soup.md"), bytes);
    assert_eq!(
        object(&mut app, &format!("get|{}", created.id)).path,
        "Winter/Hot soup.md"
    );
    // A taken path refuses the move.
    let other = object(&mut app, "create|recipe|Stew|-|{}|Body");
    assert_eq!(
        call(
            &mut app,
            &format!("move|{}|{}|Winter/Hot soup.md", other.id, other.revision)
        ),
        ["error\talready-exists\tWinter/Hot soup.md"]
    );
}

#[test]
fn files_that_share_an_id_stop_writes_by_id_with_the_identity_conflict_error() {
    let shared = recipe_file(A, "servings: 2\n", "Same ID\n");
    let fixture = Fixture::new(&[("recipes/One.md", &shared), ("recipes/Two.md", &shared)]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let id = format!("recipe_{A}");
    let rev = revision(&shared);
    let conflict = [format!("error\tidentity-conflict\t{id}")];

    for attempt in [
        format!("get|{id}"),
        format!(r#"update|{id}|{rev}|{{"servings":3}}"#),
        format!("move|{id}|{rev}|recipes/Three.md"),
        format!("delete|{id}|{rev}"),
    ] {
        assert_eq!(call(&mut app, &attempt), conflict, "{attempt}");
    }
    assert_eq!(fixture.read("recipes/One.md"), shared);
    assert_eq!(fixture.read("recipes/Two.md"), shared);
    assert!(!fixture.exists("recipes/Three.md"));
}

#[test]
fn creates_and_moves_refuse_traversal_reserved_folders_and_links() {
    let fixture = Fixture::new(&[]);
    let outside = fixture.outside();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, fixture.root.join("Linked")).unwrap();
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let soup = object(&mut app, "create|recipe|Soup|-|{}|Body");
    let before = fixture.snapshot();

    let mut refused = vec![
        "../Escaped.md",
        "/tmp/Escaped.md",
        "recipes/../../Escaped.md",
        ".noura/Hidden.md",
        ".NOURA/Hidden.md",
        ".Git/Hidden.md",
        "recipes/Soup.txt",
    ];
    if cfg!(unix) {
        refused.extend(["Linked/Escaped.md", "Linked"]);
    }
    for path in refused {
        assert_eq!(
            call(&mut app, &format!("create|recipe|Escaped|{path}|{{}}|Body")),
            [format!("error\tinvalid-path\t{path}")],
            "create {path}"
        );
        assert_eq!(
            call(
                &mut app,
                &format!("move|{}|{}|{path}", soup.id, soup.revision)
            ),
            [format!("error\tinvalid-path\t{path}")],
            "move {path}"
        );
    }

    assert_eq!(fixture.snapshot(), before);
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

#[test]
fn deleting_every_index_file_rebuilds_the_same_objects_from_the_files() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let soup = object(&mut app, r#"create|recipe|Soup|-|{"servings":3}|Body"#);
    object(&mut app, "create|recipe|Stew|-|{}|Body");
    object(
        &mut app,
        &format!("move|{}|{}|Winter/Soup.md", soup.id, soup.revision),
    );
    let before: Vec<(String, String, String, serde_json::Value)> = listed(&mut app)
        .into_iter()
        .map(|answer| (answer.id, answer.path, answer.revision, answer.properties))
        .collect();
    let index = index_files(&fixture, &app);
    drop(app);

    for file in &index {
        support::remove_when_free(file);
    }
    let mut app = fixture.app();
    // The plugin runs again in the reopened workspace.
    support::plugins::wait_until(&mut app, "the recipes plugin runs", |app| {
        app.plugins()
            .iter()
            .any(|row| row.id == "recipes" && row.running)
    });

    let after: Vec<(String, String, String, serde_json::Value)> = listed(&mut app)
        .into_iter()
        .map(|answer| (answer.id, answer.path, answer.revision, answer.properties))
        .collect();
    assert_eq!(after, before);
    assert_eq!(after.len(), 2);
    assert!(index[0].exists());
}
