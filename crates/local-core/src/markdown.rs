use std::{borrow::Cow, collections::BTreeMap};

use crate::{CoreError, ErrorCategory, Result, WorkspaceObject, valid_object_id};

#[derive(Debug, Clone, PartialEq)]
pub enum ParsedMarkdown {
    Managed(WorkspaceObject),
    Unmanaged {
        title: String,
        body: String,
        frontmatter: Option<BTreeMap<String, serde_json::Value>>,
    },
    Malformed {
        title: String,
        body: String,
        error: String,
    },
}

pub fn revision(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn split_title(content: &str) -> (String, String) {
    let mut lines = content.lines();
    match lines.next() {
        Some(first) if first.starts_with("# ") => {
            let title = first[2..].trim().to_owned();
            let body = lines
                .collect::<Vec<_>>()
                .join("\n")
                .trim_start_matches('\n')
                .to_owned();
            (title, body)
        }
        _ => (String::new(), content.to_owned()),
    }
}

pub fn parse_markdown(relative_path: &str, bytes: &[u8]) -> ParsedMarkdown {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return ParsedMarkdown::Malformed {
            title: String::new(),
            body: String::new(),
            error: "The file is not UTF-8".into(),
        };
    };
    let text = if text.contains("\r\n") {
        Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(text)
    };
    let text = text.as_ref();
    if !text.starts_with("---\n") {
        let (title, body) = split_title(text);
        return ParsedMarkdown::Unmanaged {
            title,
            body,
            frontmatter: None,
        };
    }
    let Some(end) = text[4..].find("\n---\n").map(|index| index + 4) else {
        let (title, body) = split_title(text);
        return ParsedMarkdown::Malformed {
            title,
            body,
            error: "Frontmatter has no closing delimiter".into(),
        };
    };
    let yaml = &text[4..end];
    let markdown = text[end + 5..].trim_start_matches('\n');
    let (title, body) = split_title(markdown);
    let properties: BTreeMap<String, serde_json::Value> = match serde_yaml_ng::from_str(yaml) {
        Ok(properties) => properties,
        Err(error) => {
            return ParsedMarkdown::Malformed {
                title,
                body,
                error: format!("Invalid YAML frontmatter: {error}"),
            };
        }
    };
    let Some(id) = properties
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
    else {
        return ParsedMarkdown::Unmanaged {
            title,
            body,
            frontmatter: Some(properties),
        };
    };
    let Some(object_type) = properties
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
    else {
        return ParsedMarkdown::Malformed {
            title,
            body,
            error: "Managed frontmatter requires a string type".into(),
        };
    };
    if !valid_object_id(&id, &object_type) {
        return ParsedMarkdown::Malformed {
            title,
            body,
            error: "The stable ID does not match the object type".into(),
        };
    }
    let created = properties
        .get("created")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let updated = properties
        .get("updated")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let mut custom_properties = properties;
    for key in ["id", "type", "created", "updated"] {
        custom_properties.remove(key);
    }
    ParsedMarkdown::Managed(WorkspaceObject {
        id,
        object_type,
        title,
        body,
        relative_path: relative_path.to_owned(),
        revision: revision(bytes),
        created,
        updated,
        properties: custom_properties,
    })
}

pub fn serialize_object(object: &WorkspaceObject) -> Result<Vec<u8>> {
    if !valid_object_id(&object.id, &object.object_type) {
        return Err(CoreError::new(
            "invalid_object_id",
            ErrorCategory::Identity,
            "The stable ID does not match the object type",
            "serialize_object",
        ));
    }
    let mut ordered = serde_yaml_ng::Mapping::new();
    let mut insert = |key: &str, value: serde_json::Value| -> Result<()> {
        let value = serde_yaml_ng::to_value(value).map_err(|_| {
            CoreError::new(
                "serialize_failed",
                ErrorCategory::Parse,
                "Frontmatter could not be serialized",
                "serialize_object",
            )
        })?;
        ordered.insert(serde_yaml_ng::Value::String(key.into()), value);
        Ok(())
    };
    insert("id", serde_json::Value::String(object.id.clone()))?;
    insert(
        "type",
        serde_json::Value::String(object.object_type.clone()),
    )?;
    for key in [
        "status",
        "priority",
        "project",
        "due",
        "date",
        "start",
        "end",
        "kanban_order",
    ] {
        if let Some(value) = object.properties.get(key) {
            insert(key, value.clone())?;
        }
    }
    for (key, value) in &object.properties {
        if !matches!(
            key.as_str(),
            "id" | "type"
                | "status"
                | "priority"
                | "project"
                | "due"
                | "date"
                | "start"
                | "end"
                | "kanban_order"
                | "created"
                | "updated"
        ) {
            insert(key, value.clone())?;
        }
    }
    if let Some(created) = &object.created {
        insert("created", serde_json::Value::String(created.clone()))?;
    }
    if let Some(updated) = &object.updated {
        insert("updated", serde_json::Value::String(updated.clone()))?;
    }
    let yaml = serde_yaml_ng::to_string(&ordered).map_err(|_| {
        CoreError::new(
            "serialize_failed",
            ErrorCategory::Parse,
            "Frontmatter could not be serialized",
            "serialize_object",
        )
    })?;
    let mut result = format!("---\n{}---\n\n# {}\n", yaml, object.title.trim());
    if !object.body.trim().is_empty() {
        result.push('\n');
        result.push_str(object.body.trim_end());
        result.push('\n');
    }
    Ok(result.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{new_object_id, now_rfc3339};

    #[test]
    fn managed_markdown_round_trips_deterministically() {
        let now = now_rfc3339();
        let object = WorkspaceObject {
            id: new_object_id("note"),
            object_type: "note".into(),
            title: "Example".into(),
            body: "Body".into(),
            relative_path: "notes/example.md".into(),
            revision: String::new(),
            created: Some(now.clone()),
            updated: Some(now),
            properties: BTreeMap::from([("custom".into(), serde_json::json!("kept"))]),
        };
        let first = serialize_object(&object).unwrap();
        let ParsedMarkdown::Managed(parsed) = parse_markdown(&object.relative_path, &first) else {
            panic!("managed object expected")
        };
        let second = serialize_object(&parsed).unwrap();
        assert_eq!(first, second);
        assert_eq!(parsed.properties["custom"], "kept");
    }

    #[test]
    fn malformed_frontmatter_is_not_managed() {
        assert!(matches!(
            parse_markdown("bad.md", b"---\nid: [\n---\n# Bad"),
            ParsedMarkdown::Malformed { .. }
        ));
    }

    #[test]
    fn crlf_frontmatter_is_parsed_as_managed() {
        let id = new_object_id("note");
        let bytes = format!("---\r\nid: {id}\r\ntype: note\r\n---\r\n# Title\r\n\r\nBody\r\n");
        let ParsedMarkdown::Managed(object) = parse_markdown("note.md", bytes.as_bytes()) else {
            panic!("CRLF frontmatter should be managed");
        };
        assert_eq!(object.body, "Body");
    }
}
