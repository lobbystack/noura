//! Canonical serialization is a fixed point: for any managed object, parsing
//! its bytes and serializing again gives the same bytes, whether or not its
//! type is registered.

use std::collections::BTreeMap;

use proptest::prelude::*;
use serde_json::Value;
use workspace_format::{
    ObjectTypeRegistry, ParsedMarkdown, PropertyDeclaration, PropertyKind, PropertySchema,
    WorkspaceObject, parse_markdown,
};

/// Any Unicode text, control characters and line breaks included.
fn any_text() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..24).prop_map(String::from_iter)
}

/// Property names: a small shared pool so declared and written names
/// overlap, plus arbitrary text.
fn property_name() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::sample::select(vec![
            "status", "due", "alpha", "zeta", "Émile", "id", "created", "a b", "",
        ])
        .prop_map(String::from),
        any_text(),
    ]
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        any::<f64>()
            .prop_filter_map("JSON numbers are finite", serde_json::Number::from_f64)
            .prop_map(Value::Number),
        any_text().prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map(any_text(), inner, 0..4)
                .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}

/// Distinct declared names that avoid the envelope's own fields, as
/// registration requires.
fn declared_names() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(property_name(), 0..6).prop_map(|names| {
        let mut seen = Vec::new();
        for name in names {
            let reserved = matches!(name.as_str(), "id" | "type" | "created" | "updated" | "");
            if !reserved && !seen.contains(&name) {
                seen.push(name);
            }
        }
        seen
    })
}

fn object_type() -> impl Strategy<Value = String> {
    "[a-z][a-z-]{0,12}"
}

#[derive(Debug)]
struct Case {
    registered: bool,
    declared: Vec<String>,
    object: WorkspaceObject,
}

fn case() -> impl Strategy<Value = Case> {
    (
        any::<bool>(),
        declared_names(),
        object_type(),
        any::<u128>(),
        any_text(),
        any_text(),
        prop::option::of(any_text()),
        prop::option::of(any_text()),
        prop::collection::btree_map(property_name(), json_value(), 0..8),
    )
        .prop_map(
            |(
                registered,
                declared,
                object_type,
                ulid,
                title,
                body,
                created,
                updated,
                properties,
            )| {
                let id = format!(
                    "{object_type}_{}",
                    ulid::Ulid::from(ulid).to_string().to_ascii_lowercase()
                );
                Case {
                    registered,
                    declared,
                    object: WorkspaceObject {
                        id,
                        object_type,
                        title,
                        body,
                        relative_path: "objects/example.md".into(),
                        revision: String::new(),
                        created,
                        updated,
                        properties: properties.into_iter().collect::<BTreeMap<_, _>>(),
                    },
                }
            },
        )
}

fn reparse(bytes: &[u8]) -> WorkspaceObject {
    match parse_markdown("objects/example.md", bytes) {
        ParsedMarkdown::Managed(object) => object,
        other => panic!(
            "canonical bytes must parse as managed, got {other:?} from {:?}",
            String::from_utf8_lossy(bytes)
        ),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn parsing_serializing_and_parsing_again_gives_identical_bytes(case in case()) {
        let mut types = ObjectTypeRegistry::new();
        if case.registered {
            let schema = PropertySchema {
                properties: case
                    .declared
                    .iter()
                    .map(|name| PropertyDeclaration {
                        name: name.clone(),
                        kind: PropertyKind::String,
                        nullable: false,
                        default: None,
                    })
                    .collect(),
            };
            types.register(&case.object.object_type, schema).unwrap();
        }
        let written = types.serialize(&case.object).unwrap();
        let parsed = reparse(&written);
        let mut expected = case.object.properties.clone();
        for key in ["id", "type", "created", "updated"] {
            expected.remove(key);
        }
        prop_assert_eq!(&parsed.properties, &expected);
        prop_assert_eq!(&parsed.created, &case.object.created);
        prop_assert_eq!(&parsed.updated, &case.object.updated);
        let first = types.serialize(&parsed).unwrap();
        let second = types.serialize(&reparse(&first)).unwrap();
        prop_assert_eq!(String::from_utf8(first).unwrap(), String::from_utf8(second).unwrap());
    }
}
