//! Seam 3: the Tasks plugin in the real host. Its manifest declares the
//! `task` type with today's v1 fields, and other plugins change tasks only
//! through the commands it declares: a plugin that names Tasks as a
//! command target can call them, one that doesn't is denied, and Tasks
//! itself can't write an object of another type. The app-core suite runs
//! the same commands against real files.

mod support;

use std::{path::Path, sync::Arc};

use plugin_host::{
    Capability, Limits, ObjectTypes, Router, RunningPlugin, Services, WorkspaceObjects,
    read_manifest,
};
use support::plugins::{self, FakeObjects};
use workspace_format::PropertySchema;

/// The Tasks plugin's manifest, as its crate ships it.
const MANIFEST: &str = include_str!("../../plugins/tasks/plugin.toml");

/// The shared fixtures that hold today's v1 task schema.
const FIXTURES: &str =
    include_str!("../../../docs/workspace-format/fixtures/object-envelope-v1.json");

const TASK_ID: &str = "task_01jbcdefghjkmnpqrstvwxyz0a";

/// The Tasks plugin, started with what its manifest declares, the probe
/// as `caller` with `targets` as its command targets, and the objects the
/// plugins write.
struct Workspace {
    tasks: Arc<RunningPlugin>,
    caller: Arc<RunningPlugin>,
    types: ObjectTypes,
    objects: Arc<FakeObjects>,
    _router: Arc<Router>,
    _folders: Vec<tempfile::TempDir>,
}

impl Workspace {
    fn new(targets: &str) -> Option<Self> {
        let tasks = plugins::built("tasks_plugin")?;
        let probe = plugins::built("probe_plugin")?;
        let router = Arc::new(Router::default());
        let types = ObjectTypes::new();
        let objects = Arc::new(FakeObjects::default());
        let service: Arc<dyn WorkspaceObjects> = objects.clone();
        let folder = tempfile::tempdir().unwrap();
        let library = tempfile::tempdir().unwrap();
        plugins::write_folder(folder.path(), MANIFEST, "tasks_plugin.wasm", &tasks);
        let host = plugins::host(library.path());
        host.install(&host.inspect(folder.path()).unwrap()).unwrap();
        let loaded = host.load("tasks").unwrap();
        let tasks = host.start(
            &loaded,
            &loaded.manifest().capabilities,
            Services {
                objects: Some(service),
                types: types.clone(),
                router: Arc::downgrade(&router),
                ..Services::default()
            },
        );
        let (caller, caller_folders) = plugins::start(
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
        router.insert(tasks.clone());
        router.insert(caller.clone());
        let [caller_folder, caller_library] = caller_folders;
        Some(Self {
            tasks,
            caller,
            types,
            objects,
            _router: router,
            _folders: vec![folder, library, caller_folder, caller_library],
        })
    }

    /// What the caller's call to the Tasks command `command` with `input`
    /// answered.
    fn call(&self, command: &str, input: &str) -> String {
        let nodes = self
            .caller
            .render(&format!("call:tasks/{command}:{input}"))
            .expect("the caller renders");
        plugins::texts(&nodes).join("\n")
    }

    /// The JSON a call answered with.
    fn answer(&self, command: &str, input: &str) -> serde_json::Value {
        let answer = self.call(command, input);
        let json = answer
            .strip_prefix("answer ")
            .unwrap_or_else(|| panic!("{command} failed: {answer}"));
        serde_json::from_str(json).unwrap()
    }

    /// The JSON of a call that failed.
    fn refusal(&self, command: &str, input: &str) -> serde_json::Value {
        let answer = self.call(command, input);
        let json = answer
            .strip_prefix("error failed ")
            .unwrap_or_else(|| panic!("{command} didn't fail: {answer}"));
        serde_json::from_str(json).unwrap()
    }
}

#[test]
fn the_manifest_declares_the_task_type_with_todays_v1_fields() {
    let manifest = read_manifest(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../plugins/tasks"
    )))
    .expect("the manifest reads");
    assert_eq!(manifest.id, "tasks");
    assert_eq!(
        manifest.capabilities,
        [
            Capability::WorkspaceRead,
            Capability::ObjectTypes,
            Capability::Views,
            Capability::InboxSections,
            Capability::Commands {
                targets: Vec::new()
            },
            Capability::Tools,
        ]
    );
    let Some(workspace) = Workspace::new("[\"hello\"]") else {
        return;
    };

    let registered = workspace.tasks.register_object_types().unwrap();

    assert_eq!(registered, [("task".to_owned(), Ok(()))]);
    assert_eq!(workspace.types.owner("task").as_deref(), Some("tasks"));
    let fixtures: serde_json::Value = serde_json::from_str(FIXTURES).unwrap();
    let expected: PropertySchema =
        serde_json::from_value(fixtures["schemas"]["task"].clone()).unwrap();
    assert_eq!(workspace.types.schemas().schema("task"), Some(&expected));
}

#[test]
fn tasks_declares_new_task_for_the_palette_and_five_commands_for_other_plugins() {
    let Some(workspace) = Workspace::new("[\"hello\"]") else {
        return;
    };

    let commands: Vec<(String, bool, bool)> = workspace
        .tasks
        .commands()
        .unwrap()
        .into_iter()
        .map(|command| (command.id, command.palette, command.plugins))
        .collect();

    assert_eq!(
        commands,
        [
            ("new-task".to_owned(), true, false),
            ("create".to_owned(), false, true),
            ("update".to_owned(), false, true),
            ("complete".to_owned(), false, true),
            ("reopen".to_owned(), false, true),
            ("list".to_owned(), false, true),
        ]
    );
}

#[test]
fn list_answers_every_task_or_one_projects_tasks_without_their_bodies() {
    let Some(workspace) = Workspace::new("[\"tasks\"]") else {
        return;
    };
    workspace.tasks.register_object_types().unwrap();
    let garden = "project_01jbcdefghjkmnpqrstvwxyz0b";
    workspace.answer(
        "create",
        &format!(r#"{{"title":"Weed the beds","properties":{{"project":"{garden}"}},"body":"All of them."}}"#),
    );

    let all = workspace.answer("list", "{}");
    let tasks = all["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], TASK_ID);
    assert_eq!(tasks[0]["title"], "Weed the beds");
    assert_eq!(tasks[0]["revision"], "rev-1");
    assert_eq!(tasks[0]["properties"]["project"], garden);
    assert_eq!(tasks[0].get("body"), None);
    let garden_tasks = workspace.answer("list", &format!(r#"{{"project":"{garden}"}}"#));
    assert_eq!(garden_tasks["tasks"], all["tasks"]);
    let other = workspace.answer(
        "list",
        r#"{"project":"project_01jbcdefghjkmnpqrstvwxyz0c"}"#,
    );
    assert_eq!(other["tasks"], serde_json::json!([]));
    assert_eq!(
        workspace.refusal("list", r#"{"project":7}"#)["code"],
        "invalid_input"
    );
}

#[test]
fn a_plugin_that_names_tasks_as_a_target_creates_updates_completes_and_reopens_tasks() {
    let Some(workspace) = Workspace::new("[\"tasks\"]") else {
        return;
    };
    workspace.tasks.register_object_types().unwrap();

    let created = workspace.answer(
        "create",
        r#"{"title":"Water the plants","properties":{"priority":"high"},"body":"Every morning."}"#,
    );
    assert_eq!(created["id"], TASK_ID);
    assert_eq!(created["revision"], "rev-1");
    assert_eq!(created["properties"]["priority"], "high");
    assert_eq!(created["body"], "Every morning.");

    let target = format!(r#""id":"{TASK_ID}","expected_revision":"rev-1""#);
    workspace.answer(
        "update",
        &format!(
            r#"{{{target},"properties":{{"status":"in-progress","priority":"urgent","project":"project_01jbcdefghjkmnpqrstvwxyz0b","due":"2026-10-24","kanban_order":"a0"}}}}"#
        ),
    );
    workspace.answer(
        "update",
        &format!(r#"{{{target},"properties":{{"due":null}}}}"#),
    );
    workspace.answer("complete", &format!("{{{target}}}"));
    workspace.answer("reopen", &format!("{{{target}}}"));

    assert_eq!(
        workspace.objects.calls(),
        [
            "create task Water the plants".to_owned(),
            format!(
                r#"update {TASK_ID} rev-1 {{"due":"2026-10-24","kanban_order":"a0","priority":"urgent","project":"project_01jbcdefghjkmnpqrstvwxyz0b","status":"in-progress"}}"#
            ),
            format!("update {TASK_ID} rev-1 {{}}"),
            format!(r#"update {TASK_ID} rev-1 {{"status":"done"}}"#),
            format!(r#"update {TASK_ID} rev-1 {{"status":"todo"}}"#),
        ]
    );
}

#[test]
fn a_plugin_that_doesnt_name_tasks_as_a_target_is_denied() {
    let Some(workspace) = Workspace::new("[\"hello\"]") else {
        return;
    };
    workspace.tasks.register_object_types().unwrap();

    for command in ["create", "update", "complete", "reopen", "list"] {
        assert_eq!(
            workspace.call(command, r#"{"title":"Water the plants"}"#),
            format!("error target-not-declared tasks/{command}")
        );
    }
    assert_eq!(workspace.objects.calls(), Vec::<String>::new());
}

#[test]
fn changes_need_the_tasks_id_and_revision_and_only_task_fields() {
    let Some(workspace) = Workspace::new("[\"tasks\"]") else {
        return;
    };
    workspace.tasks.register_object_types().unwrap();

    assert_eq!(
        workspace.refusal("complete", &format!(r#"{{"id":"{TASK_ID}"}}"#))["code"],
        "target_required"
    );
    let unknown = workspace.refusal(
        "update",
        &format!(
            r#"{{"id":"{TASK_ID}","expected_revision":"rev-1","properties":{{"color":"red"}}}}"#
        ),
    );
    assert_eq!(unknown["code"], "unknown_property");
    assert_eq!(unknown["property"], "color");
    assert_eq!(
        workspace.refusal("create", r#"{"title":"  "}"#)["code"],
        "title_required"
    );
    assert_eq!(
        workspace.refusal("create", "not json")["code"],
        "invalid_input"
    );
    assert_eq!(workspace.objects.calls(), Vec::<String>::new());
}

#[test]
fn tasks_cant_write_an_object_of_another_type() {
    let Some(workspace) = Workspace::new("[\"tasks\"]") else {
        return;
    };
    workspace.tasks.register_object_types().unwrap();
    let note = "note_01jbcdefghjkmnpqrstvwxyz0a";

    for (command, input) in [
        (
            "update",
            format!(
                r#"{{"id":"{note}","expected_revision":"rev-1","properties":{{"status":"done"}}}}"#
            ),
        ),
        (
            "complete",
            format!(r#"{{"id":"{note}","expected_revision":"rev-1"}}"#),
        ),
    ] {
        let refused = workspace.refusal(command, &input);
        assert_eq!(refused["code"], "not_a_task", "{command}");
        assert_eq!(refused["capability"], "object-types", "{command}");
    }
    assert_eq!(workspace.objects.calls(), Vec::<String>::new());
}
