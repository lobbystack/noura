//! The operation record and its payload (§13).

use serde::{Deserialize, Serialize};

use super::unknown_value;
use crate::codec::{
    Bytes, Decode, DecodeError, DecodeErrorKind, Encode, Reader, SortSpec, Sorted, Text, Whole,
    Writer,
};
use crate::crypto::{AEAD_TAG_LEN, CryptoError, derive_key, open, seal};
use crate::padding::{PaddingError, pad, padded_length, unpad};
use crate::tag::Tag;
use crate::types::{Digest, Id, Nonce, SecretKey};

/// The most covers an operation carries (§27).
pub const MAX_COVERS: usize = 256;
/// The most blobs an operation lists (§27).
pub const MAX_BLOBS: usize = 4;
/// The most bytes an encoded payload takes before padding, `n` in §6.
pub const MAX_PAYLOAD_LEN: usize = 262_144;
/// The largest operation ciphertext: `padded_length(262,148)` plus the tag.
pub const MAX_OPERATION_CIPHERTEXT: usize = 270_352;
/// The longest path, in bytes.
pub const MAX_PATH_LEN: usize = 4_096;
/// The largest load number, 2^29 − 1 (§13).
pub const MAX_LOAD: u32 = (1 << 29) - 1;

crate::wire_struct! {
    /// `cover`: every operation on the `rid` from `device` with a counter at
    /// or below `counter` is superseded.
    pub struct Cover {
        /// The covered device.
        pub device: Id,
        /// The highest covered counter.
        pub counter: u64,
    }
}

/// Sorts covers and chain heads by device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ByDevice;

impl SortSpec<Cover> for ByDevice {
    fn key(item: &Cover) -> Vec<u8> {
        item.device.0.to_vec()
    }
}

/// The signed body of an operation (§13).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Operation {
    /// The workspace.
    pub workspace: Id,
    /// The key domain.
    pub domain: Id,
    /// The signing device.
    pub device: Id,
    /// The position in the device's chain for this domain, at least 1.
    pub counter: u64,
    /// The digest of the device's previous record in this domain.
    pub previous: Digest,
    /// The epoch whose key encrypts the payload.
    pub epoch: u64,
    /// The policy head revision the device saw.
    pub policy_revision: u64,
    /// The digest of the change at `policy_revision`.
    pub policy_head: Digest,
    /// The digest of the attempt this record re-seals or replaces at this
    /// chain position, or zero.
    pub previous_attempt: Digest,
    /// The server's restore generation when the device signed.
    pub generation: u64,
    /// `covered_seq` of the latest checkpoint the device reached, or 0.
    pub checkpoint_seq: u64,
    /// The digest of that checkpoint, or zero.
    pub checkpoint: Digest,
    /// The remote object ID.
    pub rid: Id,
    /// The delete flag: 1 for a delete, 0 for any other operation.
    pub delete: bool,
    /// The covers, sorted by device, one per device.
    pub supersedes: Sorted<Cover, ByDevice, MAX_COVERS>,
    /// The blobs the payload references, sorted.
    pub blobs: Sorted<Id, Whole, MAX_BLOBS>,
    /// The payload nonce.
    pub nonce: Nonce,
    /// The padded payload under the operation subkey, with its tag.
    pub ciphertext: Bytes<MAX_OPERATION_CIPHERTEXT>,
}

impl Operation {
    /// The AEAD associated data: the body prefix through `blobs`, then the
    /// digest of the epoch record that announced the epoch (§4).
    pub fn aad(&self, epoch_record: &Digest) -> Vec<u8> {
        let mut w = Writer::new();
        self.encode_prefix(&mut w);
        w.put(epoch_record);
        w.into_bytes()
    }

    fn encode_prefix(&self, w: &mut Writer) {
        w.tag(Tag::Op);
        w.put(&self.workspace);
        w.put(&self.domain);
        w.put(&self.device);
        w.put(&self.counter);
        w.put(&self.previous);
        w.put(&self.epoch);
        w.put(&self.policy_revision);
        w.put(&self.policy_head);
        w.put(&self.previous_attempt);
        w.put(&self.generation);
        w.put(&self.checkpoint_seq);
        w.put(&self.checkpoint);
        w.put(&self.rid);
        w.put(&self.delete);
        w.put(&self.supersedes);
        w.put(&self.blobs);
    }

    /// Encrypts `payload` into this operation under `epoch_key`, the key of
    /// the epoch that `epoch_record` announced.
    pub fn seal_payload(
        &mut self,
        payload: &Payload,
        epoch_key: &SecretKey,
        epoch_record: &Digest,
        nonce: Nonce,
    ) -> Result<(), SealError> {
        let encoded = payload.to_bytes();
        if encoded.len() > MAX_PAYLOAD_LEN {
            return Err(SealError::TooLarge);
        }
        let padded = pad(&encoded).map_err(|_| SealError::TooLarge)?;
        self.nonce = nonce;
        let key = derive_key(crate::crypto::context::OPERATION_KEY, epoch_key.expose());
        self.ciphertext = Bytes(seal(&key, &nonce, &self.aad(epoch_record), &padded));
        Ok(())
    }

    /// Decrypts, unpads and parses the payload, then checks that it matches
    /// the header's `rid` and delete flag and the class the server gave the
    /// operation (§13, "Client checks on an operation").
    pub fn open_payload(
        &self,
        epoch_key: &SecretKey,
        epoch_record: &Digest,
        class: OpClass,
    ) -> Result<Payload, OpenError> {
        let key = derive_key(crate::crypto::context::OPERATION_KEY, epoch_key.expose());
        let padded = open(&key, &self.nonce, &self.aad(epoch_record), &self.ciphertext)
            .map_err(OpenError::Decrypt)?;
        let encoded = unpad(&padded).map_err(OpenError::Padding)?;
        if encoded.len() > MAX_PAYLOAD_LEN {
            return Err(OpenError::TooLarge);
        }
        let payload = Payload::from_bytes(encoded).map_err(OpenError::Decode)?;
        if payload.rid != self.rid {
            return Err(OpenError::RidMismatch);
        }
        if payload.kind.is_delete() != self.delete {
            return Err(OpenError::DeleteFlagMismatch);
        }
        if payload.kind.class() != class {
            return Err(OpenError::ClassMismatch);
        }
        Ok(payload)
    }
}

/// Whether the server counts an operation as a state operation or a patch
/// (§13): an empty `supersedes` on an existing `rid` makes a patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpClass {
    /// A file, delete or text snapshot.
    State,
    /// A text update.
    Patch,
}

/// Why sealing a payload failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SealError {
    /// The encoded payload exceeds 262,144 bytes; it belongs in a blob.
    #[error("payload exceeds 262,144 bytes")]
    TooLarge,
}

/// Why a client counts an operation's payload as unreadable (§13).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OpenError {
    /// The ciphertext doesn't decrypt.
    #[error("payload doesn't decrypt: {0}")]
    Decrypt(CryptoError),
    /// The padding is malformed.
    #[error("payload padding: {0}")]
    Padding(PaddingError),
    /// The encoded payload exceeds 262,144 bytes.
    #[error("payload exceeds 262,144 bytes")]
    TooLarge,
    /// The payload doesn't parse.
    #[error("payload doesn't parse: {0}")]
    Decode(DecodeError),
    /// The payload names another `rid` than the header.
    #[error("payload rid differs from the header's")]
    RidMismatch,
    /// The payload's kind doesn't match the header's delete flag.
    #[error("payload kind doesn't match the delete flag")]
    DeleteFlagMismatch,
    /// The payload's kind doesn't match the class the server gave it.
    #[error("payload kind doesn't match its class")]
    ClassMismatch,
}

impl Encode for Operation {
    fn encode(&self, w: &mut Writer) {
        self.encode_prefix(w);
        w.put(&self.nonce);
        w.put(&self.ciphertext);
    }
}

impl Decode for Operation {
    const MIN_SIZE: usize = Tag::Op.encoded_len()
        + 16 * 3
        + 8
        + 32
        + 8 * 2
        + 32 * 2
        + 8 * 2
        + 32
        + 16
        + 1
        + 4
        + 4
        + 24
        + 4;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.tag(Tag::Op)?;
        let workspace = r.get()?;
        let domain = r.get()?;
        let device = r.get()?;
        let counter_offset = r.offset();
        let counter: u64 = r.get()?;
        if counter == 0 {
            return Err(r.error_at(DecodeErrorKind::OutOfRange, counter_offset));
        }
        Ok(Self {
            workspace,
            domain,
            device,
            counter,
            previous: r.get()?,
            epoch: r.get()?,
            policy_revision: r.get()?,
            policy_head: r.get()?,
            previous_attempt: r.get()?,
            generation: r.get()?,
            checkpoint_seq: r.get()?,
            checkpoint: r.get()?,
            rid: r.get()?,
            delete: r.get()?,
            supersedes: r.get()?,
            blobs: r.get()?,
            nonce: r.get()?,
            ciphertext: r.get()?,
        })
    }
}

/// The padded length of an operation ciphertext whose payload encodes to
/// `n` bytes, tag included.
pub fn operation_ciphertext_len(n: usize) -> u64 {
    padded_length(n as u64 + 4) + AEAD_TAG_LEN as u64
}

crate::wire_struct! {
    /// `origin`: where a cross-domain move came from, or where it went.
    pub struct Origin {
        /// The other domain.
        pub domain: Id,
        /// The object's `rid` there.
        pub rid: Id,
    }
}

crate::wire_struct! {
    /// `blob_ref`: how to fetch and decrypt a blob (§21).
    pub struct BlobRef {
        /// The blob ID.
        pub blob: Id,
        /// The blob's ciphertext length.
        pub ciphertext_length: u64,
        /// `BLAKE3(tag(noura.sync.blob.parts) || list<digest>)`.
        pub ciphertext_digest: Digest,
        /// The blob's own random key.
        pub blob_key: SecretKey,
        /// The file's length `L`.
        pub plaintext_length: u64,
    }
}

/// `content`: inline bytes, or a reference to a blob.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "form", rename_all = "snake_case")]
pub enum Content {
    /// Form 1: the bytes inline.
    Inline {
        /// The bytes.
        bytes: Bytes,
    },
    /// Form 2: a blob.
    Blob(BlobRef),
}

impl Encode for Content {
    fn encode(&self, w: &mut Writer) {
        match self {
            Content::Inline { bytes } => {
                w.u8(1);
                w.put(bytes);
            }
            Content::Blob(blob) => {
                w.u8(2);
                w.put(blob);
            }
        }
    }
}

impl Decode for Content {
    const MIN_SIZE: usize = 1 + 4;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let offset = r.offset();
        match r.u8()? {
            1 => Ok(Content::Inline { bytes: r.get()? }),
            2 => Ok(Content::Blob(r.get()?)),
            _ => Err(unknown_value(r, offset)),
        }
    }
}

crate::wire_struct! {
    /// `meta`: a text-mode note's path, under `path`, or a property, under
    /// `p:` and its name, with the sequence that places a snapshot's entry
    /// and the value, absent for a removal (§13, §22).
    pub struct Meta {
        /// The key.
        pub key: Text,
        /// The effective sequence of the write, or 0.
        pub seq: u64,
        /// The value, or absent for a removed property.
        pub value: Option<Bytes>,
    }
}

/// Sorts `meta` entries by key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ByKey;

impl SortSpec<Meta> for ByKey {
    fn key(item: &Meta) -> Vec<u8> {
        item.key.as_bytes().to_vec()
    }
}

/// A payload's `meta` entries, sorted by key.
pub type MetaList = Sorted<Meta, ByKey>;

crate::byte_enum! {
    /// Why an object was deleted.
    pub enum DeleteReason {
        /// 1: deleted.
        Deleted = 1,
        /// 2: moved to another domain.
        Moved = 2,
    }
}

/// A workspace-relative path that passes the rules of §13, carried as
/// `bytes` of at most 4,096.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SyncPath(String);

impl SyncPath {
    /// Validates `path`.
    pub fn new(path: impl Into<String>) -> Result<Self, DecodeErrorKind> {
        let path = path.into();
        validate_path(path.as_bytes())?;
        Ok(Self(path))
    }

    /// The path.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Encode for SyncPath {
    fn encode(&self, w: &mut Writer) {
        w.bytes(self.0.as_bytes());
    }
}

impl Decode for SyncPath {
    const MIN_SIZE: usize = 4;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let offset = r.offset();
        let bytes = r.bytes(MAX_PATH_LEN)?;
        let path = validate_path(bytes).map_err(|kind| r.error_at(kind, offset))?;
        Ok(Self(path.to_owned()))
    }
}

const WINDOWS_DEVICES: &[&str] = &[
    "CON", "CONIN$", "CONOUT$", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5",
    "COM6", "COM7", "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5",
    "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];

/// Checks a payload path against §13: UTF-8, at most 4,096 bytes, `/`
/// between components, and no absolute path, `.` or `..` component, empty
/// component, control character, Windows reserved name or character,
/// trailing dot or space, or reserved root `.noura` or `.git` in any ASCII
/// case.
pub fn validate_path(bytes: &[u8]) -> Result<&str, DecodeErrorKind> {
    if bytes.len() > MAX_PATH_LEN {
        return Err(DecodeErrorKind::TooLong);
    }
    let path = std::str::from_utf8(bytes).map_err(|_| DecodeErrorKind::InvalidUtf8)?;
    if path.is_empty()
        || path
            .chars()
            .any(|c| c.is_control() || "\\:<>\"|?*".contains(c))
    {
        return Err(DecodeErrorKind::InvalidPath);
    }
    for component in path.split('/') {
        let stem = component
            .split('.')
            .next()
            .unwrap_or_default()
            .trim_end_matches(' ')
            .to_uppercase();
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.ends_with(['.', ' '])
            || WINDOWS_DEVICES.contains(&stem.as_str())
        {
            return Err(DecodeErrorKind::InvalidPath);
        }
    }
    let root = path
        .split('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if root == ".noura" || root == ".git" {
        return Err(DecodeErrorKind::InvalidPath);
    }
    Ok(path)
}

/// The kind-specific body of a payload (§13).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PayloadKind {
    /// 1: a file's state.
    File {
        /// The managed file's frontmatter ID, or empty.
        object_id: Text,
        /// The workspace-relative path.
        path: SyncPath,
        /// BLAKE3 of the file's bytes as materialized.
        revision: Digest,
        /// The bytes.
        content: Content,
        /// Where a cross-domain move came from.
        origin: Option<Origin>,
    },
    /// 2: a delete.
    Delete {
        /// The managed file's frontmatter ID, or empty.
        object_id: Text,
        /// BLAKE3 of the deleted bytes, or zero when unknown.
        revision: Digest,
        /// Why.
        reason: DeleteReason,
        /// Where a moved object went.
        target: Option<Origin>,
    },
    /// 3: a text-mode note's yrs document.
    TextSnapshot {
        /// The managed file's frontmatter ID, or empty.
        object_id: Text,
        /// BLAKE3 of the file's bytes as materialized.
        revision: Digest,
        /// The text lineage.
        text_lineage: Id,
        /// The path and properties.
        meta: MetaList,
        /// The encoded yrs document.
        content: Content,
        /// Where a cross-domain move came from.
        origin: Option<Origin>,
    },
    /// 4: a yrs update to a text-mode note.
    TextUpdate {
        /// The managed file's frontmatter ID, or empty.
        object_id: Text,
        /// The text lineage.
        text_lineage: Id,
        /// The load number, from 1 to 2^29 − 1.
        load: u32,
        /// Changed path and properties.
        meta: MetaList,
        /// The yrs update.
        content: Content,
    },
}

impl PayloadKind {
    /// The kind byte.
    pub fn code(&self) -> u8 {
        match self {
            PayloadKind::File { .. } => 1,
            PayloadKind::Delete { .. } => 2,
            PayloadKind::TextSnapshot { .. } => 3,
            PayloadKind::TextUpdate { .. } => 4,
        }
    }

    /// Whether this is a delete.
    pub fn is_delete(&self) -> bool {
        matches!(self, PayloadKind::Delete { .. })
    }

    /// The class the kind belongs in: a text update is a patch, every other
    /// kind a state operation.
    pub fn class(&self) -> OpClass {
        match self {
            PayloadKind::TextUpdate { .. } => OpClass::Patch,
            _ => OpClass::State,
        }
    }
}

/// An operation payload (§13).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Payload {
    /// The remote object ID, which must match the header's.
    pub rid: Id,
    /// The kind and its body.
    #[serde(flatten)]
    pub kind: PayloadKind,
}

impl Encode for Payload {
    fn encode(&self, w: &mut Writer) {
        w.tag(Tag::Payload);
        w.put(&self.rid);
        w.u8(self.kind.code());
        match &self.kind {
            PayloadKind::File {
                object_id,
                path,
                revision,
                content,
                origin,
            } => {
                w.put(object_id);
                w.put(path);
                w.put(revision);
                w.put(content);
                w.put(origin);
            }
            PayloadKind::Delete {
                object_id,
                revision,
                reason,
                target,
            } => {
                w.put(object_id);
                w.put(revision);
                w.put(reason);
                w.put(target);
            }
            PayloadKind::TextSnapshot {
                object_id,
                revision,
                text_lineage,
                meta,
                content,
                origin,
            } => {
                w.put(object_id);
                w.put(revision);
                w.put(text_lineage);
                w.put(meta);
                w.put(content);
                w.put(origin);
            }
            PayloadKind::TextUpdate {
                object_id,
                text_lineage,
                load,
                meta,
                content,
            } => {
                w.put(object_id);
                w.put(text_lineage);
                w.put(load);
                w.put(meta);
                w.put(content);
            }
        }
    }
}

impl Decode for Payload {
    const MIN_SIZE: usize = Tag::Payload.encoded_len() + 16 + 1 + 2 + 32 + 1 + 1;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.tag(Tag::Payload)?;
        let rid = r.get()?;
        let kind_offset = r.offset();
        let kind = match r.u8()? {
            1 => PayloadKind::File {
                object_id: r.get()?,
                path: r.get()?,
                revision: r.get()?,
                content: r.get()?,
                origin: r.get()?,
            },
            2 => PayloadKind::Delete {
                object_id: r.get()?,
                revision: r.get()?,
                reason: r.get()?,
                target: r.get()?,
            },
            3 => PayloadKind::TextSnapshot {
                object_id: r.get()?,
                revision: r.get()?,
                text_lineage: r.get()?,
                meta: r.get()?,
                content: r.get()?,
                origin: r.get()?,
            },
            4 => {
                let object_id = r.get()?;
                let text_lineage = r.get()?;
                let load_offset = r.offset();
                let load: u32 = r.get()?;
                if !(1..=MAX_LOAD).contains(&load) {
                    return Err(r.error_at(DecodeErrorKind::OutOfRange, load_offset));
                }
                PayloadKind::TextUpdate {
                    object_id,
                    text_lineage,
                    load,
                    meta: r.get()?,
                    content: r.get()?,
                }
            }
            _ => return Err(unknown_value(r, kind_offset)),
        };
        Ok(Self { rid, kind })
    }
}
