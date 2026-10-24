//! The verdict stage (sync-v2.md §22): whether a remote `text_snapshot` or
//! `text_update` is readable, from its bytes and two numbers from the
//! verified log alone.

use sync_protocol::codec::Decode;
use sync_protocol::ids::yrs_client_id;
use sync_protocol::records::{Content, MAX_LOAD, Meta, Payload, PayloadKind, validate_path};
use yrs::updates::decoder::Decode as _;

use crate::Unreadable;
use crate::json;
use crate::lib0::{self, Shape};

/// The low 29 bits of a client ID hold the load number.
const LOAD_BITS: u32 = 29;

/// What the verdict judges.
pub struct Record<'a> {
    /// The decrypted, unpadded payload.
    pub payload: &'a [u8],
    /// The blob's file when the payload's content is a blob reference.
    pub blob: Option<&'a [u8]>,
    /// The signer's card index in the workspace (§8).
    pub card_index: u32,
    /// How many cards the workspace indexed at the record's
    /// `policy_revision`.
    pub card_count: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Snapshot,
    Update,
}

/// Judges `record`: `Ok` when every device may merge it.
pub fn judge(record: &Record<'_>) -> Result<(), Unreadable> {
    let payload = Payload::from_bytes(record.payload).map_err(|_| Unreadable::Payload)?;
    let (kind, object_id, meta, content, load) = match &payload.kind {
        PayloadKind::TextSnapshot {
            object_id,
            meta,
            content,
            ..
        } => (Kind::Snapshot, object_id, meta, content, None),
        PayloadKind::TextUpdate {
            object_id,
            meta,
            content,
            load,
            ..
        } => (Kind::Update, object_id, meta, content, Some(*load)),
        _ => return Err(Unreadable::Payload),
    };
    let yrs_bytes: &[u8] = match (content, record.blob) {
        (Content::Inline { bytes }, None) => bytes,
        (Content::Blob(blob), Some(file)) if blob.plaintext_length == file.len() as u64 => file,
        _ => return Err(Unreadable::Blob),
    };

    // sync-protocol's payload decoder already refuses such a load, so the
    // record comes out as `Payload` above; this keeps the rule if that
    // decoder ever stops checking it.
    if let Some(load) = load
        && !(1..=MAX_LOAD).contains(&load)
    {
        return Err(Unreadable::Load);
    }
    let shape = lib0::read_update(yrs_bytes)?;
    match load {
        Some(load) => check_update_clients(&shape, yrs_client_id(record.card_index, load))?,
        None => check_snapshot_clients(&shape, record.card_count)?,
    }
    yrs::Update::decode_v1(yrs_bytes).map_err(|_| Unreadable::YrsParse)?;
    check_meta(kind, object_id.as_str(), meta)
}

/// An update holds structs under its signer's client ID alone.
fn check_update_clients(shape: &Shape, own: Option<u64>) -> Result<(), Unreadable> {
    for run in &shape.runs {
        if run.structs > 0 && Some(run.client) != own {
            return Err(Unreadable::ForeignClient);
        }
    }
    Ok(())
}

/// A snapshot holds structs under client ID 0, a lineage's first document,
/// or under `i × 2^29 + l` for an indexed card `i` and a load `l` of at
/// least 1.
fn check_snapshot_clients(shape: &Shape, card_count: u32) -> Result<(), Unreadable> {
    for run in &shape.runs {
        if run.structs == 0 || run.client == 0 {
            continue;
        }
        let index = run.client >> LOAD_BITS;
        let load = run.client & ((1 << LOAD_BITS) - 1);
        if index == 0 || index > u64::from(card_count) || load == 0 {
            return Err(Unreadable::SnapshotClient);
        }
    }
    Ok(())
}

/// A property name: 1 to 256 bytes of UTF-8 with no control character.
fn valid_property_name(name: &str) -> bool {
    (1..=256).contains(&name.len()) && !name.chars().any(char::is_control)
}

/// The rules of §22 on a record's `meta` entries.
fn check_meta(kind: Kind, object_id: &str, meta: &[Meta]) -> Result<(), Unreadable> {
    let managed = !object_id.is_empty();
    let (mut path, mut id, mut object_type) = (false, false, false);
    for entry in meta {
        let key = entry.key.as_str();
        let property = key.strip_prefix("p:");
        if key != "path" && !property.is_some_and(valid_property_name) {
            return Err(Unreadable::MetaKey);
        }
        if kind == Kind::Update && entry.seq != 0 {
            return Err(Unreadable::MetaSeq);
        }
        let value = entry.value.as_deref();
        let Some(name) = property else {
            let value = value.ok_or(Unreadable::MetaPath)?;
            validate_path(value).map_err(|_| Unreadable::MetaPath)?;
            path = true;
            continue;
        };
        if !managed {
            return Err(Unreadable::MetaUnmanaged);
        }
        if value.is_some_and(|value| !json::is_canonical(value)) {
            return Err(Unreadable::MetaValue);
        }
        match name {
            "id" => {
                if value != Some(json::canonical_string(object_id).as_bytes()) {
                    return Err(Unreadable::MetaId);
                }
                id = true;
            }
            "type" => {
                value.ok_or(Unreadable::MetaType)?;
                object_type = true;
            }
            _ => {}
        }
    }
    if kind == Kind::Snapshot {
        if !path {
            return Err(Unreadable::MetaPath);
        }
        if managed && !id {
            return Err(Unreadable::MetaId);
        }
        if managed && !object_type {
            return Err(Unreadable::MetaType);
        }
    }
    Ok(())
}
