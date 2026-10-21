//! Objects of registered types: writes pass the type's schema in
//! `workspace-format` and reach disk durably before they return, and reads
//! come from the files, whatever the index says.

use std::collections::BTreeMap;

use local_core::{CreateObjectInput, ObjectPatch, WorkspaceEngine};
use serde_json::{Value, json};
use tempfile::tempdir;
use workspace_format::{ObjectTypeRegistry, PropertySchema};

fn engine() -> (tempfile::TempDir, tempfile::TempDir, WorkspaceEngine) {
    let workspace = tempdir().unwrap();
    let app_data = tempdir().unwrap();
    let engine =
        WorkspaceEngine::create_with_app_data(workspace.path(), "Typed objects", app_data.path())
            .unwrap();
    (workspace, app_data, engine)
}

fn schemas() -> ObjectTypeRegistry {
    let schema: PropertySchema = serde_json::from_value(json!({
        "properties": [
            { "name": "course", "kind": { "enum": ["starter", "main", "dessert"] }, "default": "main" },
            { "name": "servings", "kind": "number", "default": 2 },
            { "name": "cooked", "kind": "date", "nullable": true }
        ]
    }))
    .unwrap();
    let mut schemas = ObjectTypeRegistry::new();
    schemas.register("recipe", schema).unwrap();
    schemas
}

fn properties(value: Value) -> BTreeMap<String, Value> {
    serde_json::from_value(value).unwrap()
}

fn recipe(title: &str, value: Value) -> CreateObjectInput {
    CreateObjectInput {
        object_type: "recipe".into(),
        title: title.into(),
        body: "Simmer.\n".into(),
        relative_path: None,
        properties: properties(value),
    }
}

fn patch(revision: &str, value: Value) -> ObjectPatch {
    ObjectPatch {
        title: None,
        body: None,
        properties: properties(value),
        remove_properties: Vec::new(),
        expected_revision: revision.into(),
    }
}

fn read(workspace: &tempfile::TempDir, relative: &str) -> String {
    std::fs::read_to_string(workspace.path().join(relative)).unwrap()
}

#[test]
fn a_created_object_takes_its_types_prefix_defaults_and_declared_order() {
    let (workspace, _app_data, engine) = engine();

    let created = engine
        .create_object_checked(
            recipe("Soup", json!({"zeta": 1, "servings": 4})),
            &schemas(),
        )
        .unwrap();

    assert!(
        created.value.id.starts_with("recipe_"),
        "{}",
        created.value.id
    );
    assert_eq!(created.value.relative_path, "recipes/Soup.md");
    let text = read(&workspace, "recipes/Soup.md");
    let expected_start = format!(
        "---\nid: {}\ntype: recipe\ncourse: main\nservings: 4\nzeta: 1\ncreated: ",
        created.value.id
    );
    assert!(text.starts_with(&expected_start), "{text}");
    assert!(text.ends_with("---\nSimmer.\n"), "{text}");
    assert_eq!(
        created.revision,
        workspace_format::revision(text.as_bytes())
    );
}

#[test]
fn properties_that_fail_the_schema_name_the_field_and_write_nothing() {
    let (workspace, _app_data, engine) = engine();
    let mut events = engine.subscribe();

    let error = engine
        .create_object_checked(recipe("Soup", json!({"course": "breakfast"})), &schemas())
        .unwrap_err();

    assert_eq!(error.code, "invalid_property");
    assert_eq!(
        error.details,
        Some(json!({"property": "course", "code": "not_allowed"}))
    );
    assert!(!workspace.path().join("recipes").exists());
    assert!(events.try_recv().is_err());
    assert!(engine.list_objects("recipe").unwrap().is_empty());
}

#[test]
fn an_update_passes_the_schema_and_keeps_unknown_properties_and_the_body() {
    let (workspace, _app_data, engine) = engine();
    let created = engine
        .create_object_checked(recipe("Soup", json!({"zeta": "kept"})), &schemas())
        .unwrap();
    let before = read(&workspace, "recipes/Soup.md");

    let error = engine
        .update_object_checked(
            &created.value.id,
            patch(&created.revision, json!({"servings": "many"})),
            &schemas(),
        )
        .unwrap_err();
    assert_eq!(error.code, "invalid_property");
    assert_eq!(
        error.details,
        Some(json!({"property": "servings", "code": "wrong_type"}))
    );
    assert_eq!(read(&workspace, "recipes/Soup.md"), before);

    let updated = engine
        .update_object_checked(
            &created.value.id,
            patch(&created.revision, json!({"course": "dessert"})),
            &schemas(),
        )
        .unwrap();
    let text = read(&workspace, "recipes/Soup.md");
    assert!(
        text.contains("type: recipe\ncourse: dessert\nservings: 2\nzeta: kept\ncreated: "),
        "{text}"
    );
    assert!(text.ends_with("---\nSimmer.\n"), "{text}");
    assert_eq!(updated.value.properties["zeta"], json!("kept"));
    assert_eq!(
        updated.revision,
        workspace_format::revision(text.as_bytes())
    );
}

#[test]
fn an_update_with_an_old_revision_changes_nothing() {
    let (workspace, _app_data, engine) = engine();
    let created = engine
        .create_object_checked(recipe("Soup", json!({})), &schemas())
        .unwrap();
    let edited = read(&workspace, "recipes/Soup.md").replace("Simmer.", "Boil.");
    std::fs::write(workspace.path().join("recipes/Soup.md"), &edited).unwrap();

    let error = engine
        .update_object_checked(
            &created.value.id,
            patch(&created.revision, json!({"servings": 3})),
            &schemas(),
        )
        .unwrap_err();

    assert_eq!(error.code, "revision_conflict");
    assert_eq!(read(&workspace, "recipes/Soup.md"), edited);
}

#[test]
fn reads_come_from_the_files_after_an_outside_edit() {
    let (workspace, _app_data, engine) = engine();
    let created = engine
        .create_object_checked(recipe("Soup", json!({})), &schemas())
        .unwrap();
    let edited = read(&workspace, "recipes/Soup.md").replace("servings: 2", "servings: 9");
    std::fs::write(workspace.path().join("recipes/Soup.md"), &edited).unwrap();
    let revision = workspace_format::revision(edited.as_bytes());

    let object = engine.read_object(&created.value.id).unwrap();
    assert_eq!(object.revision, revision);
    assert_eq!(object.properties["servings"], json!(9));

    let listed = engine.list_objects("recipe").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].revision, revision);
    assert_eq!(listed[0].properties["servings"], json!(9));
    assert!(engine.list_objects("note").unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn a_write_that_cant_reach_disk_fails_and_neither_the_index_nor_an_event_reports_it() {
    use std::os::unix::fs::PermissionsExt;

    let (workspace, _app_data, engine) = engine();
    let created = engine
        .create_object_checked(recipe("Soup", json!({})), &schemas())
        .unwrap();
    let before = read(&workspace, "recipes/Soup.md");
    let folder = workspace.path().join("recipes");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let mut events = engine.subscribe();

    let update = engine.update_object_checked(
        &created.value.id,
        patch(&created.revision, json!({"servings": 3})),
        &schemas(),
    );
    let create = engine.create_object_checked(recipe("Stew", json!({})), &schemas());
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(update.is_err());
    assert!(create.is_err());
    assert!(events.try_recv().is_err(), "no event reports the writes");
    assert_eq!(read(&workspace, "recipes/Soup.md"), before);
    let listed = engine.list_objects("recipe").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].revision, created.revision);
    assert_eq!(
        engine
            .get_object(&created.value.id)
            .unwrap()
            .unwrap()
            .revision,
        created.revision
    );
}
