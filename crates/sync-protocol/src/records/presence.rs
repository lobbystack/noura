//! Presence records (§23).

use crate::codec::{Bytes, Decode, DecodeError, Encode, List, Reader, Writer};
use crate::crypto::{CryptoError, context, derive_key, keyed_hash, open, seal};
use crate::padding::{PaddingError, pad, unpad};
use crate::tag::Tag;
use crate::types::{Digest, Id, Nonce, SecretKey};

/// The most bytes of a yrs relative position in a selection.
pub const MAX_POSITION_LEN: usize = 192;
/// The most selections a presence payload carries.
pub const MAX_SELECTIONS: usize = 8;
/// The most bytes of a padded presence plaintext.
pub const MAX_PRESENCE_PLAINTEXT: usize = 4_096;

crate::byte_enum! {
    /// Whether the collaborator is typing.
    pub enum PresenceState {
        /// 1: active.
        Active = 1,
        /// 2: idle.
        Idle = 2,
    }
}

crate::wire_struct! {
    /// One selection: two yrs relative positions.
    pub struct Selection {
        /// The anchor.
        pub anchor: Bytes<MAX_POSITION_LEN>,
        /// The head.
        pub head: Bytes<MAX_POSITION_LEN>,
    }
}

crate::wire_struct! {
    /// A presence record's padded plaintext.
    pub struct PresencePayload: tag PresencePayload {
        /// The note.
        pub rid: Id,
        /// Active or idle.
        pub state: PresenceState,
        /// At most 8 selections.
        pub selections: List<Selection, MAX_SELECTIONS>,
    }
}

/// The signed body of a presence record.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Presence {
    /// The workspace.
    pub workspace: Id,
    /// The key domain.
    pub domain: Id,
    /// The domain's current epoch.
    pub epoch: u64,
    /// The sending device.
    pub device: Id,
    /// The connection's session ID.
    pub session: Id,
    /// The record's sequence within the session.
    pub sequence: u64,
    /// Unix time in minutes when sent.
    pub minute: u32,
    /// The note's presence channel.
    pub channel: Id,
    /// The nonce.
    pub nonce: Nonce,
    /// The padded payload and its tag, at most 4 KiB of plaintext.
    pub ciphertext: Bytes<{ MAX_PRESENCE_PLAINTEXT + 16 }>,
}

/// A note's presence channel in an epoch: the first 16 bytes of
/// `BLAKE3.keyed_hash(presence subkey, tag(noura.sync.presence.channel) ||
/// rid)`.
pub fn presence_channel(epoch_key: &SecretKey, rid: &Id) -> Id {
    let key = derive_key(context::PRESENCE_KEY, epoch_key.expose());
    let mut data = Tag::PresenceChannel.bytes();
    data.extend_from_slice(&rid.0);
    let digest: Digest = keyed_hash(&key, &data);
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest.0[..16]);
    Id(id)
}

/// Why a presence payload failed to open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PresenceError {
    /// Decryption failed.
    #[error("{0}")]
    Decrypt(#[from] CryptoError),
    /// Malformed padding.
    #[error("{0}")]
    Padding(#[from] PaddingError),
    /// The payload doesn't parse.
    #[error("{0}")]
    Decode(#[from] DecodeError),
    /// The padded plaintext exceeds 4 KiB.
    #[error("presence plaintext exceeds 4 KiB")]
    TooLarge,
}

impl Presence {
    /// The AEAD associated data: the prefix through `channel`, then the
    /// epoch record's digest.
    pub fn aad(&self, epoch_record: &Digest) -> Vec<u8> {
        let mut w = Writer::new();
        self.encode_prefix(&mut w);
        w.put(epoch_record);
        w.into_bytes()
    }

    fn encode_prefix(&self, w: &mut Writer) {
        w.tag(Tag::Presence);
        w.put(&self.workspace);
        w.put(&self.domain);
        w.put(&self.epoch);
        w.put(&self.device);
        w.put(&self.session);
        w.put(&self.sequence);
        w.put(&self.minute);
        w.put(&self.channel);
    }

    /// Encrypts `payload` under the presence subkey of `epoch_key`.
    pub fn seal_payload(
        &mut self,
        payload: &PresencePayload,
        epoch_key: &SecretKey,
        epoch_record: &Digest,
        nonce: Nonce,
    ) -> Result<(), PresenceError> {
        let padded = pad(&payload.to_bytes())?;
        if padded.len() > MAX_PRESENCE_PLAINTEXT {
            return Err(PresenceError::TooLarge);
        }
        let key = derive_key(context::PRESENCE_KEY, epoch_key.expose());
        self.nonce = nonce;
        self.ciphertext = Bytes(seal(&key, &nonce, &self.aad(epoch_record), &padded));
        Ok(())
    }

    /// Decrypts the payload.
    pub fn open_payload(
        &self,
        epoch_key: &SecretKey,
        epoch_record: &Digest,
    ) -> Result<PresencePayload, PresenceError> {
        let key = derive_key(context::PRESENCE_KEY, epoch_key.expose());
        let padded = open(&key, &self.nonce, &self.aad(epoch_record), &self.ciphertext)?;
        Ok(PresencePayload::from_bytes(unpad(&padded)?)?)
    }

    /// Whether `minute` lies within 2 minutes of the receiver's clock.
    pub fn within_clock_window(&self, now_minute: u32) -> bool {
        within_clock_window(self.minute, now_minute)
    }
}

/// Whether a presence record sent at `minute` lies within the 2-minute
/// window receivers allow around their clock's `now_minute` (§23).
pub fn within_clock_window(minute: u32, now_minute: u32) -> bool {
    minute.abs_diff(now_minute) <= 2
}

impl Encode for Presence {
    fn encode(&self, w: &mut Writer) {
        self.encode_prefix(w);
        w.put(&self.nonce);
        w.put(&self.ciphertext);
    }
}

impl Decode for Presence {
    const MIN_SIZE: usize = Tag::Presence.encoded_len() + 16 * 4 + 8 * 2 + 4 + 16 + 24 + 4;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.tag(Tag::Presence)?;
        Ok(Self {
            workspace: r.get()?,
            domain: r.get()?,
            epoch: r.get()?,
            device: r.get()?,
            session: r.get()?,
            sequence: r.get()?,
            minute: r.get()?,
            channel: r.get()?,
            nonce: r.get()?,
            ciphertext: r.get()?,
        })
    }
}
