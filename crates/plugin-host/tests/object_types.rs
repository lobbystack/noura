//! Seam 3: object types. A plugin that holds `object-types` registers its
//! types with a schema in the workspace format's declaration, the kernel
//! keeps its own types, and a type stays with the first running plugin
//! that registers it. Object calls reach the service only for the calling
//! plugin's own types, and cross with their revisions, schema problems and
//! structured errors.

mod support;

use std::sync::Arc;

use plugin_host::{
    Capability, Event, ObjectRecord, ObjectTypes, PluginError, PropertyProblem, RunningPlugin,
    ServiceError, Services, TypeError, WorkspacePath,
};
use support::plugins::{self, FakeObjects};

const TYPES_AND_VIEWS: [&str; 2] = ["name = \"object-types\"", "name = \"views\""];

const NOTE_ID: &str = "note_01jbcdefghjkmnpqrstvwxyz0a";
const RECIPE_ID: &str = "recipe_01jbcdefghjkmnpqrstvwxyz0a";

/// The recipes plugin running as `id` with `capabilities`, sharing `types`
/// and `objects` with the other plugins of the workspace.
fn recipes(
    id: &str,
    capabilities: &[&str],
    types: &ObjectTypes,
    objects: &Arc<FakeObjects>,
) -> Option<(Arc<RunningPlugin>, [tempfile::TempDir; 2])> {
    let component = plugins::built("recipes_plugin")?;
    Some(plugins::running_as(
        id,
        &component,
        capabilities,
        Services {
            objects: Some(objects.clone()),
            types: types.clone(),
            ..Services::default()
        },
    ))
}

/// Run `call` on the recipes page and return the answer's lines.
fn call(instance: &RunningPlugin, call: &str) -> Vec<String> {
    instance
        .handle_event("recipes", &Event::Pressed(call.to_owned()))
        .expect("the plugin takes the call");
    plugins::texts(&instance.render("recipes").expect("the page renders"))
}

fn recipe() -> ObjectRecord {
    ObjectRecord {
        id: RECIPE_ID.into(),
        object_type: "recipe".into(),
        title: "Soup".into(),
        path: WorkspacePath::new("recipes/Soup.md").unwrap(),
        body: "Simmer.\n".into(),
        properties: r#"{"course":"brunch"}"#.into(),
        revision: "rev-1".into(),
        created: Some("2026-10-20T10:00:00Z".into()),
        updated: Some("2026-10-20T10:00:00Z".into()),
        problem: Some(PropertyProblem {
            property: "course".into(),
            code: "not_allowed".into(),
        }),
    }
}

#[test]
fn registering_types_needs_the_object_types_capability() {
    let types = ObjectTypes::new();
    let objects = Arc::new(FakeObjects::default());
    let Some((instance, _folders)) = recipes("recipes", &["name = \"views\""], &types, &objects)
    else {
        return;
    };

    assert_eq!(
        instance.register_object_types().err(),
        Some(PluginError::CapabilityNotDeclared {
            capability: "object-types"
        })
    );
    assert_eq!(types.owner("recipe"), None);
    assert_eq!(
        instance.render_panel(RECIPE_ID).err(),
        Some(PluginError::CapabilityNotDeclared {
            capability: "object-types"
        })
    );
    assert_eq!(
        call(&instance, "list|recipe"),
        ["error\tcapability-not-declared\tobject-types"]
    );
    assert!(objects.calls().is_empty(), "{:?}", objects.calls());
}

#[test]
fn a_plugin_registers_its_types_and_the_kernels_note_is_refused() {
    let types = ObjectTypes::new();
    let objects = Arc::new(FakeObjects::default());
    let Some((instance, _folders)) = recipes("recipes", &TYPES_AND_VIEWS, &types, &objects) else {
        return;
    };

    let registered = instance.register_object_types().unwrap();

    assert_eq!(
        registered,
        [
            ("recipe".to_owned(), Ok(())),
            (
                "note".to_owned(),
                Err(TypeError::KernelType {
                    object_type: "note".into()
                })
            ),
        ]
    );
    assert_eq!(types.owner("recipe").as_deref(), Some("recipes"));
    assert_eq!(types.owner("note"), None);
    assert_eq!(instance.object_types(), ["recipe"]);
    assert!(types.schemas().contains("recipe"));
}

#[test]
fn a_type_another_running_plugin_holds_is_refused_until_that_plugin_stops() {
    let types = ObjectTypes::new();
    let objects = Arc::new(FakeObjects::default());
    let Some((first, _first_folders)) = recipes("recipes", &TYPES_AND_VIEWS, &types, &objects)
    else {
        return;
    };
    let (second, _second_folders) =
        recipes("recipes-two", &TYPES_AND_VIEWS, &types, &objects).unwrap();
    first.register_object_types().unwrap();

    let registered = second.register_object_types().unwrap();

    assert_eq!(
        registered[0],
        (
            "recipe".to_owned(),
            Err(TypeError::HeldByPlugin {
                object_type: "recipe".into(),
                plugin: "recipes".into()
            })
        )
    );
    assert_eq!(registered[0].1.as_ref().unwrap_err().code(), "type_held");
    assert_eq!(types.owner("recipe").as_deref(), Some("recipes"));

    drop(first);
    assert_eq!(types.owner("recipe"), None);
    let registered = second.register_object_types().unwrap();
    assert_eq!(registered[0], ("recipe".to_owned(), Ok(())));
    assert_eq!(types.owner("recipe").as_deref(), Some("recipes-two"));
}

#[test]
fn a_plugin_keeps_its_types_in_a_new_store_and_a_new_start_of_it_takes_them_over() {
    let types = ObjectTypes::new();
    let objects = Arc::new(FakeObjects::default());
    let Some((first, _first_folders)) = recipes("recipes", &TYPES_AND_VIEWS, &types, &objects)
    else {
        return;
    };
    first.register_object_types().unwrap();

    // A new store, as after a call that failed, registers them again.
    first.restart();
    assert_eq!(
        first.register_object_types().unwrap()[0],
        ("recipe".to_owned(), Ok(()))
    );
    assert_eq!(first.object_types(), ["recipe"]);

    // A new start of the plugin, as after a reload, takes them over, and
    // the old start ending leaves them with the new one.
    let (second, _second_folders) = recipes("recipes", &TYPES_AND_VIEWS, &types, &objects).unwrap();
    assert_eq!(
        second.register_object_types().unwrap()[0],
        ("recipe".to_owned(), Ok(()))
    );
    assert!(first.object_types().is_empty());
    assert_eq!(
        call(&first, "list|recipe"),
        ["error\tcapability-not-declared\tobject-types"]
    );
    drop(first);
    assert_eq!(types.owner("recipe").as_deref(), Some("recipes"));
    assert_eq!(second.object_types(), ["recipe"]);
    assert!(types.schemas().contains("recipe"));
}

#[test]
fn revoking_object_types_or_turning_the_plugin_off_gives_its_types_up() {
    let types = ObjectTypes::new();
    let objects = Arc::new(FakeObjects::default());
    let Some((plugin, _folders)) = recipes("recipes", &TYPES_AND_VIEWS, &types, &objects) else {
        return;
    };
    plugin.register_object_types().unwrap();

    plugin.set_approved(&[Capability::Views]);

    assert_eq!(types.owner("recipe"), None);
    assert!(!types.schemas().contains("recipe"));
    assert_eq!(
        call(&plugin, "list|recipe"),
        ["error\tcapability-not-declared\tobject-types"]
    );
    assert!(objects.calls().is_empty(), "{:?}", objects.calls());

    plugin.set_approved(&[Capability::ObjectTypes, Capability::Views]);
    plugin.register_object_types().unwrap();
    assert_eq!(types.owner("recipe").as_deref(), Some("recipes"));
    plugin.release_object_types();
    assert_eq!(types.owner("recipe"), None);
}

#[test]
fn a_schema_the_workspace_format_refuses_fails_with_a_structured_error() {
    let types = ObjectTypes::new();
    let refused = [
        ("Recipe", r#"{"properties":[]}"#, "invalid_type_name"),
        ("recipe", "not json", "invalid_schema"),
        (
            "recipe",
            r#"{"properties":[{"name":"id","kind":"string"}]}"#,
            "reserved_property",
        ),
        (
            "recipe",
            r#"{"properties":[{"name":"a","kind":"string"},{"name":"a","kind":"number"}]}"#,
            "duplicate_property",
        ),
        (
            "recipe",
            r#"{"properties":[{"name":"a","kind":"number","default":"x"}]}"#,
            "invalid_default",
        ),
        ("note", r#"{"properties":[]}"#, "kernel_type"),
        ("workspace", r#"{"properties":[]}"#, "kernel_type"),
        // Tasks belong to the Tasks plugin, projects to the Projects
        // plugin, and chats and their messages to the AI plugin, even
        // while they are off.
        ("task", r#"{"properties":[]}"#, "type_reserved"),
        ("project", r#"{"properties":[]}"#, "type_reserved"),
        ("chat", r#"{"properties":[]}"#, "type_reserved"),
        ("chat-message", r#"{"properties":[]}"#, "type_reserved"),
    ];
    for (object_type, schema, code) in refused {
        let error = types
            .register("recipes", 1, object_type, schema)
            .expect_err(code);
        assert_eq!(error.code(), code, "{object_type} {schema}");
    }
    assert_eq!(types.owner("recipe"), None);
    assert_eq!(types.owner("task"), None);
    // The plugin it is held for registers it.
    types
        .register("tasks", 2, "task", r#"{"properties":[]}"#)
        .unwrap();
    assert_eq!(types.owner("task").as_deref(), Some("tasks"));
    types
        .register("ai", 3, "chat", r#"{"properties":[]}"#)
        .unwrap();
    assert_eq!(types.owner("chat").as_deref(), Some("ai"));
}

#[test]
fn a_plugin_writes_only_objects_of_its_own_types() {
    let types = ObjectTypes::new();
    let objects = Arc::new(FakeObjects::default());
    let Some((owner, _owner_folders)) = recipes("recipes", &TYPES_AND_VIEWS, &types, &objects)
    else {
        return;
    };
    let (other, _other_folders) =
        recipes("recipes-two", &TYPES_AND_VIEWS, &types, &objects).unwrap();
    owner.register_object_types().unwrap();
    other.register_object_types().unwrap();

    let refused = "error\tcapability-not-declared\tobject-types";
    for attempt in [
        "create|note|Plan|-|{}|Body".to_owned(),
        format!("get|{NOTE_ID}"),
        "list|note".to_owned(),
        format!("update|{NOTE_ID}|rev-1|{{}}"),
        format!("move|{NOTE_ID}|rev-1|Elsewhere.md"),
        format!("delete|{NOTE_ID}|rev-1"),
    ] {
        assert_eq!(call(&owner, &attempt), [refused], "{attempt}");
    }
    // The second plugin's registration of recipe was refused, so recipes
    // stay the first plugin's.
    for attempt in [
        "create|recipe|Soup|-|{}|Body".to_owned(),
        format!("get|{RECIPE_ID}"),
        "list|recipe".to_owned(),
        format!("update|{RECIPE_ID}|rev-1|{{}}"),
        format!("move|{RECIPE_ID}|rev-1|recipes/Stew.md"),
        format!("delete|{RECIPE_ID}|rev-1"),
        // An ID with no type in it.
        "delete|soup|rev-1".to_owned(),
    ] {
        assert_eq!(call(&other, &attempt), [refused], "{attempt}");
    }
    assert!(objects.calls().is_empty(), "{:?}", objects.calls());
}

#[test]
fn object_calls_cross_with_their_revision_and_schema_problem() {
    let types = ObjectTypes::new();
    let objects = Arc::new(FakeObjects::default());
    objects.objects.lock().unwrap().push(recipe());
    let Some((instance, _folders)) = recipes("recipes", &TYPES_AND_VIEWS, &types, &objects) else {
        return;
    };
    instance.register_object_types().unwrap();
    let line = format!(
        "object\t{RECIPE_ID}\trev-1\trecipes/Soup.md\tcourse:not_allowed\t{{\"course\":\"brunch\"}}\t\"Simmer.\\n\""
    );

    assert_eq!(
        call(&instance, &format!("get|{RECIPE_ID}")),
        std::slice::from_ref(&line)
    );
    assert_eq!(call(&instance, "list|recipe"), [line]);
    assert_eq!(
        call(
            &instance,
            r#"create|recipe|Stew|recipes/Winter/Stew.md|{"servings":4}|Hot."#
        ),
        [format!(
            "object\t{RECIPE_ID}\trev-1\trecipes/Winter/Stew.md\t-\t{{\"servings\":4}}\t\"Hot.\""
        )]
    );
    call(
        &instance,
        &format!(r#"update|{RECIPE_ID}|rev-1|{{"servings":3}}"#),
    );
    call(
        &instance,
        &format!("move|{RECIPE_ID}|rev-1|recipes/Stew.md"),
    );
    assert_eq!(
        call(&instance, &format!("delete|{RECIPE_ID}|rev-1")),
        [format!("deleted\t{RECIPE_ID}")]
    );
    assert_eq!(
        objects.calls(),
        [
            format!("get {RECIPE_ID}"),
            "list recipe".to_owned(),
            "create recipe Stew".to_owned(),
            format!(r#"update {RECIPE_ID} rev-1 {{"servings":3}}"#),
            format!("move {RECIPE_ID} rev-1 recipes/Stew.md"),
            format!("delete {RECIPE_ID} rev-1"),
        ]
    );
}

#[test]
fn paths_that_arent_workspace_relative_never_reach_the_service() {
    let types = ObjectTypes::new();
    let objects = Arc::new(FakeObjects::default());
    let Some((instance, _folders)) = recipes("recipes", &TYPES_AND_VIEWS, &types, &objects) else {
        return;
    };
    instance.register_object_types().unwrap();

    for bad in [
        "../outside.md",
        "/etc/soup.md",
        "recipes\\Soup.md",
        "recipes/../../x.md",
    ] {
        assert_eq!(
            call(&instance, &format!("create|recipe|Soup|{bad}|{{}}|Body")),
            [format!("error\tinvalid-path\t{bad}")]
        );
        assert_eq!(
            call(&instance, &format!("move|{RECIPE_ID}|rev-1|{bad}")),
            [format!("error\tinvalid-path\t{bad}")]
        );
    }
    assert_eq!(
        call(&instance, "create|recipe|Soup|-|[1, 2]|Body"),
        ["error\tfailed\tProperties must be a JSON object."]
    );
    assert!(objects.calls().is_empty(), "{:?}", objects.calls());
}

#[test]
fn write_errors_reach_the_plugin_as_structured_errors() {
    let types = ObjectTypes::new();
    let cases = [
        (
            ServiceError::RevisionConflict(RECIPE_ID.into()),
            format!("error\trevision-conflict\t{RECIPE_ID}"),
        ),
        (
            ServiceError::IdentityConflict(RECIPE_ID.into()),
            format!("error\tidentity-conflict\t{RECIPE_ID}"),
        ),
        (
            ServiceError::InvalidProperty(PropertyProblem {
                property: "course".into(),
                code: "not_allowed".into(),
            }),
            "error\tinvalid-property\tcourse:not_allowed".to_owned(),
        ),
        (
            ServiceError::AlreadyExists("recipes/Soup.md".into()),
            "error\talready-exists\trecipes/Soup.md".to_owned(),
        ),
    ];
    for (refusal, expected) in cases {
        let objects = Arc::new(FakeObjects {
            refuse: Some(refusal),
            ..FakeObjects::default()
        });
        let Some((instance, _folders)) = recipes("recipes", &TYPES_AND_VIEWS, &types, &objects)
        else {
            return;
        };
        instance.register_object_types().unwrap();

        assert_eq!(
            call(
                &instance,
                &format!(r#"update|{RECIPE_ID}|rev-1|{{"course":"x"}}"#)
            ),
            [expected]
        );
    }
}

#[test]
fn the_properties_panel_draws_from_the_objects_file_and_its_button_writes_with_the_revision() {
    let types = ObjectTypes::new();
    let objects = Arc::new(FakeObjects::default());
    objects.objects.lock().unwrap().push(recipe());
    let Some((instance, _folders)) = recipes("recipes", &TYPES_AND_VIEWS, &types, &objects) else {
        return;
    };
    instance.register_object_types().unwrap();

    let nodes = instance.render_panel(RECIPE_ID).unwrap();
    assert_eq!(
        plugins::texts(&nodes),
        [
            "Course: brunch",
            "Servings: null",
            "Vegetarian: no",
            "course doesn't fit: not_allowed"
        ]
    );
    instance
        .handle_panel_event(RECIPE_ID, &Event::Pressed("vegetarian".into()))
        .unwrap();

    assert_eq!(
        objects.calls(),
        [
            format!("get {RECIPE_ID}"),
            format!("get {RECIPE_ID}"),
            format!(r#"update {RECIPE_ID} rev-1 {{"vegetarian":true}}"#),
        ]
    );
}
