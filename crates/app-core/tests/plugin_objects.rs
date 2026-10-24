//! Seam 1: objects of a plugin's own types against a real workspace
//! folder. The recipes test plugin runs in the real host, registers the
//! `recipe` type and writes recipes through the plugins service: each
//! write passes the schema, carries the revision the plugin read and is on
//! disk when the call returns. Recipes open in the note editor with the
//! plugin's properties panel beside the body.

mod support;

use std::time::Instant;

use app_core::{App, Change, SearchQuery, TabId, ViewKind, ViewNode};
use support::{
    FakeSystem, Fixture, index_files, note, open_from_tree,
    plugins::{self, PluginFolder},
    recipes::{A, Answer, B, REFUSED, call, object, parse, recipe_file, revision, with_recipes},
    settle_saves,
};

#[test]
fn creating_writes_a_managed_file_whose_id_carries_the_types_prefix_and_passes_the_schema() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };

    let created = object(
        &mut app,
        r#"create|recipe|Tomato soup|-|{"servings":4,"spice":"cumin"}|Simmer.
"#,
    );

    assert!(created.id.starts_with("recipe_"), "{}", created.id);
    assert_eq!(created.path, "recipes/Tomato soup.md");
    let bytes = fixture.read("recipes/Tomato soup.md");
    assert_eq!(created.revision, revision(&bytes));
    let text = String::from_utf8(bytes).unwrap();
    let start = format!(
        "---\nid: {}\ntype: recipe\ncourse: main\nservings: 4\nvegetarian: false\nspice: cumin\n\
         created: ",
        created.id
    );
    assert!(text.starts_with(&start), "{text}");
    assert!(text.ends_with("---\nSimmer.\n"), "{text}");
    assert_eq!(created.problem, "-");
}

#[test]
fn a_create_with_a_path_needs_no_title_and_one_without_a_path_does() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };

    let stew = object(&mut app, "create|recipe||Mains/Stew.md|{}|Braise.");
    let refused = call(&mut app, "create|recipe||-|{}|Braise.");

    assert_eq!(stew.path, "Mains/Stew.md");
    assert_eq!(revision(&fixture.read("Mains/Stew.md")), stew.revision);
    assert_eq!(refused, ["error\tfailed\tA title is required."]);
    assert!(!fixture.exists("recipes"));
}

#[test]
fn properties_that_fail_the_schema_name_the_field_and_write_nothing() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let before = fixture.snapshot();

    for (properties, refusal) in [
        (r#"{"course":"brunch"}"#, "course:not_allowed"),
        (r#"{"servings":"many"}"#, "servings:wrong_type"),
        (r#"{"cooked":"yesterday"}"#, "cooked:invalid_date"),
    ] {
        assert_eq!(
            call(&mut app, &format!("create|recipe|Soup|-|{properties}|Body")),
            [format!("error\tinvalid-property\t{refusal}")]
        );
    }
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(call(&mut app, "list|recipe"), Vec::<String>::new());
}

#[test]
fn a_plugin_cant_write_notes_or_another_plugins_objects_and_their_files_keep_their_bytes() {
    let note_bytes = note(A, "Plan the garden.\n");
    let fixture = Fixture::new(&[("Plan.md", &note_bytes)]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let soup = object(&mut app, "create|recipe|Soup|-|{}|Body");
    let soup_bytes = fixture.read("recipes/Soup.md");
    // A second copy of the plugin can't take the recipe type the first
    // one holds, so recipes aren't its objects.
    let Some(second) = PluginFolder::recipes("recipes-two") else {
        return;
    };
    plugins::install(&mut app, second.path());
    assert!(app.set_plugin_enabled("recipes-two", true));
    support::settle(&mut app);
    let row = app
        .plugins()
        .into_iter()
        .find(|row| row.id == "recipes-two")
        .unwrap();
    let problem = row.problem.unwrap_or_default();
    assert!(
        problem.contains("recipes already holds the type recipe"),
        "{problem}"
    );
    let note_id = format!("note_{A}");
    let note_revision = revision(&note_bytes);
    let refused = vec![REFUSED.to_owned()];

    for attempt in [
        format!(r#"update|{note_id}|{note_revision}|{{"color":"red"}}"#),
        format!("move|{note_id}|{note_revision}|Moved.md"),
        format!("delete|{note_id}|{note_revision}"),
        "create|note|Plan 2|-|{}|Body".to_owned(),
    ] {
        assert_eq!(call(&mut app, &attempt), refused, "{attempt}");
    }
    let second_page = "plugin:recipes-two:recipes";
    for attempt in [
        format!(r#"update|{}|{}|{{"servings":9}}"#, soup.id, soup.revision),
        format!("move|{}|{}|recipes/Stew.md", soup.id, soup.revision),
        format!("delete|{}|{}", soup.id, soup.revision),
        "create|recipe|Stew|-|{}|Body".to_owned(),
    ] {
        app.press_plugin_button(second_page, &attempt);
        support::settle(&mut app);
        assert_eq!(
            plugins::texts(&app, second_page).unwrap(),
            refused,
            "{attempt}"
        );
    }
    assert_eq!(fixture.read("Plan.md"), note_bytes);
    assert_eq!(fixture.read("recipes/Soup.md"), soup_bytes);
    assert!(!fixture.exists("Moved.md"));
    assert!(!fixture.exists("recipes/Stew.md"));
}

#[test]
fn writes_carry_the_revision_the_plugin_read_and_an_outside_edit_stops_them() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let soup = object(&mut app, "create|recipe|Soup|-|{}|Simmer.\n");
    let edited = String::from_utf8(fixture.read("recipes/Soup.md"))
        .unwrap()
        .replace("Simmer.", "Boil.");
    std::fs::write(fixture.root.join("recipes/Soup.md"), &edited).unwrap();
    let conflict = vec![format!("error\trevision-conflict\t{}", soup.id)];

    for attempt in [
        format!(r#"update|{}|{}|{{"servings":3}}"#, soup.id, soup.revision),
        format!("body|{}|{}|Fry.", soup.id, soup.revision),
        format!("move|{}|{}|recipes/Stew.md", soup.id, soup.revision),
        format!("delete|{}|{}", soup.id, soup.revision),
    ] {
        assert_eq!(call(&mut app, &attempt), conflict, "{attempt}");
        assert_eq!(fixture.read("recipes/Soup.md"), edited.as_bytes());
    }
    assert!(!fixture.exists("recipes/Stew.md"));

    // With the revision of the file as it is now, the writes go in.
    let current = object(&mut app, &format!("get|{}", soup.id));
    assert_eq!(current.revision, revision(edited.as_bytes()));
    assert_eq!(current.body, "Boil.\n");
    let updated = object(
        &mut app,
        &format!(
            r#"update|{}|{}|{{"servings":3}}"#,
            soup.id, current.revision
        ),
    );
    assert_eq!(updated.properties["servings"], 3);
    assert_eq!(updated.body, "Boil.\n");
    let moved = object(
        &mut app,
        &format!("move|{}|{}|Soups/Soup.md", soup.id, updated.revision),
    );
    assert_eq!(moved.id, soup.id);
    assert_eq!(moved.path, "Soups/Soup.md");
    assert!(!fixture.exists("recipes/Soup.md"));
    assert_eq!(revision(&fixture.read("Soups/Soup.md")), moved.revision);
}

#[test]
fn delete_moves_the_file_to_the_system_trash_as_kernel_deletes_do() {
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let soup = object(&mut app, "create|recipe|Soup|-|{}|Body");
    let bytes = fixture.read("recipes/Soup.md");

    assert_eq!(
        call(&mut app, &format!("delete|{}|{}", soup.id, soup.revision)),
        [format!("deleted\t{}", soup.id)]
    );

    assert!(!fixture.exists("recipes/Soup.md"));
    assert_eq!(std::fs::read(system.in_bin("Soup.md")).unwrap(), bytes);
    assert_eq!(call(&mut app, "list|recipe"), Vec::<String>::new());
}

#[test]
fn delete_uses_the_workspaces_trash_folder_when_the_system_has_none() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app_with(FakeSystem::without_trash());
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let soup = object(&mut app, "create|recipe|Soup|-|{}|Body");
    let bytes = fixture.read("recipes/Soup.md");

    call(&mut app, &format!("delete|{}|{}", soup.id, soup.revision));

    assert!(!fixture.exists("recipes/Soup.md"));
    let trashed: Vec<String> = plugins::files_under(&fixture.root.join(".noura/trash"))
        .into_iter()
        .filter(|path| path.ends_with("recipes/Soup.md"))
        .collect();
    assert_eq!(trashed.len(), 1, "{trashed:?}");
    assert_eq!(
        std::fs::read(fixture.root.join(".noura/trash").join(&trashed[0])).unwrap(),
        bytes
    );
}

#[cfg(unix)]
#[test]
fn a_write_whose_bytes_cant_become_durable_fails_and_nothing_reports_it() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};

    let before = recipe_file(A, "servings: 2\n", "Simmer.\n");
    let fixture = Fixture::new(&[("recipes/Soup.md", &before)]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let soup = object(&mut app, &format!("get|recipe_{A}"));
    let tab = open_from_tree(&mut app, "recipes/Soup.md");
    let shown = panel(&mut app, tab);
    let changes = Arc::new(Mutex::new(Vec::new()));
    let heard = Arc::clone(&changes);
    app.subscribe(move |change| heard.lock().unwrap().push(change));
    // A read-only folder stops the temporary file each write starts with,
    // so no write can reach the disk.
    let folder = fixture.root.join("recipes");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();

    let answers = [
        call(
            &mut app,
            &format!(
                r#"update|{}|{}|{{"servings":5,"source":"saffron"}}"#,
                soup.id, soup.revision
            ),
        ),
        call(
            &mut app,
            &format!("body|{}|{}|Braise.", soup.id, soup.revision),
        ),
        call(&mut app, "create|recipe|Stew|-|{}|Braise."),
    ];
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    app.poll();

    for answer in &answers {
        assert!(answer[0].starts_with("error\tfailed\t"), "{answer:?}");
    }
    assert_eq!(fixture.read("recipes/Soup.md"), before);
    assert!(!fixture.exists("recipes/Stew.md"));

    // The index keeps the revision and content the file holds, and has no
    // row for Stew.
    let index = rusqlite::Connection::open(&index_files(&fixture, &app)[0]).unwrap();
    let (indexed_revision, body, properties): (String, String, String) = index
        .query_row(
            "SELECT f.hash,o.body,o.frontmatter_json FROM objects o \
             JOIN files f ON f.id=o.file_id WHERE o.stable_id=?1",
            [&soup.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(indexed_revision, soup.revision);
    assert!(!body.contains("Braise"), "{body}");
    assert!(!properties.contains("saffron"), "{properties}");
    let stews: i64 = index
        .query_row(
            "SELECT count(*) FROM files WHERE relative_path LIKE '%Stew%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stews, 0);
    // Search reads the index, and finds none of what the writes held.
    let workspace = app.workspace().unwrap();
    for word in ["saffron", "braise", "stew"] {
        let query = SearchQuery {
            text: word.to_owned(),
            ..SearchQuery::default()
        };
        assert_eq!(workspace.search(&query).unwrap(), [], "{word}");
    }

    // The app reports no change: the open note keeps the revision it read
    // and its text, the plugin's panel shows the same properties, and the
    // tree has no Stew.
    assert!(
        !changes.lock().unwrap().contains(&Change::Note),
        "{changes:?}"
    );
    let session = app.note(tab).unwrap();
    assert_eq!(session.revision(), soup.revision);
    assert_eq!(session.text(), "Simmer.\n");
    assert!(!session.is_unsaved());
    assert_eq!(panel(&mut app, tab), shown);
    assert!(
        !support::rows(&app).iter().any(|row| row.contains("Stew")),
        "{:?}",
        support::rows(&app)
    );
    let listed = call(&mut app, "list|recipe");
    assert_eq!(listed.len(), 1);
    assert_eq!(parse(&listed[0]).unwrap().revision, soup.revision);
}

#[test]
fn an_object_that_fails_its_schema_stays_untouched_and_reads_with_the_failing_field() {
    let broken = recipe_file(A, "course: brunch\nservings: 2\n", "Eggs.\n");
    let fine = recipe_file(B, "course: main\n", "Soup.\n");
    let fixture = Fixture::new(&[("recipes/Brunch.md", &broken), ("recipes/Soup.md", &fine)]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };

    let listed: Vec<Answer> = support::recipes::listed(&mut app);

    assert_eq!(listed.len(), 2, "{listed:?}");
    assert_eq!(listed[0].path, "recipes/Brunch.md");
    assert_eq!(listed[0].problem, "course:not_allowed");
    assert_eq!(listed[0].properties["course"], "brunch");
    assert_eq!(listed[1].problem, "-");
    let got = object(&mut app, &format!("get|recipe_{A}"));
    assert_eq!(got.problem, "course:not_allowed");
    assert_eq!(fixture.read("recipes/Brunch.md"), broken);
    assert_eq!(fixture.read("recipes/Soup.md"), fine);
}

/// The texts in a view, top to bottom.
fn view_texts(node: &ViewNode, texts: &mut Vec<String>) {
    match &node.kind {
        ViewKind::Text(text) => texts.push(text.clone()),
        ViewKind::Button { label } => texts.push(format!("[{label}]")),
        _ => {}
    }
    for child in &node.children {
        view_texts(child, texts);
    }
}

/// What the plugin's properties panel of the note in `tab` shows once the
/// plugin drew it on its thread, or `None` when the kernel's panel shows.
fn panel(app: &mut App, tab: TabId) -> Option<Vec<String>> {
    let _asked = app.plugin_panel(tab)?;
    support::settle(app);
    let view = app.plugin_panel(tab)?.expect("the panel draws");
    let mut texts = Vec::new();
    view_texts(view.root(), &mut texts);
    Some(texts)
}

#[test]
fn a_recipe_opens_in_the_note_editor_with_the_plugins_panel_and_keeps_its_bytes() {
    let bytes = recipe_file(A, "course: dessert\nservings: 6\n", "Layer and chill.\r\n");
    let fixture = Fixture::new(&[("recipes/Tiramisu.md", &bytes)]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let modified = std::fs::metadata(fixture.root.join("recipes/Tiramisu.md"))
        .unwrap()
        .modified()
        .unwrap();

    let tab = open_from_tree(&mut app, "recipes/Tiramisu.md");

    let session = app.note(tab).expect("the recipe opens in the note editor");
    assert_eq!(session.text(), "Layer and chill.\n");
    assert_eq!(session.object_id(), Some(format!("recipe_{A}").as_str()));
    assert_eq!(
        panel(&mut app, tab),
        Some(vec![
            "Course: dessert".to_owned(),
            "Servings: 6".to_owned(),
            "Vegetarian: no".to_owned(),
            "[Mark vegetarian]".to_owned(),
        ])
    );
    app.close_tab(tab);
    settle_saves(&mut app);
    assert_eq!(fixture.read("recipes/Tiramisu.md"), bytes);
    assert_eq!(
        std::fs::metadata(fixture.root.join("recipes/Tiramisu.md"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
}

#[test]
fn a_property_changed_from_the_panel_and_unsaved_typing_both_reach_the_file() {
    let bytes = recipe_file(A, "course: main\n", "Simmer.\n");
    let fixture = Fixture::new(&[("recipes/Soup.md", &bytes)]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let tab = open_from_tree(&mut app, "recipes/Soup.md");
    let text = format!("{}Serve hot.\n", app.note(tab).unwrap().text());
    app.edit_note(tab, 1, &text, Instant::now());
    assert!(app.note(tab).unwrap().is_unsaved());

    app.press_panel_button(tab, "vegetarian");
    support::settle(&mut app);

    assert_eq!(
        panel(&mut app, tab).unwrap()[2..],
        [
            "Vegetarian: yes".to_owned(),
            "[Mark not vegetarian]".to_owned()
        ]
    );
    app.save_note_now(tab);
    settle_saves(&mut app);
    let written = String::from_utf8(fixture.read("recipes/Soup.md")).unwrap();
    assert!(written.contains("\nvegetarian: true\n"), "{written}");
    assert!(written.ends_with("---\nSimmer.\nServe hot.\n"), "{written}");
    assert!(!app.note(tab).unwrap().is_unsaved());
    assert!(app.note(tab).unwrap().save_problem().is_none());
}

#[test]
fn with_the_plugin_off_recipes_stay_and_open_with_the_kernels_panel() {
    let bytes = recipe_file(A, "course: dessert\n", "Layer and chill.\n");
    let fixture = Fixture::new(&[("recipes/Tiramisu.md", &bytes)]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    assert!(app.set_plugin_enabled("recipes", false));

    assert!(support::rows(&app).contains(&"recipes".to_owned()));
    let tab = open_from_tree(&mut app, "recipes/Tiramisu.md");

    assert_eq!(panel(&mut app, tab), None, "the kernel's panel shows");
    let properties = app.note(tab).unwrap().properties().unwrap();
    assert!(
        properties
            .iter()
            .any(|property| property.key == "course" && property.read_only),
        "{properties:?}"
    );
    assert_eq!(app.note(tab).unwrap().text(), "Layer and chill.\n");
    app.close_tab(tab);
    settle_saves(&mut app);
    assert_eq!(fixture.read("recipes/Tiramisu.md"), bytes);
    // The index still holds the recipe.
    assert!(app.set_plugin_enabled("recipes", true));
    support::settle(&mut app);
    let listed = call(&mut app, "list|recipe");
    assert_eq!(listed.len(), 1);
    assert_eq!(parse(&listed[0]).unwrap().id, format!("recipe_{A}"));
}

#[test]
fn revoking_object_types_shows_the_kernels_panel_until_you_allow_it_again() {
    let bytes = recipe_file(A, "course: dessert\nservings: 6\n", "Layer and chill.\n");
    let fixture = Fixture::new(&[("recipes/Tiramisu.md", &bytes)]);
    let mut app = fixture.app();
    let Some(_folder) = with_recipes(&mut app) else {
        return;
    };
    let tab = open_from_tree(&mut app, "recipes/Tiramisu.md");
    assert!(panel(&mut app, tab).is_some());

    let types = app_core::Grant::Capability("object-types".to_owned());
    assert!(app.revoke_plugin_grant("recipes", &types));
    support::settle(&mut app);

    assert_eq!(panel(&mut app, tab), None, "the kernel's panel shows");
    assert_eq!(call(&mut app, "list|recipe"), [REFUSED]);

    assert!(app.allow_plugin_grant("recipes", &types));
    support::settle(&mut app);
    assert_eq!(
        panel(&mut app, tab).unwrap()[..1],
        ["Course: dessert".to_owned()]
    );
    app.close_tab(tab);
    settle_saves(&mut app);
    assert_eq!(fixture.read("recipes/Tiramisu.md"), bytes);
}
