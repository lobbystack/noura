//! Collaborative document text and metadata: reading a file's body,
//! rendering it back, and the format and metadata patches a transaction
//! carries.

use super::*;

pub(super) fn is_markdown(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

pub(super) fn body(path: &str, bytes: &[u8], object_id: &str) -> Result<String> {
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(invalid("collaboration_unsupported_text"));
    }
    let parsed = markdown::parse_markdown(path, bytes);
    if matches!(parsed, ParsedMarkdown::Malformed { .. }) && is_markdown(path) {
        return Err(invalid("collaboration_invalid_managed_document"));
    }
    if is_markdown(path)
        && let ParsedMarkdown::Managed(object) = parsed
    {
        if object.id != object_id {
            return Err(invalid("sync_identity_changed"));
        }
        Ok(object.body)
    } else {
        let (text, _) = split_raw_bytes(bytes)?;
        crate::sync::collaboration::validate_text(&text)?;
        Ok(text)
    }
}

pub(super) fn render(path: &str, bytes: &[u8], object_id: &str, text: &str) -> Result<Vec<u8>> {
    crate::sync::collaboration::validate_text(text)?;
    let next = if is_markdown(path)
        && let ParsedMarkdown::Managed(mut object) = markdown::parse_markdown(path, bytes)
    {
        if object.id != object_id {
            return Err(invalid("sync_identity_changed"));
        }
        object.body = text.into();
        markdown::serialize_object(&object)?
    } else {
        let (_, layout) = split_raw_bytes(bytes)?;
        compose_raw_bytes(text, &layout)
    };
    if next.len() > MAX_TEXT_BYTES {
        return Err(invalid("collaboration_unsupported_text"));
    }
    if !matches!(
        markdown::parse_markdown(path, bytes),
        ParsedMarkdown::Managed(_)
    ) && matches!(
        markdown::parse_markdown(path, &next),
        ParsedMarkdown::Managed(_) | ParsedMarkdown::Malformed { .. }
    ) && is_markdown(path)
    {
        return Err(invalid("collaboration_document_mode_changed"));
    }
    Ok(next)
}

pub(super) fn validate_metadata_field(field: &str) -> Result<()> {
    let valid = field == "title"
        || field.strip_prefix("property:").is_some_and(|key| {
            !key.is_empty()
                && key.len() <= 128
                && !key.contains(['\0', '\r', '\n'])
                && !matches!(key, "id" | "type" | "created" | "updated")
        });
    if !valid {
        return Err(invalid("collaboration_invalid_metadata_field"));
    }
    Ok(())
}

fn metadata_value(object: &WorkspaceObject, field: &str) -> Result<CollaborativeMetadataValue> {
    validate_metadata_field(field)?;
    if field == "title" {
        return Ok(CollaborativeMetadataValue::Value {
            value: serde_json::Value::String(object.title.clone()),
        });
    }
    let key = field
        .strip_prefix("property:")
        .ok_or_else(|| invalid("collaboration_invalid_metadata_field"))?;
    Ok(object
        .properties
        .get(key)
        .map_or(CollaborativeMetadataValue::Missing, |value| {
            CollaborativeMetadataValue::Value {
                value: value.clone(),
            }
        }))
}

pub(super) fn apply_metadata_patches(
    path: &str,
    bytes: &[u8],
    object_id: &str,
    patches: &[CollaborativeMetadataPatch],
) -> Result<Vec<u8>> {
    let ParsedMarkdown::Managed(mut object) = markdown::parse_markdown(path, bytes) else {
        return Err(invalid("collaboration_invalid_managed_document"));
    };
    if object.id != object_id {
        return Err(invalid("sync_identity_changed"));
    }
    let mut fields = BTreeSet::new();
    for patch in patches {
        validate_metadata_field(&patch.field)?;
        if !fields.insert(&patch.field)
            || metadata_value(&object, &patch.field)? != patch.expected
            || patch.expected == patch.value
        {
            return Err(invalid("collaboration_metadata_conflict"));
        }
        match (patch.field.as_str(), &patch.value) {
            ("title", CollaborativeMetadataValue::Value { value }) => {
                let title = value
                    .as_str()
                    .filter(|title| !title.trim().is_empty())
                    .ok_or_else(|| invalid("collaboration_invalid_metadata_patch"))?;
                object.title = title.trim().into();
            }
            ("title", CollaborativeMetadataValue::Missing) => {
                return Err(invalid("collaboration_invalid_metadata_patch"));
            }
            (field, value) => {
                let key = field
                    .strip_prefix("property:")
                    .ok_or_else(|| invalid("collaboration_invalid_metadata_field"))?;
                match value {
                    CollaborativeMetadataValue::Missing => {
                        object.properties.remove(key);
                    }
                    CollaborativeMetadataValue::Value { value } => {
                        object.properties.insert(key.into(), value.clone());
                    }
                }
            }
        }
    }
    normalize_domain_properties(&object.object_type, &mut object.properties)?;
    let serialized = markdown::serialize_object(&object)?;
    if body(path, &serialized, object_id)? != object.body {
        return Err(invalid("collaboration_canonicalization_loss"));
    }
    Ok(serialized)
}

pub(super) fn metadata_patches(
    object: &WorkspaceObject,
    target: &WorkspaceObject,
) -> Result<Vec<CollaborativeMetadataPatch>> {
    if object.id != target.id
        || object.object_type != target.object_type
        || object.created != target.created
    {
        return Err(invalid("collaboration_protected_metadata"));
    }
    let mut metadata = Vec::new();
    if target.title != object.title {
        metadata.push(CollaborativeMetadataPatch {
            field: "title".into(),
            expected: metadata_value(object, "title")?,
            value: metadata_value(target, "title")?,
        });
    }
    let property_keys = object
        .properties
        .keys()
        .chain(target.properties.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for key in property_keys {
        let field = format!("property:{key}");
        let expected = metadata_value(object, &field)?;
        let value = metadata_value(target, &field)?;
        if expected != value {
            metadata.push(CollaborativeMetadataPatch {
                field,
                expected,
                value,
            });
        }
    }
    Ok(metadata)
}

pub(super) fn text_format(path: &str, bytes: &[u8]) -> Result<Option<CollaborativeTextFormat>> {
    if is_markdown(path)
        && matches!(
            markdown::parse_markdown(path, bytes),
            ParsedMarkdown::Managed(_)
        )
    {
        return Ok(None);
    }
    let (_, layout) = split_raw_bytes(bytes)?;
    Ok(Some(CollaborativeTextFormat {
        has_bom: layout.has_bom,
        // Every device must derive the same format from the same bytes, so
        // this keeps the original rule: any CRLF line counts.
        uses_crlf: layout.any_crlf(),
    }))
}

pub(super) fn apply_format_patch(
    path: &str,
    bytes: &[u8],
    patch: &CollaborativeFormatPatch,
) -> Result<Vec<u8>> {
    let (body, layout) = split_raw_bytes(bytes)?;
    let (uses_crlf, has_bom) = (layout.any_crlf(), layout.has_bom);
    if is_markdown(path)
        && matches!(
            markdown::parse_markdown(path, bytes),
            ParsedMarkdown::Managed(_)
        )
        || (CollaborativeTextFormat { has_bom, uses_crlf } != patch.expected)
    {
        return Err(invalid("collaboration_format_conflict"));
    }
    // A format change rewrites every line with the new ending.
    Ok(compose_raw_bytes(
        &body,
        &RawLayout::uniform(&body, patch.value.uses_crlf, patch.value.has_bom),
    ))
}
