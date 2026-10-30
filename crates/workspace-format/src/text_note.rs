//! A Markdown note's parts in text mode (sync-v2.md §22): its properties as
//! RFC 8785 canonical JSON and its body, and the file bytes those parts
//! give. Every device writes a text-mode note's file from them, so the rules
//! here decide which bytes all of them agree on.
//!
//! A managed note's file is the frontmatter this crate's serializer writes
//! from its properties, followed by the body. An unmanaged note keeps its
//! whole file, frontmatter included, in its body, and has no properties. A
//! byte-order mark and line endings stay in the body they belong to.

use std::collections::BTreeMap;

use serde_json::Value;
use serde_yaml_ng::Value as Yaml;

use crate::{FormatError, ParsedMarkdown, frontmatter_block, parse_markdown};

/// The longest property name, in bytes of UTF-8.
pub const MAX_PROPERTY_NAME: usize = 256;
/// Integers at or above this magnitude don't map to JSON (§22).
const INTEGER_LIMIT: u64 = 1 << 53;

/// What text mode makes of a note's file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteParts {
    /// A managed note: each property's canonical JSON, by name, and the
    /// bytes after the frontmatter.
    Managed {
        /// The frontmatter's properties, `id` and `type` included.
        properties: BTreeMap<String, String>,
        /// The bytes after the frontmatter, as UTF-8.
        body: String,
    },
    /// An ordinary Markdown file, all of it the body.
    Unmanaged {
        /// The whole file, byte-order mark and line endings kept.
        body: String,
    },
    /// A file that stays out of text mode: it isn't UTF-8, or its
    /// frontmatter doesn't parse or falls outside the property rules.
    Outside,
}

/// Splits a note's file as text mode reads it.
pub fn note_parts(path: &str, bytes: &[u8]) -> NoteParts {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return NoteParts::Outside;
    };
    match parse_markdown(path, bytes) {
        ParsedMarkdown::Managed(_) => match managed_parts(text) {
            Some((properties, body)) => NoteParts::Managed {
                properties,
                body: body.to_owned(),
            },
            None => NoteParts::Outside,
        },
        ParsedMarkdown::Unmanaged { .. } => NoteParts::Unmanaged {
            body: text.to_owned(),
        },
        ParsedMarkdown::Malformed { .. } => NoteParts::Outside,
    }
}

/// The properties and the raw body of a managed note, or `None` when its
/// frontmatter falls outside the property rules.
fn managed_parts(text: &str) -> Option<(BTreeMap<String, String>, &str)> {
    let (yaml, body) = split_frontmatter(text)?;
    if names_a_node(&yaml) {
        return None;
    }
    let Yaml::Mapping(mapping) = serde_yaml_ng::from_str::<Yaml>(&yaml).ok()? else {
        return None;
    };
    let mut properties = BTreeMap::new();
    for (key, value) in mapping {
        let Yaml::String(name) = key else {
            return None;
        };
        if !valid_property_name(&name) {
            return None;
        }
        properties.insert(name, canonical_json(&json_value(&value)?));
    }
    Some((properties, body))
}

/// The frontmatter's YAML with LF line endings and the raw text after its
/// closing `---` line.
fn split_frontmatter(text: &str) -> Option<(String, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let mut yaml = String::new();
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        at += line.len();
        let content = line.strip_suffix('\n')?;
        let content = content.strip_suffix('\r').unwrap_or(content);
        if content == "---" {
            return Some((yaml, &rest[at..]));
        }
        yaml.push_str(content);
        yaml.push('\n');
    }
    None
}

/// Whether the YAML names an anchor, an alias or a tag at the start of a
/// node, outside quotes. serde_yaml_ng resolves them on the way, so the
/// text is the only place that shows them. A plain scalar with a word that
/// starts with one of these marks counts too, so such a note stays out of
/// text mode on every device.
fn names_a_node(yaml: &str) -> bool {
    for line in yaml.lines() {
        let mut quote = None;
        let mut before = ' ';
        for character in line.chars() {
            match quote {
                Some(open) if character == open => quote = None,
                Some(_) => {}
                None => {
                    if (character == '"' || character == '\'') && before.is_whitespace() {
                        quote = Some(character);
                    } else if matches!(character, '&' | '*' | '!')
                        && (before.is_whitespace() || matches!(before, '[' | '{' | ','))
                    {
                        return true;
                    } else if character == '#' && before.is_whitespace() {
                        break;
                    }
                }
            }
            before = character;
        }
    }
    false
}

/// Whether `name` passes §22's rule: 1 to 256 bytes of UTF-8 with no control
/// character.
pub fn valid_property_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PROPERTY_NAME
        && !name.chars().any(|character| character.is_control())
}

/// `value` under the YAML 1.2 core schema's mapping to JSON, or `None` when
/// it has none.
fn json_value(value: &Yaml) -> Option<Value> {
    Some(match value {
        Yaml::Null => Value::Null,
        Yaml::Bool(value) => Value::Bool(*value),
        Yaml::Number(number) => {
            if let Some(integer) = number.as_u64() {
                if integer >= INTEGER_LIMIT {
                    return None;
                }
                Value::from(integer)
            } else if let Some(integer) = number.as_i64() {
                if integer.unsigned_abs() >= INTEGER_LIMIT {
                    return None;
                }
                Value::from(integer)
            } else {
                let float = number.as_f64()?;
                serde_json::Number::from_f64(float).map(Value::Number)?
            }
        }
        Yaml::String(text) => Value::String(text.clone()),
        Yaml::Sequence(items) => {
            Value::Array(items.iter().map(json_value).collect::<Option<Vec<_>>>()?)
        }
        Yaml::Mapping(entries) => {
            let mut object = serde_json::Map::new();
            for (key, value) in entries {
                let Yaml::String(key) = key else {
                    return None;
                };
                object.insert(key.clone(), json_value(value)?);
            }
            Value::Object(object)
        }
        Yaml::Tagged(_) => return None,
    })
}

/// `value` as RFC 8785 canonical JSON.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null | Value::Bool(_) => out.push_str(&value.to_string()),
        Value::String(text) => write_string(text, out),
        Value::Number(number) => out.push_str(&canonical_number(number)),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(entries) => {
            let mut keys: Vec<&String> = entries.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_canonical(&entries[*key], out);
            }
            out.push('}');
        }
    }
}

/// A string as RFC 8785 writes it: the two-letter escapes, `\u00xx` below
/// U+0020, and everything else as itself.
fn write_string(text: &str, out: &mut String) {
    use std::fmt::Write as _;

    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            control if control < ' ' => {
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// A number as ECMAScript's `Number.prototype.toString` writes it, which
/// RFC 8785 takes.
fn canonical_number(number: &serde_json::Number) -> String {
    if let Some(integer) = number.as_i64() {
        return integer.to_string();
    }
    if let Some(integer) = number.as_u64() {
        return integer.to_string();
    }
    let value = number.as_f64().unwrap_or(0.0);
    if value == 0.0 {
        return "0".into();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    // The shortest digits that read back as this number.
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("the exponent format holds an e");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let exponent: i32 = exponent.parse().expect("an exponent");
    let point = exponent + 1;
    let count = digits.len() as i32;
    let body = if (count..=21).contains(&point) {
        format!("{digits}{}", "0".repeat((point - count) as usize))
    } else if (1..=21).contains(&point) {
        format!(
            "{}.{}",
            &digits[..point as usize],
            &digits[point as usize..]
        )
    } else if (-5..=0).contains(&point) {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else {
        let sign = if point > 0 { '+' } else { '-' };
        let shown = (point - 1).abs();
        match digits.split_at(1) {
            (first, "") => format!("{first}e{sign}{shown}"),
            (first, rest) => format!("{first}.{rest}e{sign}{shown}"),
        }
    };
    format!("{sign}{body}")
}

/// The file a text-mode note's merged state gives: the frontmatter of
/// `properties`, or none for an unmanaged note, followed by `body`.
pub fn compose_note(
    properties: &BTreeMap<String, String>,
    body: &str,
) -> Result<String, FormatError> {
    if properties.is_empty() {
        return Ok(body.to_owned());
    }
    let mut values: Vec<(&str, Value)> = Vec::with_capacity(properties.len());
    for (name, json) in properties {
        let value = serde_json::from_str(json).map_err(|_| FormatError::ObjectSerialization)?;
        values.push((name, value));
    }
    // `id` and `type`, the rest by name, then `created` and `updated`, as
    // the managed-object envelope writes them.
    let rank = |name: &str| match name {
        "id" => 0,
        "type" => 1,
        "created" => 3,
        "updated" => 4,
        _ => 2,
    };
    values.sort_by(|a, b| rank(a.0).cmp(&rank(b.0)).then(a.0.cmp(b.0)));
    Ok(format!("{}{body}", frontmatter_block(&values)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "note_01j9z3y7q8r2s4t6v8w0x2y4z6";

    fn managed(extra: &str, body: &str) -> Vec<u8> {
        format!("---\nid: {ID}\ntype: note\n{extra}---\n{body}").into_bytes()
    }

    fn parts(bytes: &[u8]) -> NoteParts {
        note_parts("Plan.md", bytes)
    }

    fn properties(bytes: &[u8]) -> BTreeMap<String, String> {
        match parts(bytes) {
            NoteParts::Managed { properties, .. } => properties,
            other => panic!("a managed note, got {other:?}"),
        }
    }

    #[test]
    fn a_managed_note_splits_into_canonical_properties_and_its_body() {
        let file = managed(
            "tags: [a, b]\nsize: 3\nratio: 0.5\nopen: true\n",
            "# Plan\n",
        );
        let NoteParts::Managed { properties, body } = parts(&file) else {
            panic!("a managed note");
        };
        assert_eq!(body, "# Plan\n");
        assert_eq!(properties["id"], format!("\"{ID}\""));
        assert_eq!(properties["tags"], r#"["a","b"]"#);
        assert_eq!(properties["size"], "3");
        assert_eq!(properties["ratio"], "0.5");
        assert_eq!(properties["open"], "true");
    }

    #[test]
    fn a_body_keeps_its_line_endings() {
        let file = format!("---\r\nid: {ID}\r\ntype: note\r\n---\r\nOne\r\nTwo\r\n");
        let NoteParts::Managed { body, .. } = parts(file.as_bytes()) else {
            panic!("a managed note");
        };
        assert_eq!(body, "One\r\nTwo\r\n");
    }

    #[test]
    fn an_unmanaged_note_keeps_its_whole_file() {
        let file = "\u{feff}---\r\ntitle: x\r\n---\r\nBody\r\n";
        assert_eq!(
            parts(file.as_bytes()),
            NoteParts::Unmanaged {
                body: file.to_owned()
            }
        );
    }

    #[test]
    fn frontmatter_outside_the_rules_stays_out_of_text_mode() {
        for extra in [
            "big: 9007199254740992\n",
            "negative: -9007199254740992\n",
            "tagged: !secret value\n",
            "shared: &anchor value\nagain: *anchor\n",
            "{bad key}: 1\n",
            "nan: .nan\n",
            "infinite: .inf\n",
            "map: {1: a}\n",
        ] {
            assert_eq!(parts(&managed(extra, "x")), NoteParts::Outside, "{extra}");
        }
        let long = format!("{}: 1\n", "k".repeat(257));
        assert_eq!(parts(&managed(&long, "x")), NoteParts::Outside);
        let fits = format!("{}: 1\n", "k".repeat(256));
        assert!(matches!(
            parts(&managed(&fits, "x")),
            NoteParts::Managed { .. }
        ));
        assert_eq!(parts(b"\xff\xfe"), NoteParts::Outside);
        assert_eq!(parts(b"---\nid: x\n"), NoteParts::Outside);
    }

    #[test]
    fn dates_and_the_largest_integers_map_to_json() {
        let file = managed(
            "when: 2026-10-30\nmost: 9007199254740991\nleast: -9007199254740991\n",
            "",
        );
        let found = properties(&file);
        assert_eq!(found["when"], r#""2026-10-30""#);
        assert_eq!(found["most"], "9007199254740991");
        assert_eq!(found["least"], "-9007199254740991");
    }

    #[test]
    fn composing_writes_the_serializers_frontmatter_then_the_body() {
        let file = managed("zeta: 1\nalpha: two\n", "Body\r\n");
        let found = properties(&file);
        let composed = compose_note(&found, "Body\r\n").unwrap();
        assert_eq!(
            composed,
            format!("---\nid: {ID}\ntype: note\nalpha: two\nzeta: 1\n---\nBody\r\n")
        );
        assert_eq!(properties(composed.as_bytes()), found);
        assert_eq!(compose_note(&BTreeMap::new(), "plain").unwrap(), "plain");
    }

    #[test]
    fn numbers_follow_rfc_8785() {
        for (number, text) in [
            (1.0, "1"),
            (0.1, "0.1"),
            (1e21, "1e+21"),
            (1.5e21, "1.5e+21"),
            (1e20, "100000000000000000000"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (-2.5e-7, "-2.5e-7"),
            (123.456, "123.456"),
        ] {
            let number = serde_json::Number::from_f64(number).unwrap();
            assert_eq!(canonical_number(&number), text);
        }
    }

    #[test]
    fn strings_and_keys_follow_rfc_8785() {
        let value =
            serde_json::json!({"b": "é\n\u{1}", "a": [null, true], "\u{1f600}": 1, "\u{e000}": 2});
        assert_eq!(
            canonical_json(&value),
            "{\"a\":[null,true],\"b\":\"é\\n\\u0001\",\"\u{1f600}\":1,\"\u{e000}\":2}"
        );
    }
}
