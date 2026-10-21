//! Serialization settles after one read: for any managed file, parsing it
//! and serializing the object gives canonical bytes, and parsing and
//! serializing those bytes again gives them unchanged, whether or not the
//! type is registered. The first test starts from an object a caller built,
//! the second from file bytes. Without a registered schema, the registry
//! writes the same bytes as `serialize_object`.

use std::collections::BTreeMap;

use proptest::prelude::*;
use serde_json::Value;
use workspace_format::{
    ObjectTypeRegistry, ParsedMarkdown, PropertyDeclaration, PropertyKind, PropertySchema,
    WorkspaceObject, parse_markdown, serialize_object,
};

/// Text that YAML reads in a special way: the line breaks of YAML 1.1 and
/// 1.2, the byte order mark, document markers, indicators, a merge key, and
/// plain scalars that resolve to null, booleans or numbers. `any::<char>()`
/// almost never produces most of them.
const YAML_SIGNIFICANT: &[&str] = &[
    "\n", "\r", "\r\n", "\t", " ", "\u{85}", "\u{2028}", "\u{2029}", "\u{feff}", "\u{e000}", "---",
    "...", "a\n", "<<", "null", "~", "yes", "No", "0x1f", "1e3", ".inf", "# ", ": ", "- ", "? ",
    "|", ">", "'", "\"", "\\", "&a", "*a", "!t", "%", "@", "`", "{", "[",
];

/// Any Unicode text, control characters and line breaks included, mixed
/// with pieces of text that YAML reads in a special way.
fn any_text() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            3 => any::<char>().prop_map(String::from),
            2 => prop::sample::select(YAML_SIGNIFICANT).prop_map(String::from),
        ],
        0..16,
    )
    .prop_map(|pieces| pieces.concat())
}

/// Property names: a small shared pool so declared and written names
/// overlap, plus arbitrary text.
fn property_name() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::sample::select(vec![
            "status",
            "due",
            "kanban_order",
            "start",
            "alpha",
            "zeta",
            "Émile",
            "id",
            "created",
            "a b",
            "",
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

/// Line breaks as files hold them, with runs of carriage returns.
const LINE_BREAKS: &[&str] = &["\n", "\r\n", "\r\r\n", "\r\r\r\n", "\r"];

/// Markdown after the frontmatter: any text, heavy on line breaks, carriage
/// returns and the text that starts a heading or closes frontmatter.
fn markdown_text() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            3 => any::<char>().prop_map(String::from),
            3 => prop::sample::select(LINE_BREAKS).prop_map(String::from),
            1 => prop::sample::select(&["# ", "#", " ", "\t", "---", "\u{85}", "\u{2028}"][..])
                .prop_map(String::from),
        ],
        0..24,
    )
    .prop_map(|pieces| pieces.concat())
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

/// A file whose frontmatter is the serializer's, with any text after it.
#[derive(Debug)]
struct FileCase {
    case: Case,
    bytes: String,
}

fn file_case() -> impl Strategy<Value = FileCase> {
    (
        case(),
        any::<bool>(),
        prop::collection::vec(prop::sample::select(LINE_BREAKS), 0..3),
        prop::option::of((any_text(), prop::sample::select(LINE_BREAKS))),
        markdown_text(),
    )
        .prop_map(|(case, crlf, gap, heading, markdown)| {
            let written = String::from_utf8(serialize_object(&case.object).unwrap()).unwrap();
            let end = written.find("\n---\n").unwrap() + "\n---\n".len();
            let mut bytes = written[..end].to_owned();
            if crlf {
                bytes = bytes.replace('\n', "\r\n");
            }
            bytes.push_str(&gap.concat());
            if let Some((title, line_break)) = heading {
                bytes.push_str(&format!("# {title}{line_break}"));
            }
            bytes.push_str(&markdown);
            FileCase { case, bytes }
        })
}

/// A registry that holds the case's schema when the case is registered.
fn registry(case: &Case) -> ObjectTypeRegistry {
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
    types
}

/// Whether a key at any depth holds a line separator. The serializer
/// writes keys as libyaml does, raw.
fn keys_hold_line_separators(properties: &BTreeMap<String, Value>) -> bool {
    fn nested(value: &Value) -> bool {
        match value {
            Value::Array(items) => items.iter().any(nested),
            Value::Object(map) => map
                .iter()
                .any(|(key, item)| key.contains(['\u{2028}', '\u{2029}']) || nested(item)),
            _ => false,
        }
    }
    properties
        .iter()
        .any(|(key, value)| key.contains(['\u{2028}', '\u{2029}']) || nested(value))
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

/// The YAML between the opening `---` line and the closing one.
fn frontmatter(bytes: &[u8]) -> &str {
    let text = std::str::from_utf8(bytes).unwrap();
    let end = text[4..].find("\n---\n").unwrap() + 4;
    &text[4..end]
}

/// 2048 cases, or the number `PROPTEST_CASES` sets.
fn config() -> ProptestConfig {
    if std::env::var_os("PROPTEST_CASES").is_some() {
        ProptestConfig::default()
    } else {
        ProptestConfig::with_cases(2048)
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn parsing_serializing_and_parsing_again_gives_identical_bytes(case in case()) {
        let types = registry(&case);
        let written = types.serialize(&case.object).unwrap();
        // Strings holding a YAML 1.1 line break other than LF are
        // double-quoted, so YAML 1.2 readers get the same text.
        let frontmatter = frontmatter(&written);
        prop_assert!(
            keys_hold_line_separators(&case.object.properties)
                || !frontmatter.contains(['\u{85}', '\u{2028}', '\u{2029}']),
            "a raw line separator in {:?}",
            frontmatter
        );
        if !case.registered {
            // Without a schema the registry writes the built-in order.
            prop_assert_eq!(&written, &serialize_object(&case.object).unwrap());
        }
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

    #[test]
    fn files_settle_after_one_read(file in file_case()) {
        let types = registry(&file.case);
        let parsed = reparse(file.bytes.as_bytes());
        let first = types.serialize(&parsed).unwrap();
        let reparsed = reparse(&first);
        prop_assert_eq!(&reparsed.properties, &parsed.properties);
        prop_assert_eq!(&reparsed.title, &parsed.title);
        let second = types.serialize(&reparsed).unwrap();
        prop_assert_eq!(String::from_utf8(first).unwrap(), String::from_utf8(second).unwrap());
    }
}
