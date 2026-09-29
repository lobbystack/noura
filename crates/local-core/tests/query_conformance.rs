//! The browser build answers the same read queries without SQLite. Both run
//! the shared fixture in `docs/workspace-format/fixtures/queries-v1.json`, so
//! the two implementations cannot drift apart unnoticed.

use local_core::{ObjectFilter, ObjectSummaryQuery, WorkspaceEngine};
use serde_json::{Value, json};
use tempfile::tempdir;

fn fixture() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/workspace-format/fixtures/queries-v1.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn ids<T: serde::Serialize>(values: &[T]) -> Vec<Value> {
    values
        .iter()
        .map(|value| serde_json::to_value(value).unwrap()["id"].clone())
        .collect()
}

#[test]
fn native_queries_match_the_shared_browser_fixture() {
    let fixture = fixture();
    let workspace = tempdir().unwrap();
    let app_data = tempdir().unwrap();
    let engine =
        WorkspaceEngine::create_with_app_data(workspace.path(), "Queries", app_data.path())
            .unwrap();
    for file in fixture["files"].as_array().unwrap() {
        let path = workspace.path().join(file["path"].as_str().unwrap());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, file["content"].as_str().unwrap()).unwrap();
    }
    engine.reconcile().unwrap();

    for case in fixture["objectQueries"].as_array().unwrap() {
        let filter: ObjectFilter = serde_json::from_value(case["query"].clone()).unwrap();
        let objects = engine.query_objects_filtered(&filter).unwrap();
        assert_eq!(
            Value::Array(ids(&objects)),
            case["expected"],
            "objects_query {}",
            case["query"]
        );
    }

    for case in fixture["summaryQueries"].as_array().unwrap() {
        let query: ObjectSummaryQuery = serde_json::from_value(case["query"].clone()).unwrap();
        let summaries = engine.query_object_summaries(&query).unwrap();
        assert_eq!(
            Value::Array(ids(&summaries)),
            case["expected"],
            "objects_summaries {}",
            case["query"]
        );
    }

    for case in fixture["calendarQueries"].as_array().unwrap() {
        let result = engine.calendar(
            case["start"].as_str().unwrap(),
            case["end"].as_str().unwrap(),
        );
        match case.get("error") {
            Some(code) => assert_eq!(json!(result.unwrap_err().code), *code),
            None => {
                let entries = result
                    .unwrap()
                    .iter()
                    .map(|entry| json!([entry.source_id, entry.property]))
                    .collect::<Vec<_>>();
                assert_eq!(Value::Array(entries), case["expected"], "calendar {case}");
            }
        }
    }

    let entries = engine
        .list_workspace_entries()
        .unwrap()
        .iter()
        .map(|entry| {
            let value = serde_json::to_value(entry).unwrap();
            json!([
                value["relativePath"],
                value["kind"],
                value["parseStatus"],
                value["objectId"]
            ])
        })
        .collect::<Vec<_>>();
    assert_eq!(Value::Array(entries), fixture["entries"]);

    let mut diagnostics = engine
        .state()
        .diagnostics
        .iter()
        .map(|value| json!([value.code, value.relative_path, value.object_id]))
        .collect::<Vec<_>>();
    diagnostics.sort_by_key(|value| value.to_string());
    assert_eq!(Value::Array(diagnostics), fixture["diagnostics"]);

    let duplicate = fixture["duplicateId"].as_str().unwrap();
    assert_eq!(
        engine.get_object(duplicate).unwrap_err().code,
        "identity_conflict"
    );
}
