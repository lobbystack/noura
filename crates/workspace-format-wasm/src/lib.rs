//! WebAssembly bindings for Noura's canonical workspace-format implementation.
//!
//! This crate intentionally contains no format rules. It only translates the
//! JavaScript boundary to the platform-independent `workspace-format` crate.

use serde::{Serialize, de::DeserializeOwned};
use wasm_bindgen::prelude::*;
use workspace_format::{
    FormatError, WorkspaceManifest, WorkspaceObject, normalize_workspace_manifest,
    parse_markdown as parse_canonical_markdown,
    parse_workspace_manifest as parse_canonical_workspace_manifest, revision,
    serialize_object as serialize_canonical_object,
    serialize_workspace_manifest as serialize_canonical_workspace_manifest, valid_object_id,
    validate_workspace_manifest,
};

fn public_error(code: &'static str, message: &'static str) -> JsValue {
    let json = format!(r#"{{"code":"{code}","message":"{message}"}}"#);
    js_sys::JSON::parse(&json).unwrap_or_else(|_| JsValue::from_str("workspace format error"))
}

fn format_error(error: FormatError) -> JsValue {
    match error {
        FormatError::InvalidObjectId => public_error(
            "invalid_object_id",
            "The stable ID does not match the object type",
        ),
        FormatError::ObjectSerialization => public_error(
            "object_serialization_failed",
            "Frontmatter could not be serialized",
        ),
        FormatError::InvalidManifest => {
            public_error("invalid_manifest", ".noura/workspace.yaml is invalid")
        }
        FormatError::InvalidWorkspaceId => public_error(
            "invalid_workspace_id",
            "The workspace ID must be a lowercase stable workspace ID",
        ),
        FormatError::WorkspaceNameRequired => {
            public_error("workspace_name_required", "A workspace name is required")
        }
        FormatError::InvalidPluginId => public_error(
            "invalid_plugin_id",
            "Plugin identifiers use lowercase letters, digits, and hyphens",
        ),
        FormatError::UnsupportedWorkspaceVersion => public_error(
            "unsupported_workspace_version",
            "This workspace format version is not supported",
        ),
        FormatError::ManifestSerialization => public_error(
            "manifest_serialization_failed",
            ".noura/workspace.yaml could not be serialized",
        ),
    }
}

fn invalid_input() -> JsValue {
    public_error(
        "invalid_input",
        "The workspace format input has an invalid shape",
    )
}

fn internal_error() -> JsValue {
    public_error(
        "internal_error",
        "The workspace format operation could not be completed",
    )
}

fn from_js<T: DeserializeOwned>(value: JsValue) -> Result<T, JsValue> {
    let json = js_sys::JSON::stringify(&value).map_err(|_| invalid_input())?;
    let Some(json) = json.as_string() else {
        return Err(invalid_input());
    };
    serde_json::from_str(&json).map_err(|_| invalid_input())
}

fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
    let json = serde_json::to_string(value).map_err(|_| internal_error())?;
    js_sys::JSON::parse(&json).map_err(|_| internal_error())
}

/// Parses Markdown with the canonical workspace-format parser.
#[wasm_bindgen]
pub fn parse_markdown(relative_path: &str, bytes: &[u8]) -> Result<JsValue, JsValue> {
    to_js(&parse_canonical_markdown(relative_path, bytes))
}

/// Serializes a managed object with the canonical workspace-format serializer.
#[wasm_bindgen]
pub fn serialize_object(object: JsValue) -> Result<Vec<u8>, JsValue> {
    let object = from_js::<WorkspaceObject>(object)?;
    serialize_canonical_object(&object).map_err(format_error)
}

/// Parses and normalizes a workspace manifest with the canonical parser.
#[wasm_bindgen]
pub fn parse_workspace_manifest(bytes: &[u8]) -> Result<JsValue, JsValue> {
    let manifest = parse_canonical_workspace_manifest(bytes).map_err(format_error)?;
    to_js(&manifest)
}

/// Validates, normalizes, and serializes a workspace manifest canonically.
#[wasm_bindgen]
pub fn serialize_workspace_manifest(manifest: JsValue) -> Result<String, JsValue> {
    let mut manifest = from_js::<WorkspaceManifest>(manifest)?;
    validate_workspace_manifest(&manifest).map_err(format_error)?;
    if manifest.format_version != 1 {
        return Err(format_error(FormatError::UnsupportedWorkspaceVersion));
    }
    normalize_workspace_manifest(&mut manifest);
    serialize_canonical_workspace_manifest(&manifest).map_err(format_error)
}

/// Returns the revision used for external-edit detection.
#[wasm_bindgen]
pub fn content_revision(bytes: &[u8]) -> String {
    revision(bytes)
}

/// Checks a stable object ID against its object type.
#[wasm_bindgen]
pub fn is_valid_object_id(id: &str, object_type: &str) -> bool {
    valid_object_id(id, object_type)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use workspace_format::{WorkspaceManifest, valid_object_id, validate_workspace_manifest};

    #[derive(Deserialize)]
    struct Fixtures {
        manifest: Vec<ManifestFixture>,
        object_id: Vec<ObjectIdFixture>,
    }

    #[derive(Deserialize)]
    struct ManifestFixture {
        valid: bool,
        value: serde_json::Value,
    }

    #[derive(Deserialize)]
    struct ObjectIdFixture {
        valid: bool,
        value: String,
    }

    #[test]
    fn binding_uses_shared_conformance_rules() {
        let fixtures: Fixtures = serde_json::from_str(include_str!(
            "../../../docs/workspace-format/fixtures/conformance-v1.json"
        ))
        .unwrap();
        let manifests_match = fixtures.manifest.into_iter().all(|fixture| {
            serde_json::from_value::<WorkspaceManifest>(fixture.value)
                .ok()
                .is_some_and(|manifest| validate_workspace_manifest(&manifest).is_ok())
                == fixture.valid
        });
        let object_ids_match = fixtures
            .object_id
            .into_iter()
            .all(|fixture| valid_object_id(&fixture.value, "note") == fixture.valid);

        assert!(manifests_match && object_ids_match);
    }
}
