//! Links by title (ADR 0005): the kernel rewrites every link to a file
//! that moves or gets a new title, inside Noura or in another app, checks
//! each file's revision before it writes, and finds backlinks in the
//! index.

use std::collections::BTreeMap;
use std::path::Path;

use local_core::{CreateObjectInput, ObjectPatch, WorkspaceEngine};
use tempfile::TempDir;
use workspace_format::{ObjectTypeRegistry, PropertySchema};

const GARDEN: &str = "project_01j00000000000000000000001";
const TASK: &str = "task_01j00000000000000000000002";

struct Fixture {
    root: TempDir,
    _app_data: TempDir,
    engine: WorkspaceEngine,
}

impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        for (path, text) in files {
            write(root.path(), path, text);
        }
        let engine =
            WorkspaceEngine::create_with_app_data(root.path(), "Links", app_data.path()).unwrap();
        engine.reconcile().unwrap();
        Self {
            root,
            _app_data: app_data,
            engine,
        }
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.root.path().join(path)).unwrap()
    }

    fn write(&self, path: &str, text: &str) {
        write(self.root.path(), path, text);
    }
}

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn project(id: &str) -> String {
    format!("---\nid: {id}\ntype: project\nstatus: active\n---\nOverview.\n")
}

fn task(id: &str, project: &str) -> String {
    format!(
        "---\nid: {id}\ntype: task\nstatus: todo\nproject: '{project}'\n# kept as written\npriority: high\n---\nSee {project}.\n"
    )
}

fn tasks_schema() -> ObjectTypeRegistry {
    let mut registry = ObjectTypeRegistry::new();
    let schema: PropertySchema = serde_json::from_value(serde_json::json!({
        "properties": [
            { "name": "project", "kind": { "reference": "project" }, "nullable": true },
            { "name": "source", "kind": { "reference": "*" }, "nullable": true },
            { "name": "related", "kind": { "list": { "reference": "*" } } }
        ]
    }))
    .unwrap();
    registry.register("task", schema).unwrap();
    registry
}

fn workspace() -> Fixture {
    Fixture::new(&[
        ("Projects/Garden/Garden.md", &project(GARDEN)),
        ("Tasks/Weed.md", &task(TASK, "[[Garden]]")),
        (
            "Notes/Plan.md",
            "Start with [[Garden#Beds|the beds]], then ![[Garden]].\n`[[Garden]]` stays.\n",
        ),
        ("Notes/Other.md", "Nothing about it.\n"),
    ])
}

#[test]
fn an_in_app_rename_rewrites_body_and_property_links_and_nothing_else() {
    let fixture = workspace();
    let revision = fixture.engine.read_object(GARDEN).unwrap().revision;
    let other_before = fixture.read("Notes/Other.md");

    fixture
        .engine
        .update_object(
            GARDEN,
            ObjectPatch {
                title: Some("Backyard".into()),
                body: None,
                properties: BTreeMap::new(),
                remove_properties: Vec::new(),
                expected_revision: revision,
            },
        )
        .unwrap();

    assert_eq!(fixture.read("Tasks/Weed.md"), task(TASK, "[[Backyard]]"));
    assert_eq!(
        fixture.read("Notes/Plan.md"),
        "Start with [[Backyard#Beds|the beds]], then ![[Backyard]].\n`[[Garden]]` stays.\n"
    );
    assert_eq!(fixture.read("Notes/Other.md"), other_before);
    assert!(
        fixture
            .engine
            .state()
            .diagnostics
            .iter()
            .all(|problem| problem.code != "stale_link")
    );
}

#[test]
fn a_rename_in_another_app_rewrites_links_once_the_kernel_sees_it() {
    let fixture = workspace();
    std::fs::rename(
        fixture.root.path().join("Projects/Garden/Garden.md"),
        fixture.root.path().join("Projects/Garden/Orchard.md"),
    )
    .unwrap();

    fixture.engine.reconcile().unwrap();

    assert_eq!(
        fixture.engine.read_object(GARDEN).unwrap().relative_path,
        "Projects/Garden/Orchard.md"
    );
    assert!(
        fixture
            .read("Tasks/Weed.md")
            .contains("project: '[[Orchard]]'")
    );
    assert!(fixture.read("Tasks/Weed.md").contains("See [[Orchard]]."));
    assert!(
        fixture
            .read("Notes/Plan.md")
            .contains("[[Orchard#Beds|the beds]]")
    );
}

#[test]
fn a_move_next_to_a_file_with_the_same_title_links_by_path() {
    let fixture = workspace();
    fixture.write("Archive/2025/Old/Garden.md", "An older garden.\n");
    fixture.engine.reconcile().unwrap();
    // `[[Garden]]` still leads to the project: the archived note is
    // further from the linking files.
    assert_eq!(
        fixture
            .engine
            .resolve_link(Some("Tasks/Weed.md"), "[[Garden]]")
            .unwrap()
            .as_deref(),
        Some("Projects/Garden/Garden.md")
    );

    let revision = fixture.engine.read_object(GARDEN).unwrap().revision;
    fixture
        .engine
        .move_object(GARDEN, "Archive/2025/Projects/Garden.md", &revision)
        .unwrap();

    // `[[Garden]]` would lead to the archived note now, so links to the
    // project take its path, and they still lead to it.
    let weed = fixture.read("Tasks/Weed.md");
    assert!(
        weed.contains("project: '[[Archive/2025/Projects/Garden]]'"),
        "{weed}"
    );
    assert_eq!(
        fixture
            .engine
            .resolve_link(Some("Tasks/Weed.md"), "[[Archive/2025/Projects/Garden]]")
            .unwrap()
            .as_deref(),
        Some("Archive/2025/Projects/Garden.md")
    );
}

#[test]
fn two_files_sharing_a_title_keep_their_own_links() {
    let fixture = Fixture::new(&[
        ("Home/Plan.md", "Home plan.\n"),
        ("Work/Plan.md", "Work plan.\n"),
        ("Notes/Index.md", "[[Home/Plan]] and [[Work/Plan]]\n"),
    ]);

    fixture
        .engine
        .move_file("Work/Plan.md", "Work/Roadmap.md")
        .unwrap();

    assert_eq!(
        fixture.read("Notes/Index.md"),
        "[[Home/Plan]] and [[Roadmap]]\n"
    );
}

#[test]
fn moving_a_folder_moves_the_links_to_every_file_in_it() {
    let fixture = Fixture::new(&[
        ("Home/Plan.md", "Plan.\n"),
        ("Home/Budget.md", "Budget.\n"),
        (
            "Notes/Index.md",
            "[[Home/Plan]], [[Home/Budget]] and [[Plan]]\n",
        ),
    ]);

    fixture.engine.move_folder("Home", "House").unwrap();

    assert_eq!(
        fixture.read("Notes/Index.md"),
        "[[Plan]], [[Budget]] and [[Plan]]\n"
    );
}

#[test]
fn a_file_edited_during_the_rewrite_keeps_the_edit_and_shows_as_a_problem() {
    let fixture = workspace();
    let root = fixture.root.path().to_owned();
    let edited = "Start with [[Garden]], edited elsewhere.\n";
    fixture
        .engine
        .set_link_rewrite_hook(Some(Box::new(move |path| {
            if path == "Notes/Plan.md" {
                std::fs::write(root.join(path), edited).unwrap();
            }
        })));
    let revision = fixture.engine.read_object(GARDEN).unwrap().revision;

    fixture
        .engine
        .move_object(GARDEN, "Projects/Garden/Backyard.md", &revision)
        .unwrap();
    fixture.engine.set_link_rewrite_hook(None);

    assert_eq!(fixture.read("Notes/Plan.md"), edited);
    assert!(fixture.read("Tasks/Weed.md").contains("[[Backyard]]"));
    fixture.engine.reconcile().unwrap();
    let problems: Vec<_> = fixture
        .engine
        .state()
        .diagnostics
        .into_iter()
        .filter(|problem| problem.code == "stale_link")
        .collect();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].relative_path.as_deref(), Some("Notes/Plan.md"));

    // Once the file no longer holds the old link, the problem goes.
    fixture.write("Notes/Plan.md", "Start with [[Backyard]].\n");
    fixture.engine.reconcile().unwrap();
    assert!(
        fixture
            .engine
            .state()
            .diagnostics
            .iter()
            .all(|problem| problem.code != "stale_link")
    );
}

#[test]
fn backlinks_come_from_the_index_and_a_rebuild_finds_the_same() {
    let fixture = workspace();
    let before = fixture
        .engine
        .backlinks("Projects/Garden/Garden.md")
        .unwrap();
    assert_eq!(before, ["Notes/Plan.md", "Tasks/Weed.md"]);

    // A rebuild reads every file into a new, empty index, as after you
    // delete `index.sqlite`.
    fixture.engine.rebuild_index().unwrap();

    assert_eq!(
        fixture
            .engine
            .backlinks("Projects/Garden/Garden.md")
            .unwrap(),
        before
    );
    assert!(
        fixture
            .engine
            .backlinks("Notes/Other.md")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_reference_must_lead_to_an_object_of_its_type() {
    let fixture = workspace();
    let schemas = tasks_schema();
    let create = |properties: serde_json::Value| {
        fixture
            .engine
            .create_object_checked(
                CreateObjectInput {
                    object_type: "task".into(),
                    title: "Mulch".into(),
                    body: String::new(),
                    relative_path: None,
                    properties: serde_json::from_value(properties).unwrap(),
                },
                &schemas,
            )
            .map_err(Box::new)
    };

    let refused = |result: Result<_, Box<local_core::CoreError>>| {
        let error = result.unwrap_err();
        assert_eq!(error.code, "invalid_property");
        error.details.unwrap()["code"].clone()
    };
    assert_eq!(
        refused(create(serde_json::json!({"project": "[[Plan]]"}))),
        "invalid_reference"
    );
    assert_eq!(
        refused(create(serde_json::json!({"project": "[[Nowhere]]"}))),
        "invalid_reference"
    );
    assert_eq!(
        refused(create(serde_json::json!({"project": GARDEN}))),
        "invalid_reference"
    );
    assert_eq!(
        refused(create(
            serde_json::json!({"related": ["[[Plan]]", "[[Nowhere]]"]})
        )),
        "invalid_reference"
    );

    let created = create(serde_json::json!({
        "project": "[[Garden]]",
        "source": "[[Weed]]",
        "related": ["[[Weed]]", "[[Garden]]"],
    }))
    .unwrap()
    .value;
    let text = fixture.read(&created.relative_path);
    assert!(text.contains("project: '[[Garden]]'"), "{text}");
    assert!(text.contains("source: '[[Weed]]'"), "{text}");

    // A plain note is no object, so it can't stand for one.
    assert_eq!(
        refused(create(serde_json::json!({"source": "[[Plan]]"}))),
        "invalid_reference"
    );

    // An update that leaves a link alone doesn't check it again.
    std::fs::remove_file(fixture.root.path().join("Tasks/Weed.md")).unwrap();
    fixture.engine.reconcile().unwrap();
    let current = fixture.engine.read_object(&created.id).unwrap();
    fixture
        .engine
        .update_object_checked(
            &created.id,
            ObjectPatch {
                title: None,
                body: Some("Spread it.\n".into()),
                properties: BTreeMap::new(),
                remove_properties: Vec::new(),
                expected_revision: current.revision,
            },
            &schemas,
        )
        .unwrap();
}

#[test]
fn links_and_titles_noura_writes_for_an_object() {
    let fixture = workspace();
    assert_eq!(fixture.engine.link_to_object(GARDEN).unwrap(), "[[Garden]]");
    fixture.write("Garden.md", "Another.\n");
    fixture.engine.reconcile().unwrap();
    assert_eq!(
        fixture.engine.link_to_object(GARDEN).unwrap(),
        "[[Projects/Garden/Garden]]"
    );
    assert_eq!(
        fixture
            .engine
            .resolve_link(None, "../../etc/passwd")
            .unwrap(),
        None
    );
}

#[test]
fn malformed_and_unreadable_files_are_left_as_they_are() {
    let fixture = Fixture::new(&[
        ("Plan.md", "Plan.\n"),
        ("Broken.md", "---\nid: [\n---\n[[Plan]] and [[unclosed\n"),
        (
            "Plain.md",
            "---\ntitle: 'It''s [[Plan]]'\nalso: '[[Plan]]'\n---\n[[Plan]]\n",
        ),
    ]);
    std::fs::write(fixture.root.path().join("Binary.md"), b"[[Plan]] \xff\xfe").unwrap();
    fixture.engine.reconcile().unwrap();

    fixture.engine.move_file("Plan.md", "Roadmap's.md").unwrap();

    assert_eq!(
        fixture.read("Broken.md"),
        "---\nid: [\n---\n[[Roadmap's]] and [[unclosed\n"
    );
    // A title with a quote can't stand in single-quoted YAML as it is, so
    // the frontmatter's text changes too, and that file without an ID is
    // left alone and shows as a problem.
    assert_eq!(
        fixture.read("Plain.md"),
        "---\ntitle: 'It''s [[Plan]]'\nalso: '[[Plan]]'\n---\n[[Plan]]\n"
    );
    assert!(
        fixture
            .engine
            .state()
            .diagnostics
            .iter()
            .any(|problem| problem.code == "stale_link"
                && problem.relative_path.as_deref() == Some("Plain.md"))
    );
    assert_eq!(
        std::fs::read(fixture.root.path().join("Binary.md")).unwrap(),
        b"[[Plan]] \xff\xfe"
    );
}

#[test]
fn a_copy_that_shares_an_id_is_no_move() {
    let fixture = workspace();
    std::fs::copy(
        fixture.root.path().join("Projects/Garden/Garden.md"),
        fixture.root.path().join("Projects/Garden/Garden copy.md"),
    )
    .unwrap();
    let weed = fixture.read("Tasks/Weed.md");

    fixture.engine.reconcile().unwrap();

    assert_eq!(fixture.read("Tasks/Weed.md"), weed);
}
