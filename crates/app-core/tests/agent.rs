//! Seam 1: a workspace as an AI tool outside Noura uses it through
//! `AgentWorkspace`, against a real workspace folder and app data: search,
//! reading items by ID and by path, and creating and changing notes under
//! the app's file rules.

mod support;

use std::collections::BTreeMap;

use app_core::{AgentWorkspace, NoteChanges, SearchQuery, WorkspaceService};
use support::{Fixture, note, object_id};

const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";

fn task(suffix: &str) -> Vec<u8> {
    format!(
        "---\nid: task_{suffix}\ntype: task\nstatus: todo\ncreated: 2026-10-01T10:00:00Z\n\
         updated: 2026-10-01T10:00:00Z\n---\nWater the beds.\n"
    )
    .into_bytes()
}

fn agent(fixture: &Fixture) -> AgentWorkspace {
    AgentWorkspace::open(fixture.app_data.path(), &fixture.root).unwrap()
}

fn query(text: &str) -> SearchQuery {
    SearchQuery {
        text: text.to_owned(),
        ..SearchQuery::default()
    }
}

#[test]
fn an_agent_searches_and_reads_items_by_id_and_by_path() {
    let fixture = Fixture::new(&[
        ("Docs/Plan.md", &note(A, "Plant tomatoes in May.\n")),
        ("Loose.md", b"---\ntags: [garden]\n---\nTomatoes too.\n"),
    ]);
    let agent = agent(&fixture);
    let workspace = agent.workspace();

    let hits = workspace.search(&query("tomatoes")).unwrap();
    let mut paths: Vec<&str> = hits.iter().map(|hit| hit.relative_path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["Docs/Plan.md", "Loose.md"]);

    let plan = workspace.read_item(&format!("note_{A}")).unwrap();
    assert_eq!(plan.id.as_deref(), Some(format!("note_{A}").as_str()));
    assert_eq!(plan.object_type.as_deref(), Some("note"));
    assert_eq!(plan.title, "Plan");
    assert_eq!(plan.relative_path, "Docs/Plan.md");
    assert_eq!(plan.properties["color"], "green");
    assert!(
        plan.body.contains("Plant tomatoes in May."),
        "{}",
        plan.body
    );
    assert_eq!(workspace.read_item_at("Docs/Plan.md").unwrap(), plan);

    // A plain Markdown file shows its frontmatter as text and has no ID.
    let loose = workspace.read_item_at("Loose.md").unwrap();
    assert_eq!(loose.id, None);
    assert!(loose.properties.is_empty());
    assert_eq!(loose.body, "---\ntags: [garden]\n---\nTomatoes too.\n");

    assert_eq!(
        workspace
            .read_item("note_01jbcdefghjkmnpqrstvwxyz0z")
            .unwrap_err()
            .code,
        "object_not_found"
    );
}

#[test]
fn a_search_full_of_punctuation_finds_what_the_app_finds() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Tomatoes: \"early\" (May) * 2\n"))]);
    let app = fixture.app();
    let agent = agent(&fixture);
    for text in [
        "\"early\" (May)",
        "* OR NOT ^ : - +",
        "tomatoes:*",
        "\"",
        "()[]{}",
    ] {
        let app_hits = app.workspace().unwrap().search(&query(text));
        let agent_hits = agent.workspace().search(&query(text));
        assert!(agent_hits.is_ok(), "{text}: {agent_hits:?}");
        assert_eq!(agent_hits, app_hits, "{text}");
    }
}

#[test]
fn an_agent_creates_a_named_note_that_is_on_disk_when_the_call_returns() {
    let fixture = Fixture::new(&[("Docs/Plan.md", &note(A, "Body\n"))]);
    let agent = agent(&fixture);

    let (created, synced) = local_core::durability_probe::record(|| {
        agent
            .workspace()
            .create_named_note("Docs", "Ideas", "First idea\n")
    });
    let created = created.unwrap();

    assert_eq!(created.relative_path, "Docs/Ideas.md");
    assert_eq!(created.title, "Ideas");
    assert_eq!(created.object_type.as_deref(), Some("note"));
    assert_eq!(created.body, "First idea\n");
    let bytes = fixture.read("Docs/Ideas.md");
    assert_eq!(Some(object_id(&bytes)), created.id);
    let docs = fixture.root.join("Docs").canonicalize().unwrap();
    assert!(
        synced
            .iter()
            .any(|folder| folder.canonicalize().ok().as_ref() == Some(&docs)),
        "{synced:?}"
    );

    // A taken title and a name the file tree refuses change nothing.
    let before = fixture.snapshot();
    let taken = agent
        .workspace()
        .create_named_note("Docs", "plan", "")
        .unwrap_err();
    assert_eq!(taken.code, "name_taken");
    let refused = agent
        .workspace()
        .create_named_note("", "a/b", "")
        .unwrap_err();
    assert_eq!(refused.code, "invalid_name");
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn paths_that_escape_the_workspace_are_refused_as_the_app_refuses_them() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Body\n"))]);
    let outside = fixture.outside();
    std::fs::write(outside.join("Secret.md"), "Keep out.\n").unwrap();
    let app = fixture.app();
    let agent = agent(&fixture);
    let before = fixture.snapshot();

    for folder in ["../Outside", "Docs/../../Outside"] {
        let from_agent = agent
            .workspace()
            .create_named_note(folder, "Escaped", "")
            .unwrap_err();
        assert_eq!(from_agent.code, "unsafe_path", "{folder}");
        assert_eq!(
            app.workspace().unwrap().create_note(folder).unwrap_err(),
            from_agent,
            "{folder}"
        );
    }
    let absolute = outside.to_str().unwrap().to_owned();
    assert!(
        agent
            .workspace()
            .create_named_note(&absolute, "Escaped", "")
            .is_err()
    );
    for path in ["../Outside/Secret.md", "Docs/../../Outside/Secret.md"] {
        assert_eq!(
            agent.workspace().read_item_at(path).unwrap_err().code,
            "unsafe_path",
            "{path}"
        );
    }
    let absolute_file = outside.join("Secret.md");
    assert_eq!(
        agent
            .workspace()
            .read_item_at(absolute_file.to_str().unwrap())
            .unwrap_err()
            .code,
        "absolute_path"
    );
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(
        std::fs::read_dir(&outside).unwrap().count(),
        1,
        "nothing was written outside"
    );
}

#[test]
fn an_agent_updates_a_note_only_while_it_holds_the_revision_it_read() {
    let fixture = Fixture::new(&[
        ("Docs/Plan.md", &note(A, "Body\n")),
        ("Tasks/Water.md", &task(A)),
    ]);
    let agent = agent(&fixture);
    let workspace = agent.workspace();
    let id = format!("note_{A}");
    let read = workspace.read_item(&id).unwrap();

    let changes = NoteChanges {
        title: Some("Plan B".into()),
        body: Some("New body\n".into()),
        properties: BTreeMap::from([
            ("color".to_owned(), serde_json::Value::Null),
            ("stage".to_owned(), serde_json::json!("draft")),
        ]),
        expected_revision: read.revision.clone(),
    };
    let (updated, synced) =
        local_core::durability_probe::record(|| workspace.update_note(&id, &changes));
    let updated = updated.unwrap();
    assert_eq!(updated.id.as_deref(), Some(id.as_str()));
    assert_eq!(updated.relative_path, "Docs/Plan B.md");
    assert_eq!(updated.body, "New body\n");
    assert_eq!(updated.properties.get("color"), None);
    assert_eq!(updated.properties["stage"], "draft");
    assert!(!fixture.exists("Docs/Plan.md"));
    let bytes = String::from_utf8(fixture.read("Docs/Plan B.md")).unwrap();
    assert!(bytes.contains(&format!("id: {id}")), "{bytes}");
    assert!(bytes.ends_with("New body\n"), "{bytes}");
    assert!(!synced.is_empty());

    // The revision read before the change is stale now.
    let before = fixture.snapshot();
    let stale = workspace
        .update_note(
            &id,
            &NoteChanges {
                body: Some("Lost edit\n".into()),
                expected_revision: read.revision,
                ..NoteChanges::default()
            },
        )
        .unwrap_err();
    assert_eq!(stale.code, "revision_conflict");

    // A task isn't a note.
    let water = workspace.read_item(&format!("task_{A}")).unwrap();
    let not_a_note = workspace
        .update_note(
            &format!("task_{A}"),
            &NoteChanges {
                body: Some("Changed\n".into()),
                expected_revision: water.revision,
                ..NoteChanges::default()
            },
        )
        .unwrap_err();
    assert_eq!(not_a_note.code, "not_a_note");
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn a_folder_without_a_workspace_opens_nothing_and_stays_as_it_is() {
    let fixture = Fixture::new(&[]);
    let plain = fixture.outside();
    std::fs::write(plain.join("Note.md"), "Plain.\n").unwrap();
    let recents = WorkspaceService::new(fixture.app_data.path()).recents();

    let error = AgentWorkspace::open(fixture.app_data.path(), &plain)
        .err()
        .expect("a plain folder isn't a workspace");

    assert_eq!(error.code, "workspace_manifest_missing");
    assert!(!plain.join(".noura").exists());
    assert_eq!(
        WorkspaceService::new(fixture.app_data.path()).recents(),
        recents
    );

    // Opening a workspace leaves the recent list as it was too.
    let other = Fixture::new(&[]);
    drop(AgentWorkspace::open(fixture.app_data.path(), &other.root).unwrap());
    assert_eq!(
        WorkspaceService::new(fixture.app_data.path()).recents(),
        recents
    );
}
