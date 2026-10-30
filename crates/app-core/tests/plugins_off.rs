//! Seam 1 against a real workspace folder: with every plugin off, the
//! kernel knows no task, project or calendar fields. A workspace of tasks,
//! projects and dated notes opens, indexes and searches, and each file
//! opens in the note editor as a generic managed object, its properties as
//! they are written, beside the kernel's own panel. Opening and closing
//! every file leaves its bytes as they were.

mod support;

use std::collections::BTreeMap;

use app_core::{SearchQuery, Workspace};
use serde_json::Value;
use support::{Fixture, open_from_tree, settle, settle_saves};

/// The files of the shared queries fixture, which holds tasks, a project
/// and dated notes, with a few that another app wrote.
fn files() -> Vec<(String, Vec<u8>)> {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../docs/workspace-format/fixtures/queries-v1.json"
    ))
    .unwrap();
    let mut files: Vec<(String, Vec<u8>)> = fixture["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap().to_owned(),
                file["content"].as_str().unwrap().as_bytes().to_vec(),
            )
        })
        .collect();
    files.extend([
        (
            // A value the Tasks plugin wouldn't accept, a byte-order mark,
            // CRLF line endings and no final line break.
            "Tasks/Weed.md".to_owned(),
            "\u{feff}---\r\nid: task_01k00000000000000000000004\r\ntype: task\r\n\
             status: someday\r\npriority: 2\r\nestimate: 3\r\n---\r\nPull the weeds"
                .as_bytes()
                .to_vec(),
        ),
        (
            // Hand-written frontmatter, with a comment and odd spacing.
            "Projects/Garden/Garden.md".to_owned(),
            b"---\nid: project_01k00000000000000000000011\ntype: project\n\
              status:   active   # in season\nowner: Ada\n---\nBeds and paths.\n"
                .to_vec(),
        ),
        (
            "Notes/Standup.md".to_owned(),
            b"---\nid: note_01k00000000000000000000021\ntype: note\n\
              start: 2026-09-04T09:00:00+02:00\nend: 2026-09-04T09:15:00+02:00\n\
              date: 2026-09-04\n---\nWho does what.\n"
                .to_vec(),
        ),
    ]);
    files
}

fn titles(workspace: &Workspace, text: &str, object_type: Option<&str>) -> Vec<String> {
    let mut titles: Vec<String> = workspace
        .search(&SearchQuery {
            text: text.to_owned(),
            object_type: object_type.map(str::to_owned),
            ..SearchQuery::default()
        })
        .unwrap()
        .into_iter()
        .map(|hit| hit.title)
        .collect();
    titles.sort();
    titles
}

#[test]
fn tasks_projects_and_dated_notes_open_index_and_search_as_generic_objects() {
    let files = files();
    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    let fixture = Fixture::new(&borrowed);
    let mut app = fixture.app();
    let before = fixture.snapshot();
    assert!(
        app.plugins().iter().all(|plugin| !plugin.running),
        "a plugin runs"
    );

    // The index holds every object by its type, and search finds them.
    let workspace = app.workspace().unwrap();
    assert_eq!(
        workspace.search_types(),
        Ok(vec!["note".into(), "project".into(), "task".into()])
    );
    assert_eq!(titles(workspace, "release", Some("task")), ["call", "ship"]);
    assert_eq!(titles(workspace, "someday", None), ["Weed"]);
    assert_eq!(titles(workspace, "season", None), Vec::<String>::new());
    assert_eq!(titles(workspace, "beds", Some("project")), ["Garden"]);
    assert_eq!(titles(workspace, "who", Some("note")), ["Standup"]);

    // Each managed file opens in the note editor with its properties as
    // written, and no plugin draws its panel.
    for (path, bytes) in &files {
        let local_core::ParsedMarkdown::Managed(object) = local_core::parse_markdown(path, bytes)
        else {
            continue;
        };
        if path.starts_with('.') || path == "Notes/dup-b.md" {
            // Hidden files stay out of the tree, and dup-a opens the ID
            // both copies share.
            continue;
        }
        let tab = open_from_tree(&mut app, path);
        settle(&mut app);
        let note = app.note(tab).unwrap();
        assert_eq!(
            note.object_type(),
            Some(object.object_type.as_str()),
            "{path}"
        );
        assert!(!note.shows_whole_file(), "{path}");
        let shown: BTreeMap<String, ()> = note
            .properties()
            .unwrap()
            .into_iter()
            .map(|property| (property.key, ()))
            .collect();
        for key in object.properties.keys() {
            assert!(shown.contains_key(key), "{path} hides {key}");
        }
        assert!(app.plugin_panel(tab).is_none(), "{path}");
        app.close_tab(tab);
    }
    settle_saves(&mut app);
    drop(app);

    assert_eq!(fixture.snapshot(), before, "a file changed");
}
