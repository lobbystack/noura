//! Native validation and materialization of Yjs update-v1 text documents.
use super::{crypto::decode, identifier, invalid};
use crate::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use yrs::{
    Any, Doc, GetString, OffsetKind, Options, Out, ReadTxn, StateVector, Text, Transact, Update,
    updates::{decoder::Decode, encoder::Encode},
};

pub const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_DOCUMENT_BYTES: usize = 32 * 1024 * 1024;
pub const TEXT_NAME: &str = "content";

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationOpenInput {
    pub relative_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationSession {
    pub object_id: String,
    pub generation: String,
    pub session_id: String,
    pub update: String,
    pub revision: String,
    pub read_only: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationSubmitInput {
    pub session_id: String,
    pub generation: String,
    pub batch_id: String,
    pub updates: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationReceipt {
    pub revision: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborativeChange {
    pub version: u8,
    pub object_id: String,
    pub generation: String,
    pub updates: Vec<String>,
}
impl CollaborativeChange {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.object_id)?;
        identifier(&self.generation)?;
        if self.version != 1 {
            return Err(invalid("collaboration_unsupported_version"));
        }
        validate_updates(&self.updates)
    }
}

/// An isolated candidate. Never mutate the accepted document before validating and committing bytes.
pub struct TextDocument {
    doc: Doc,
}
impl TextDocument {
    fn empty() -> Self {
        let doc = Doc::with_options(Options {
            offset_kind: OffsetKind::Utf16,
            ..Options::default()
        });
        doc.get_or_insert_text(TEXT_NAME);
        Self { doc }
    }
    pub fn fresh_generation(text: &str, generation: &str) -> Result<Self> {
        identifier(generation)?;
        validate_text(text)?;
        let hash = blake3::hash(generation.as_bytes());
        let client_id = u32::from_le_bytes(
            hash.as_bytes()[..4]
                .try_into()
                .map_err(|_| invalid("collaboration_invalid_generation"))?,
        )
        .max(1);
        let doc = Doc::with_options(Options {
            client_id: yrs::ClientID::new(client_id as u64),
            offset_kind: OffsetKind::Utf16,
            ..Options::default()
        });
        let field = doc.get_or_insert_text(TEXT_NAME);
        field.insert(&mut doc.transact_mut(), 0, text);
        Ok(Self { doc })
    }
    pub fn fresh(text: &str) -> Result<Self> {
        validate_text(text)?;
        let result = Self::empty();
        result
            .doc
            .get_or_insert_text(TEXT_NAME)
            .insert(&mut result.doc.transact_mut(), 0, text);
        Ok(result)
    }
    pub fn restore(state: &str) -> Result<Self> {
        let result = Self::empty();
        result.integrate(&decode(state, 2, MAX_DOCUMENT_BYTES)?)?;
        result.validate()?;
        Ok(result)
    }
    fn integrate(&self, bytes: &[u8]) -> Result<()> {
        let update =
            Update::decode_v1(bytes).map_err(|_| invalid("collaboration_invalid_update"))?;
        self.doc
            .transact_mut()
            .apply_update(update)
            .map_err(|_| invalid("collaboration_invalid_update"))
    }
    pub fn apply(&self, updates: &[String]) -> Result<Self> {
        validate_updates(updates)?;
        let result = Self::restore(&self.state()?)?;
        for update in updates {
            result.integrate(&decode(update, 2, MAX_DOCUMENT_BYTES)?)?;
        }
        result.validate()?;
        result.state()?;
        Ok(result)
    }
    pub fn text(&self) -> String {
        self.doc
            .get_or_insert_text(TEXT_NAME)
            .get_string(&self.doc.transact())
    }
    pub fn state(&self) -> Result<String> {
        let bytes = self
            .doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(invalid("collaboration_history_limit"));
        }
        Ok(STANDARD.encode(bytes))
    }
    pub fn state_vector(&self) -> String {
        STANDARD.encode(self.doc.transact().state_vector().encode_v1())
    }
    pub fn diff(&self, state_vector: &str) -> Result<String> {
        let vector = StateVector::decode_v1(&decode(state_vector, 1, MAX_DOCUMENT_BYTES)?)
            .map_err(|_| invalid("collaboration_invalid_state_vector"))?;
        Ok(STANDARD.encode(self.doc.transact().encode_state_as_update_v1(&vector)))
    }
    /// Make the smallest single text splice, using UTF-16 offsets shared with the editor.
    pub fn replace_text(&self, next: &str) -> Result<(Self, String)> {
        validate_text(next)?;
        let result = Self::restore(&self.state()?)?;
        let prior = self.text();
        let common_prefix = prior
            .chars()
            .zip(next.chars())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        let old_tail = &prior[common_prefix..];
        let next_tail = &next[common_prefix..];
        let common_suffix = old_tail
            .chars()
            .rev()
            .zip(next_tail.chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        let start = prior[..common_prefix].encode_utf16().count() as u32;
        let delete = old_tail[..old_tail.len() - common_suffix]
            .encode_utf16()
            .count() as u32;
        let insert = &next_tail[..next_tail.len() - common_suffix];
        {
            let text = result.doc.get_or_insert_text(TEXT_NAME);
            let mut txn = result.doc.transact_mut();
            if delete != 0 {
                text.remove_range(&mut txn, start, delete);
            }
            if !insert.is_empty() {
                text.insert(&mut txn, start, insert);
            }
        }
        let update = result.diff(&self.state_vector())?;
        result.validate()?;
        Ok((result, update))
    }
    fn validate(&self) -> Result<()> {
        let text = self.doc.get_or_insert_text(TEXT_NAME);
        let txn = self.doc.transact();
        if txn.has_missing_updates() {
            return Err(invalid("collaboration_missing_dependencies"));
        }
        for (name, value) in txn.root_refs() {
            if name != TEXT_NAME || !matches!(value, Out::YText(_)) {
                return Err(invalid("collaboration_unsupported_type"));
            }
        }
        for part in text.diff(&txn, |_| ()) {
            if part.attributes.is_some() || !matches!(part.insert, Out::Any(Any::String(_))) {
                return Err(invalid("collaboration_unsupported_type"));
            }
        }
        validate_text(&text.get_string(&txn))
    }
}
fn validate_updates(updates: &[String]) -> Result<()> {
    if updates.is_empty()
        || updates.len() > 100
        || updates.iter().map(String::len).sum::<usize>() > MAX_DOCUMENT_BYTES * 4 / 3 + 4
    {
        return Err(invalid("collaboration_update_limit"));
    }
    for update in updates {
        decode(update, 2, MAX_DOCUMENT_BYTES)?;
    }
    Ok(())
}
pub fn validate_text(text: &str) -> Result<()> {
    if text.len() > MAX_TEXT_BYTES || text.contains('\0') {
        return Err(invalid("collaboration_unsupported_text"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn yjs_fixture_edits_deletions_and_local_undo_interoperate() {
        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/workspace-format/fixtures/yjs-text-v1.json"
        ))
        .unwrap();
        let mut doc = TextDocument::restore(value["base"].as_str().unwrap()).unwrap();
        for (update, expected) in value["updates"]
            .as_array()
            .unwrap()
            .iter()
            .zip(value["states"].as_array().unwrap())
        {
            doc = doc.apply(&[update.as_str().unwrap().into()]).unwrap();
            assert_eq!(doc.text(), expected.as_str().unwrap());
        }
        assert_eq!(doc.text(), value["text"].as_str().unwrap());
        let vector = value["vector"].as_str().unwrap();
        let delta = doc.diff(vector).unwrap();
        let restored = TextDocument::restore(value["state"].as_str().unwrap()).unwrap();
        assert_eq!(restored.apply(&[delta]).unwrap().text(), doc.text());
    }
    #[test]
    fn eight_mib_text_boundary_survives_binary_state_restore() {
        let text = "x".repeat(MAX_TEXT_BYTES);
        let doc = TextDocument::fresh_generation(&text, "boundary").unwrap();
        let restored = TextDocument::restore(&doc.state().unwrap()).unwrap();
        assert_eq!(restored.text().len(), MAX_TEXT_BYTES);
        let oversized = format!("{text}x");
        assert!(restored.replace_text(&oversized).is_err());
        assert_eq!(restored.text().len(), MAX_TEXT_BYTES);
    }
    #[test]
    fn unicode_replacement_preserves_utf16_offsets() {
        let doc = TextDocument::fresh("A😀B\n").unwrap();
        let (updated, delta) = doc.replace_text("A😀中文B\n").unwrap();
        assert_eq!(doc.apply(&[delta]).unwrap().text(), updated.text());
        assert_eq!(doc.text(), "A😀B\n");
    }
    #[test]
    fn replicas_merge_concurrent_edits_without_duplicate_bootstrap() {
        let base = TextDocument::fresh("start end").unwrap();
        let (left, left_update) = base.replace_text("LEFT start end").unwrap();
        let (right, right_update) = base.replace_text("start end RIGHT").unwrap();
        let left = left.apply(&[right_update]).unwrap();
        let right = right.apply(&[left_update]).unwrap();
        assert_eq!(left.text(), "LEFT start end RIGHT");
        assert_eq!(left.text(), right.text());
        assert_eq!(
            left.apply(&[left.state().unwrap()]).unwrap().text(),
            left.text()
        );
    }
    #[test]
    fn invalid_updates_leave_accepted_state_unchanged() {
        let doc = TextDocument::fresh("accepted").unwrap();
        assert!(doc.apply(&[STANDARD.encode([255, 255, 255])]).is_err());
        let other = Doc::new();
        other.get_or_insert_map("secrets");
        use yrs::Map;
        other
            .get_or_insert_map("secrets")
            .insert(&mut other.transact_mut(), "key", "value");
        let bytes = other
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        assert!(doc.apply(&[STANDARD.encode(bytes)]).is_err());
        assert_eq!(doc.text(), "accepted");
    }
}
