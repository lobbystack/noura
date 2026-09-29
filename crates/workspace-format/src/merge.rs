//! Three-way merges for drafts that changed while the canonical file also
//! changed. Native and browser clients share these rules so the same edits
//! merge, or conflict, the same way everywhere.

use std::collections::BTreeMap;

use crate::WorkspaceObject;

/// Canonical metadata keys that always come from the file, never a draft.
const CANONICAL_KEYS: [&str; 4] = ["id", "type", "created", "updated"];

fn is_canonical_key(key: &str) -> bool {
    CANONICAL_KEYS.contains(&key)
}

/// Whether Markdown contains constructs a line merge could silently break:
/// HTML, directive fences, or reference-style link definitions.
pub fn markdown_requires_manual_review(body: &str) -> bool {
    body.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("<")
            || trimmed.starts_with(":::")
            || (trimmed.starts_with('[') && trimmed.contains("]:"))
    })
}

/// Merge a note body line by line. Returns `None` when both sides changed the
/// same lines or any version needs manual review.
pub fn merge_markdown_body(base: &str, local: &str, external: &str) -> Option<String> {
    if [base, local, external]
        .into_iter()
        .any(markdown_requires_manual_review)
    {
        return None;
    }
    merge_text(base, local, external)
}

/// Merge text line by line. Returns `None` when both sides changed the same
/// lines.
pub fn merge_text(base: &str, local: &str, external: &str) -> Option<String> {
    diffy::merge(base, local, external).ok()
}

/// The draft side of a managed object merge: what the client loaded (`base`)
/// and what it holds now (`local`).
#[derive(Debug, Clone, Copy)]
pub struct ManagedDraftFields<'a> {
    pub base_title: &'a str,
    pub local_title: &'a str,
    pub base_body: &'a str,
    pub local_body: &'a str,
    pub base_properties: &'a BTreeMap<String, serde_json::Value>,
    pub local_properties: &'a BTreeMap<String, serde_json::Value>,
}

/// Why a managed draft could not merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedMergeConflict {
    /// Both sides changed the same body lines.
    Body,
    /// Both sides changed the same property to different values.
    Property,
}

/// Merge a managed draft field by field against the canonical file. Body text
/// merges line by line; title and every top-level property merge
/// independently. Differences to the same field from both sides conflict.
pub fn merge_managed_fields(
    draft: ManagedDraftFields<'_>,
    canonical: &WorkspaceObject,
) -> Result<WorkspaceObject, ManagedMergeConflict> {
    let merged_body = merge_text(draft.base_body, draft.local_body, &canonical.body)
        .ok_or(ManagedMergeConflict::Body)?;
    let merged_title = if draft.local_title != draft.base_title {
        draft.local_title.to_owned()
    } else {
        canonical.title.clone()
    };
    let mut merged_properties = canonical.properties.clone();
    for (key, local_value) in draft.local_properties {
        let external_value = canonical.properties.get(key);
        let base_value = draft.base_properties.get(key);
        let locally_changed = Some(local_value) != base_value;
        let externally_changed = external_value != base_value;
        if locally_changed && externally_changed && external_value != Some(local_value) {
            return Err(ManagedMergeConflict::Property);
        }
        if locally_changed {
            merged_properties.insert(key.clone(), local_value.clone());
        } else if externally_changed {
            merged_properties.insert(
                key.clone(),
                external_value.cloned().unwrap_or(serde_json::Value::Null),
            );
        }
    }
    for key in draft.base_properties.keys() {
        if !draft.local_properties.contains_key(key)
            && !canonical.properties.contains_key(key)
            && !is_canonical_key(key)
        {
            merged_properties.remove(key);
        }
    }
    Ok(WorkspaceObject {
        id: canonical.id.clone(),
        object_type: canonical.object_type.clone(),
        title: merged_title,
        body: merged_body,
        relative_path: canonical.relative_path.clone(),
        revision: canonical.revision.clone(),
        created: canonical.created.clone(),
        updated: canonical.updated.clone(),
        properties: merged_properties,
    })
}

/// Properties without the canonical metadata keys a draft may not change.
pub fn normalized_properties(
    properties: &BTreeMap<String, serde_json::Value>,
) -> BTreeMap<String, serde_json::Value> {
    properties
        .iter()
        .filter(|(key, _)| !is_canonical_key(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn markdown_merge_covers_common_line_oriented_content() {
        let cases = [
            (
                "start\n\nneutral one\n\nneutral two\n\nend\n",
                "start\n\ninserted\n\nneutral one\n\nneutral two\n\nend\n",
                "start\n\nneutral one\n\nneutral two\n\nend external\n",
                vec!["inserted", "end external"],
            ),
            (
                "keep\nremove local\nkeep two\nexternal tail\n",
                "keep\nkeep two\nexternal tail\n",
                "keep\nremove local\nkeep two\nchanged tail\n",
                vec!["keep two", "changed tail"],
            ),
            (
                "- alpha\n- beta\n\nparagraph\n",
                "- alpha local\n- beta\n\nparagraph\n",
                "- alpha\n- beta\n\nparagraph external\n",
                vec!["alpha local", "paragraph external"],
            ),
            (
                "```rs\nlet a = 1;\n```\n\nafter\n",
                "```rs\nlet a = 2;\n```\n\nafter\n",
                "```rs\nlet a = 1;\n```\n\nafter external\n",
                vec!["let a = 2", "after external"],
            ),
            (
                "| A | B |\n| - | - |\n| 1 | 2 |\n\nafter\n",
                "| A | B |\n| - | - |\n| 1 | local |\n\nafter\n",
                "| A | B |\n| - | - |\n| 1 | 2 |\n\nafter external\n",
                vec!["local", "after external"],
            ),
            (
                "café\n\n世界\n",
                "café local\n\n世界\n",
                "café\n\n世界 external\n",
                vec!["café local", "世界 external"],
            ),
            (
                "one\r\n\r\ntwo\r\n",
                "one local\r\n\r\ntwo\r\n",
                "one\r\n\r\ntwo external\r\n",
                vec!["one local", "two external"],
            ),
            (
                "one\n\ntwo",
                "one local\n\ntwo",
                "one\n\ntwo external",
                vec!["one local", "two external"],
            ),
        ];
        for (index, (base, local, external, fragments)) in cases.into_iter().enumerate() {
            let merged = merge_markdown_body(base, local, external)
                .unwrap_or_else(|| panic!("case {index} unexpectedly conflicted"));
            for fragment in fragments {
                assert!(
                    merged.contains(fragment),
                    "missing {fragment:?} in {merged:?}"
                );
            }
            assert!(!merged.contains("<<<<<<<"));
        }
        assert!(merge_markdown_body("same\n", "local\n", "external\n").is_none());
    }

    #[test]
    fn markdown_body_merge_refuses_constructs_that_need_review() {
        for body in ["<div>\n", "  :::note\n", "[ref]: https://example.com\n"] {
            assert!(markdown_requires_manual_review(body));
            assert!(merge_markdown_body(body, &format!("{body}local\n"), body).is_none());
        }
        assert!(!markdown_requires_manual_review("[link](target)\nplain\n"));
    }

    #[test]
    fn text_merge_ignores_the_manual_review_guard() {
        assert_eq!(
            merge_text(
                "<a>\n\nmiddle\n\nb\n",
                "<a>\n\nmiddle\n\nb local\n",
                "<a> external\n\nmiddle\n\nb\n"
            )
            .as_deref(),
            Some("<a> external\n\nmiddle\n\nb local\n")
        );
        assert!(merge_text("same\n", "local\n", "external\n").is_none());
    }

    fn canonical(
        title: &str,
        body: &str,
        properties: BTreeMap<String, serde_json::Value>,
    ) -> WorkspaceObject {
        WorkspaceObject {
            id: "task_01j00000000000000000000000".into(),
            object_type: "task".into(),
            title: title.into(),
            body: body.into(),
            relative_path: "tasks/example.md".into(),
            revision: "external".into(),
            created: Some("2026-09-01T00:00:00Z".into()),
            updated: Some("2026-09-02T00:00:00Z".into()),
            properties,
        }
    }

    fn properties(value: serde_json::Value) -> BTreeMap<String, serde_json::Value> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn managed_fields_merge_independently() {
        let base = properties(json!({"status": "todo", "priority": "low", "gone": 1}));
        let local = properties(json!({"status": "done", "priority": "low"}));
        let external = canonical(
            "External title",
            "one\n\ntwo external\n",
            properties(json!({"status": "todo", "priority": "high", "id": "kept"})),
        );
        let merged = merge_managed_fields(
            ManagedDraftFields {
                base_title: "Title",
                local_title: "Title",
                base_body: "one\n\ntwo\n",
                local_body: "one local\n\ntwo\n",
                base_properties: &base,
                local_properties: &local,
            },
            &external,
        )
        .unwrap();
        assert_eq!(merged.title, "External title");
        assert_eq!(merged.body, "one local\n\ntwo external\n");
        assert_eq!(
            merged.properties,
            properties(json!({"status": "done", "priority": "high", "id": "kept"}))
        );
        assert_eq!(merged.revision, "external");
        assert_eq!(merged.updated, external.updated);
    }

    #[test]
    fn managed_fields_conflict_on_the_same_field() {
        let base = properties(json!({"status": "todo"}));
        let local = properties(json!({"status": "done"}));
        let draft = ManagedDraftFields {
            base_title: "Title",
            local_title: "Local title",
            base_body: "body\n",
            local_body: "body\n",
            base_properties: &base,
            local_properties: &local,
        };
        let changed_property = canonical(
            "Title",
            "body\n",
            properties(json!({"status": "in-progress"})),
        );
        assert_eq!(
            merge_managed_fields(draft, &changed_property),
            Err(ManagedMergeConflict::Property)
        );
        let same_value = canonical("Title", "body\n", properties(json!({"status": "done"})));
        assert_eq!(
            merge_managed_fields(draft, &same_value).unwrap().title,
            "Local title"
        );
        let body_draft = ManagedDraftFields {
            local_body: "local\n",
            ..draft
        };
        let changed_body = canonical("Title", "external\n", base.clone());
        assert_eq!(
            merge_managed_fields(body_draft, &changed_body),
            Err(ManagedMergeConflict::Body)
        );
    }

    #[test]
    fn normalized_properties_drop_canonical_metadata() {
        let value = properties(json!({
            "id": "x", "type": "task", "created": "c", "updated": "u", "status": "todo"
        }));
        assert_eq!(
            normalized_properties(&value),
            properties(json!({"status": "todo"}))
        );
    }
}
