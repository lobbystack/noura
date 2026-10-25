//! Seam 3: the Projects plugin in the real host. Its manifest declares the
//! `project` type with today's statuses and names Tasks as its command
//! target, it offers create and update to other plugins, and it changes
//! tasks only through the Tasks commands: a direct write to a task from a
//! plugin with Projects' manifest is denied, and a call to Tasks goes
//! through. The app-core suite runs the board against real files.

mod support;

use std::{path::Path, sync::Arc};

use plugin_host::{
    Capability, Limits, ObjectRecord, ObjectTypes, Router, RunningPlugin, Services,
    WorkspaceObjects, WorkspacePath, read_manifest,
};
use support::plugins::{self, FakeObjects};
use workspace_format::PropertySchema;

/// The Projects plugin's manifest, as its crate ships it.
const MANIFEST: &str = include_str!("../../plugins/projects/plugin.toml");

/// The Tasks plugin's manifest, as its crate ships it.
const TASKS_MANIFEST: &str = include_str!("../../plugins/tasks/plugin.toml");

/// The shared fixtures that hold today's project schema.
const FIXTURES: &str =
    include_str!("../../../docs/workspace-format/fixtures/object-envelope-v1.json");

const PROJECT_ID: &str = "project_01jbcdefghjkmnpqrstvwxyz0a";
const TASK_ID: &str = "task_01jbcdefghjkmnpqrstvwxyz0b";

/// Start the plugin in `folder` with what its manifest declares.
fn start(folder: &Path, id: &str, services: Services) -> (Arc<RunningPlugin>, tempfile::TempDir) {
    let library = tempfile::tempdir().unwrap();
    let host = plugins::host(library.path());
    host.install(&host.inspect(folder).unwrap()).unwrap();
    let loaded = host.load(id).unwrap();
    let running = host.start(&loaded, &loaded.manifest().capabilities, services);
    (running, library)
}

/// The text lines a probe's page `page` answered with.
fn lines(probe: &RunningPlugin, page: &str) -> Vec<String> {
    plugins::texts(&probe.render(page).expect("the probe renders"))
}

#[test]
fn the_manifest_declares_the_project_type_with_todays_statuses_and_names_tasks_as_its_target() {
    let manifest = read_manifest(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../plugins/projects"
    )))
    .expect("the manifest reads");
    assert_eq!(manifest.id, "projects");
    assert_eq!(
        manifest.capabilities,
        [
            Capability::WorkspaceRead,
            Capability::ObjectTypes,
            Capability::Views,
            Capability::Commands {
                targets: vec!["tasks".to_owned()]
            },
            Capability::Tools,
        ]
    );
    let Some(component) = plugins::built("projects_plugin") else {
        return;
    };
    let folder = tempfile::tempdir().unwrap();
    plugins::write_folder(folder.path(), MANIFEST, "projects_plugin.wasm", &component);
    let types = ObjectTypes::new();
    let (projects, _library) = start(
        folder.path(),
        "projects",
        Services {
            types: types.clone(),
            ..Services::default()
        },
    );

    let registered = projects.register_object_types().unwrap();

    assert_eq!(registered, [("project".to_owned(), Ok(()))]);
    assert_eq!(types.owner("project").as_deref(), Some("projects"));
    let fixtures: serde_json::Value = serde_json::from_str(FIXTURES).unwrap();
    let expected: PropertySchema =
        serde_json::from_value(fixtures["schemas"]["project"].clone()).unwrap();
    assert_eq!(types.schemas().schema("project"), Some(&expected));
    let status = &fixtures["schemas"]["project"]["properties"][0];
    assert_eq!(status["name"], "status");
    assert_eq!(
        status["kind"]["enum"],
        serde_json::json!(["planned", "active", "on-hold", "completed", "cancelled"])
    );
    assert_eq!(status["default"], "planned");
}

#[test]
fn no_other_plugin_registers_the_project_type() {
    let types = ObjectTypes::new();

    let refused = types
        .register("recipes", 1, "project", r#"{"properties":[]}"#)
        .expect_err("project is held for Projects");

    assert_eq!(refused.code(), "type_reserved");
    types
        .register("projects", 2, "project", r#"{"properties":[]}"#)
        .unwrap();
    assert_eq!(types.owner("project").as_deref(), Some("projects"));
}

/// The real Projects plugin, the probe as `caller` naming `targets` as its
/// command targets, and the objects they write.
struct Callers {
    caller: Arc<RunningPlugin>,
    objects: Arc<FakeObjects>,
    projects: Arc<RunningPlugin>,
    _router: Arc<Router>,
    _folders: Vec<tempfile::TempDir>,
}

impl Callers {
    fn new(targets: &str) -> Option<Self> {
        let component = plugins::built("projects_plugin")?;
        let probe = plugins::built("probe_plugin")?;
        let router = Arc::new(Router::default());
        let types = ObjectTypes::new();
        let objects = Arc::new(FakeObjects::default());
        let service: Arc<dyn WorkspaceObjects> = objects.clone();
        let folder = tempfile::tempdir().unwrap();
        plugins::write_folder(folder.path(), MANIFEST, "projects_plugin.wasm", &component);
        let (projects, library) = start(
            folder.path(),
            "projects",
            Services {
                objects: Some(service),
                types: types.clone(),
                router: Arc::downgrade(&router),
                ..Services::default()
            },
        );
        projects.register_object_types().unwrap();
        let (caller, [caller_folder, caller_library]) = plugins::start(
            "caller",
            &probe,
            &[
                "name = \"views\"",
                &format!("name = \"commands\"\ntargets = {targets}"),
            ],
            Services {
                router: Arc::downgrade(&router),
                ..Services::default()
            },
            Limits::default(),
        );
        router.insert(projects.clone());
        router.insert(caller.clone());
        Some(Self {
            caller,
            objects,
            projects,
            _router: router,
            _folders: vec![folder, library, caller_folder, caller_library],
        })
    }

    /// What the caller's call to the Projects command `command` answered.
    fn call(&self, command: &str, input: &str) -> String {
        lines(&self.caller, &format!("call:projects/{command}:{input}")).join("\n")
    }

    fn answer(&self, command: &str, input: &str) -> serde_json::Value {
        let answer = self.call(command, input);
        let json = answer
            .strip_prefix("answer ")
            .unwrap_or_else(|| panic!("{command} failed: {answer}"));
        serde_json::from_str(json).unwrap()
    }

    fn refusal(&self, command: &str, input: &str) -> serde_json::Value {
        let answer = self.call(command, input);
        let json = answer
            .strip_prefix("error failed ")
            .unwrap_or_else(|| panic!("{command} didn't fail: {answer}"));
        serde_json::from_str(json).unwrap()
    }
}

#[test]
fn projects_offers_create_and_update_to_other_plugins() {
    let Some(callers) = Callers::new("[\"projects\"]") else {
        return;
    };

    let commands: Vec<(String, bool, bool)> = callers
        .projects
        .commands()
        .unwrap()
        .into_iter()
        .map(|command| (command.id, command.palette, command.plugins))
        .collect();
    assert_eq!(
        commands,
        [
            ("create".to_owned(), false, true),
            ("update".to_owned(), false, true),
        ]
    );

    let created = callers.answer(
        "create",
        r#"{"title":"Garden","body":"Grow vegetables.","properties":{"status":"active"}}"#,
    );
    assert_eq!(created["id"], PROJECT_ID);
    assert_eq!(created["revision"], "rev-1");
    assert_eq!(created["properties"]["status"], "active");
    assert_eq!(created["body"], "Grow vegetables.");
    callers.answer(
        "update",
        &format!(
            r#"{{"id":"{PROJECT_ID}","expected_revision":"rev-1","properties":{{"status":"completed"}},"body":"Done."}}"#
        ),
    );

    assert_eq!(
        callers.objects.calls(),
        [
            "create project Garden".to_owned(),
            format!(r#"update {PROJECT_ID} rev-1 {{"status":"completed"}}"#),
        ]
    );
    assert_eq!(
        callers.refusal("create", r#"{"title":" "}"#)["code"],
        "title_required"
    );
    assert_eq!(
        callers.refusal("update", &format!(r#"{{"id":"{PROJECT_ID}"}}"#))["code"],
        "target_required"
    );
    let unknown = callers.refusal(
        "update",
        &format!(
            r#"{{"id":"{PROJECT_ID}","expected_revision":"rev-1","properties":{{"color":"red"}}}}"#
        ),
    );
    assert_eq!(unknown["code"], "unknown_property");
    assert_eq!(callers.objects.calls().len(), 2);
}

#[test]
fn a_plugin_that_doesnt_name_projects_as_a_target_is_denied() {
    let Some(callers) = Callers::new("[\"hello\"]") else {
        return;
    };

    for command in ["create", "update"] {
        assert_eq!(
            callers.call(command, r#"{"title":"Garden"}"#),
            format!("error target-not-declared projects/{command}")
        );
    }
    assert_eq!(callers.objects.calls(), Vec::<String>::new());
}

#[test]
fn projects_changes_tasks_only_through_the_tasks_commands() {
    let (Some(tasks), Some(probe)) = (
        plugins::built("tasks_plugin"),
        plugins::built("probe_plugin"),
    ) else {
        return;
    };
    let router = Arc::new(Router::default());
    let types = ObjectTypes::new();
    let objects = Arc::new(FakeObjects::default());
    objects.objects.lock().unwrap().push(ObjectRecord {
        id: TASK_ID.into(),
        object_type: "task".into(),
        title: "Weed the beds".into(),
        path: WorkspacePath::new("tasks/Weed the beds.md").unwrap(),
        body: String::new(),
        properties: r#"{"status":"todo","priority":"medium"}"#.into(),
        revision: "rev-1".into(),
        created: None,
        updated: None,
        problem: None,
    });
    let service: Arc<dyn WorkspaceObjects> = objects.clone();
    let services = || Services {
        objects: Some(service.clone()),
        types: types.clone(),
        router: Arc::downgrade(&router),
        ..Services::default()
    };
    let tasks_folder = tempfile::tempdir().unwrap();
    plugins::write_folder(
        tasks_folder.path(),
        TASKS_MANIFEST,
        "tasks_plugin.wasm",
        &tasks,
    );
    let (tasks, _tasks_library) = start(tasks_folder.path(), "tasks", services());
    tasks.register_object_types().unwrap();
    // The probe runs with Projects' ID and exactly the capabilities
    // Projects' manifest declares, so it can try what Projects' code
    // could.
    let projects_folder = tempfile::tempdir().unwrap();
    plugins::write_folder(
        projects_folder.path(),
        MANIFEST,
        "projects_plugin.wasm",
        &probe,
    );
    let (projects, _projects_library) = start(projects_folder.path(), "projects", services());
    router.insert(tasks.clone());
    router.insert(projects.clone());

    assert_eq!(
        lines(
            &projects,
            &format!(r#"update-object:{TASK_ID}:rev-1:{{"status":"done"}}"#)
        ),
        ["error capability-not-declared object-types"]
    );
    assert_eq!(
        lines(&projects, "create-object:task:Rake the leaves"),
        ["error capability-not-declared object-types"]
    );
    assert_eq!(objects.calls(), Vec::<String>::new());

    let answer = lines(
        &projects,
        &format!(
            r#"call:tasks/update:{{"id":"{TASK_ID}","expected_revision":"rev-1","properties":{{"status":"done","kanban_order":"a0"}}}}"#
        ),
    );
    assert!(answer[0].starts_with("answer "), "{answer:?}");
    assert_eq!(
        objects.calls(),
        [format!(
            r#"update {TASK_ID} rev-1 {{"kanban_order":"a0","status":"done"}}"#
        )]
    );
}
