//! The generic managed-object envelope and registered object types, checked
//! against the shared fixture in
//! `docs/workspace-format/fixtures/object-envelope-v1.json`.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use workspace_format::{
    ObjectTypeRegistry, ParsedMarkdown, PropertySchema, WorkspaceObject, parse_markdown,
    serialize_object,
};

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../docs/workspace-format/fixtures/object-envelope-v1.json"
    ))
    .unwrap()
}

fn cases<'a>(fixture: &'a Value, section: &str) -> &'a [Value] {
    fixture[section]
        .as_array()
        .unwrap_or_else(|| panic!("the fixture has no {section} section"))
}

fn name(case: &Value) -> &str {
    case["name"].as_str().unwrap()
}

fn managed(case: &Value) -> WorkspaceObject {
    let markdown = case["markdown"].as_str().unwrap();
    match parse_markdown("objects/example.md", markdown.as_bytes()) {
        ParsedMarkdown::Managed(object) => object,
        other => panic!("{}: expected a managed object, got {other:?}", name(case)),
    }
}

fn envelope_fields(object: &WorkspaceObject) -> Value {
    json!({
        "id": object.id,
        "type": object.object_type,
        "title": object.title,
        "body": object.body,
        "created": object.created,
        "updated": object.updated,
        "properties": object.properties,
    })
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap()
}

#[test]
fn any_managed_object_parses_and_serializes_through_the_envelope() {
    let fixture = fixture();
    let types = ObjectTypeRegistry::new();
    let registered = registry(&fixture);
    for case in cases(&fixture, "envelope") {
        let object = managed(case);
        assert_eq!(envelope_fields(&object), case["object"], "{}", name(case));
        assert_canonical(
            &types,
            &object,
            case["canonical"].as_str().unwrap(),
            name(case),
        );
        // A registered schema changes the order, not the title and body.
        let canonical = text(registered.serialize(&object).unwrap());
        assert_canonical(&registered, &object, &canonical, name(case));
    }
}

fn schema(value: &Value) -> PropertySchema {
    serde_json::from_value(value.clone()).unwrap()
}

#[test]
fn schema_declarations_decode_as_the_fixture_says() {
    let fixture = fixture();
    for case in cases(&fixture, "schema_declaration") {
        let decoded = serde_json::from_value::<PropertySchema>(case["schema"].clone());
        assert_eq!(
            decoded.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}: {decoded:?}",
            name(case)
        );
    }
}

#[test]
fn registration_accepts_or_rejects_with_a_structured_error() {
    let fixture = fixture();
    for case in cases(&fixture, "registration") {
        let mut types = ObjectTypeRegistry::new();
        let steps = case["register"].as_array().unwrap();
        let (last, earlier) = steps.split_last().unwrap();
        for step in earlier {
            types
                .register(step["type"].as_str().unwrap(), schema(&step["schema"]))
                .unwrap_or_else(|error| panic!("{}: {error}", name(case)));
        }
        let result = types.register(last["type"].as_str().unwrap(), schema(&last["schema"]));
        let error = result
            .err()
            .map(|error| serde_json::to_value(error).unwrap());
        assert_eq!(
            error.unwrap_or(Value::Null),
            case["error"],
            "{}",
            name(case)
        );
    }
}

/// A registry holding every schema in the fixture's `schemas` section.
fn registry(fixture: &Value) -> ObjectTypeRegistry {
    let mut types = ObjectTypeRegistry::new();
    for (object_type, declaration) in fixture["schemas"].as_object().unwrap() {
        types.register(object_type, schema(declaration)).unwrap();
    }
    types
}

/// The registry a case names: `schemas` holds every schema in the fixture,
/// and `empty` holds none, as when every plugin is off.
fn registry_for(fixture: &Value, case: &Value) -> ObjectTypeRegistry {
    match case["registry"].as_str().unwrap() {
        "schemas" => registry(fixture),
        "empty" => ObjectTypeRegistry::new(),
        other => panic!("{}: unknown registry {other}", name(case)),
    }
}

fn object_with(object_type: &str, properties: &Value) -> WorkspaceObject {
    WorkspaceObject {
        id: format!("{object_type}_01j00000000000000000000000"),
        object_type: object_type.into(),
        title: "Example".into(),
        body: String::new(),
        relative_path: "objects/example.md".into(),
        revision: String::new(),
        created: Some("2026-08-27T18:00:00Z".into()),
        updated: Some("2026-08-27T18:00:00Z".into()),
        properties: serde_json::from_value(properties.clone()).unwrap(),
    }
}

#[test]
fn validation_applies_defaults_and_names_the_failing_property() {
    let fixture = fixture();
    let types = registry(&fixture);
    for case in cases(&fixture, "validation") {
        let mut object = object_with(case["type"].as_str().unwrap(), &case["properties"]);
        match types.normalize(&mut object) {
            Ok(()) => assert_eq!(json!(object.properties), case["expected"], "{}", name(case)),
            Err(error) => {
                assert_eq!(
                    serde_json::to_value(error).unwrap(),
                    case["error"],
                    "{}",
                    name(case)
                );
                assert_eq!(
                    json!(object.properties),
                    case["properties"],
                    "{}: a rejected object stays unchanged",
                    name(case)
                );
            }
        }
    }
}

fn conformance_v1() -> Value {
    serde_json::from_str(include_str!(
        "../../../docs/workspace-format/fixtures/conformance-v1.json"
    ))
    .unwrap()
}

/// Serializes, parses and serializes again, and requires the same bytes.
fn assert_canonical(
    types: &ObjectTypeRegistry,
    object: &WorkspaceObject,
    expected: &str,
    case: &str,
) {
    let canonical = text(types.serialize(object).unwrap());
    assert_eq!(canonical, expected, "{case}");
    let ParsedMarkdown::Managed(reparsed) =
        parse_markdown(&object.relative_path, canonical.as_bytes())
    else {
        panic!("{case}: canonical bytes must stay managed");
    };
    assert_eq!(
        text(types.serialize(&reparsed).unwrap()),
        canonical,
        "{case}"
    );
}

#[test]
fn registered_types_serialize_declared_properties_in_declared_order() {
    let fixture = fixture();
    let types = registry(&fixture);
    for case in cases(&fixture, "registered_serialization") {
        assert_canonical(
            &types,
            &managed(case),
            case["canonical"].as_str().unwrap(),
            name(case),
        );
    }
}

#[test]
fn saving_over_a_registered_types_file_rewrites_changed_properties_in_declared_order() {
    let fixture = fixture();
    let types = registry(&fixture);
    for case in cases(&fixture, "registered_rewrites") {
        let existing = case["markdown"].as_str().unwrap();
        let mut object = managed(case);
        for (key, value) in case["set"].as_object().unwrap() {
            object.properties.insert(key.clone(), value.clone());
        }
        let written = types.serialize_over(existing.as_bytes(), &object).unwrap();
        assert_eq!(
            text(written),
            case["expected"].as_str().unwrap(),
            "{}",
            name(case)
        );
    }
}

#[test]
fn registered_types_keep_todays_body_rules() {
    let fixture = fixture();
    let types = registry(&fixture);
    let conformance = conformance_v1();
    for case in cases(&conformance, "object_body") {
        let mut object = object_with("recipe", &json!({}));
        object.body = case["body"].as_str().unwrap().into();
        let bytes = text(types.serialize(&object).unwrap());
        assert!(
            bytes.ends_with(case["file_ends_with"].as_str().unwrap()),
            "{}",
            name(case)
        );
        let ParsedMarkdown::Managed(parsed) =
            parse_markdown(&object.relative_path, bytes.as_bytes())
        else {
            panic!("{}: managed object expected", name(case));
        };
        assert_eq!(
            parsed.body,
            case["expected_body"].as_str().unwrap(),
            "{}",
            name(case)
        );
    }
}

/// The serializer writes a body a caller passes in as it is, apart from its
/// final line breaks. The next read applies the parser's line break rules,
/// and from then on the bytes stay the same. An empty registry writes
/// `serialize_object`'s bytes, and the fixture's registry has a recipe
/// schema.
#[test]
fn caller_bodies_settle_after_one_read() {
    let fixture = fixture();
    let registries = [
        ("no schema", ObjectTypeRegistry::new()),
        ("recipe schema", registry(&fixture)),
    ];
    for case in cases(&fixture, "caller_bodies") {
        for (registry, types) in &registries {
            let label = format!("{} ({registry})", name(case));
            let mut object = object_with("recipe", &json!({}));
            object.body = case["body"].as_str().unwrap().into();
            let written = text(types.serialize(&object).unwrap());
            assert!(
                written.ends_with(case["file_ends_with"].as_str().unwrap()),
                "{label}: {written:?}"
            );
            let ParsedMarkdown::Managed(parsed) =
                parse_markdown(&object.relative_path, written.as_bytes())
            else {
                panic!("{label}: managed object expected");
            };
            assert_eq!(
                parsed.body,
                case["expected_body"].as_str().unwrap(),
                "{label}"
            );
            let rewritten = text(types.serialize(&parsed).unwrap());
            assert!(
                rewritten.ends_with(case["rewritten_ends_with"].as_str().unwrap()),
                "{label}: {rewritten:?}"
            );
            assert_canonical(types, &parsed, &rewritten, &label);
        }
    }
}

#[test]
fn an_unregistered_type_stays_a_managed_object_with_every_property() {
    let fixture = fixture();
    for case in cases(&fixture, "unregistered") {
        let types = registry_for(&fixture, case);
        let mut object = managed(case);
        assert_eq!(
            json!(object.properties),
            case["properties"],
            "{}",
            name(case)
        );
        types.normalize(&mut object).unwrap();
        assert_eq!(
            json!(object.properties),
            case["properties"],
            "{}",
            name(case)
        );
        assert_canonical(
            &types,
            &object,
            case["canonical"].as_str().unwrap(),
            name(case),
        );
        assert_eq!(
            text(serialize_object(&object).unwrap()),
            case["canonical"].as_str().unwrap(),
            "{}: serialize_object writes the same order",
            name(case)
        );
    }
}

/// Keys and values that hold U+2028 or U+2029 keep the form libyaml gives
/// them and read back the same. The one exception is a last value that
/// libyaml writes as a literal block ending in one: it's double-quoted.
#[test]
fn line_separators_keep_libyamls_form_unless_a_literal_block_ends_the_frontmatter() {
    let fixture = fixture();
    for case in cases(&fixture, "line_separators") {
        let types = registry_for(&fixture, case);
        let object = managed(case);
        let expected = (case["properties"].clone(), case["updated"].clone());
        assert_eq!(
            (json!(object.properties), json!(object.updated)),
            expected,
            "{}",
            name(case)
        );
        assert_canonical(
            &types,
            &object,
            case["canonical"].as_str().unwrap(),
            name(case),
        );
        let reparsed = managed(&json!({
            "name": name(case),
            "markdown": case["canonical"],
        }));
        assert_eq!(
            (json!(reparsed.properties), json!(reparsed.updated)),
            expected,
            "{}: canonical bytes read back the same values",
            name(case)
        );
    }
}

#[test]
fn malformed_input_stays_malformed_whether_or_not_its_type_is_registered() {
    let fixture = fixture();
    for case in cases(&fixture, "malformed") {
        let bytes: Vec<u8> = match case.get("bytes") {
            Some(bytes) => serde_json::from_value(bytes.clone()).unwrap(),
            None => case["markdown"].as_str().unwrap().as_bytes().to_vec(),
        };
        match parse_markdown("objects/example.md", &bytes) {
            ParsedMarkdown::Malformed { error, .. } => assert!(
                error.starts_with(case["error_starts_with"].as_str().unwrap()),
                "{}: {error}",
                name(case)
            ),
            other => panic!("{}: expected malformed, got {other:?}", name(case)),
        }
    }
}

#[test]
fn the_format_spec_schema_example_registers() {
    let spec = include_str!("../../../docs/workspace-format/v1.md");
    let section = &spec[spec.find("### Schema declaration").unwrap()..];
    let start = section.find("```json\n").unwrap() + "```json\n".len();
    let end = start + section[start..].find("```").unwrap();
    let example: PropertySchema = serde_json::from_str(&section[start..end]).unwrap();
    ObjectTypeRegistry::new().register("task", example).unwrap();
}

#[test]
fn the_format_spec_string_example_is_what_the_serializer_writes() {
    let spec = include_str!("../../../docs/workspace-format/v1.md");
    let section = &spec[spec.find("### Envelope").unwrap()..];
    let start = section.find("```yaml\n").unwrap() + "```yaml\n".len();
    let end = start + section[start..].find("```").unwrap();
    let mut object = object_with("note", &json!({ "steps": "Boil.\nServe.\u{2028}" }));
    object.created = None;
    object.updated = None;
    let written = text(serialize_object(&object).unwrap());
    assert!(written.contains(&section[start..end]), "{written}");
}

/// A large frontmatter full of line separators, whose last value libyaml
/// writes as a literal block ending in one. Serializing it must stay linear
/// in its size: the serializer writes the mapping at most twice.
#[test]
fn serializing_line_separators_stays_linear_in_the_frontmatter_size() {
    let count = 5_000;
    let keys = (0..count)
        .map(|index| (format!("k{index}\u{2028}"), Value::Null))
        .collect::<serde_json::Map<_, _>>();
    let mut object = object_with(
        "note",
        &json!({
            "b": vec!["\u{2028}"; count],
            "c": keys,
        }),
    );
    object.created = None;
    object.updated = Some("x\n\u{2029}".into());
    let started = Instant::now();
    let bytes = serialize_object(&object).unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "serializing took {elapsed:?}"
    );
    let json = serde_json::to_string(&object.properties).unwrap().len();
    assert!(
        bytes.len() < 2 * json,
        "{} bytes of Markdown for {json} bytes of JSON properties",
        bytes.len()
    );
    let ParsedMarkdown::Managed(parsed) = parse_markdown(&object.relative_path, &bytes) else {
        panic!("the canonical bytes must stay managed");
    };
    assert_eq!(parsed.properties, object.properties);
    assert_eq!(parsed.updated, object.updated);
}
