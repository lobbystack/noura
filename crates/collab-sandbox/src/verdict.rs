//! The verdict stage (sync-v2.md §22).

use crate::Sandbox;
use crate::guest::{Failure, Guest};

/// The fuel every verdict may use before its per-byte share.
pub const VERDICT_BASE_FUEL: u64 = 1_000_000;
/// The fuel a verdict may use per byte of the record's payload and blob.
pub const VERDICT_FUEL_PER_BYTE: u64 = 1_000;
/// The most memory a verdict instance may hold.
pub const VERDICT_MEMORY_BYTES: usize = 512 << 20;

/// The fuel a verdict on `bytes` bytes of payload and blob may use.
pub fn verdict_budget(bytes: u64) -> u64 {
    VERDICT_BASE_FUEL.saturating_add(VERDICT_FUEL_PER_BYTE.saturating_mul(bytes))
}

/// A remote record, as the verdict judges it.
#[derive(Clone, Copy, Debug)]
pub struct Record<'a> {
    /// The decrypted, unpadded payload of a `text_snapshot` or
    /// `text_update`.
    pub payload: &'a [u8],
    /// The blob's file, when the payload's content is a blob.
    pub blob: Option<&'a [u8]>,
    /// The signer's card index in the workspace, from the verified log.
    pub card_index: u32,
    /// How many cards the workspace indexed at the record's
    /// `policy_revision`, from the verified log.
    pub card_count: u32,
}

/// Why a record is unreadable. Each has the code
/// `sync-v2-collaboration.json` names it by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unreadable {
    /// A varint isn't the shortest encoding of its value.
    VarintNotShortest,
    /// A varint's value doesn't fit its field.
    VarintRange,
    /// A client's first clock plus the lengths of its structs passes
    /// 2^32 − 1.
    ClockOverflow,
    /// A delete range's clock plus its length passes 2^32 − 1.
    DeleteRangeOverflow,
    /// The struct section or the delete set names one client ID twice.
    DuplicateClient,
    /// A struct or delete range has length 0.
    ZeroLength,
    /// A string isn't valid UTF-8.
    InvalidUtf8,
    /// The yrs bytes fail to parse.
    YrsParse,
    /// The yrs bytes hold a GC struct.
    GcStruct,
    /// An item's content is neither a string nor a deletion.
    ContentKind,
    /// An item names a parent other than the root `content`.
    Parent,
    /// An item carries a map key.
    MapKey,
    /// An update's `load` lies outside 1 to 2^29 − 1.
    Load,
    /// An update holds a struct under a client ID other than its signer's.
    ForeignClient,
    /// A snapshot holds a struct under a client ID no indexed card has.
    SnapshotClient,
    /// A `meta` key is neither `path` nor `p:` and a valid property name.
    MetaKey,
    /// An update's `meta` entry names a sequence other than 0.
    MetaSeq,
    /// A `p:` value isn't RFC 8785 canonical JSON.
    MetaValue,
    /// `path` lacks a value, breaks the path rules or is missing from a
    /// snapshot.
    MetaPath,
    /// A `p:` key appears on a note whose `object_id` is empty.
    MetaUnmanaged,
    /// `p:id` isn't the JSON string of `object_id`, or is missing from a
    /// managed note's snapshot.
    MetaId,
    /// `p:type` lacks a value, or is missing from a managed note's snapshot.
    MetaType,
    /// The payload doesn't decode, or isn't a text snapshot or update.
    Payload,
    /// The content form and the blob passed with it disagree.
    Blob,
    /// The verdict ran out of fuel.
    Fuel,
    /// The verdict hit its memory limit.
    Memory,
    /// The guest trapped.
    Trap,
}

impl Unreadable {
    /// Every reason, in the order of the guest's codes.
    pub const ALL: [Unreadable; 27] = [
        Self::VarintNotShortest,
        Self::VarintRange,
        Self::ClockOverflow,
        Self::DeleteRangeOverflow,
        Self::DuplicateClient,
        Self::ZeroLength,
        Self::InvalidUtf8,
        Self::YrsParse,
        Self::GcStruct,
        Self::ContentKind,
        Self::Parent,
        Self::MapKey,
        Self::Load,
        Self::ForeignClient,
        Self::SnapshotClient,
        Self::MetaKey,
        Self::MetaSeq,
        Self::MetaValue,
        Self::MetaPath,
        Self::MetaUnmanaged,
        Self::MetaId,
        Self::MetaType,
        Self::Payload,
        Self::Blob,
        Self::Fuel,
        Self::Memory,
        Self::Trap,
    ];

    /// The structured error code the fixture and the review store use.
    pub fn code(self) -> &'static str {
        match self {
            Self::VarintNotShortest => "varint_not_shortest",
            Self::VarintRange => "varint_range",
            Self::ClockOverflow => "clock_overflow",
            Self::DeleteRangeOverflow => "delete_range_overflow",
            Self::DuplicateClient => "duplicate_client",
            Self::ZeroLength => "zero_length",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::YrsParse => "yrs_parse",
            Self::GcStruct => "gc_struct",
            Self::ContentKind => "content_kind",
            Self::Parent => "parent",
            Self::MapKey => "map_key",
            Self::Load => "load",
            Self::ForeignClient => "foreign_client",
            Self::SnapshotClient => "snapshot_client",
            Self::MetaKey => "meta_key",
            Self::MetaSeq => "meta_seq",
            Self::MetaValue => "meta_value",
            Self::MetaPath => "meta_path",
            Self::MetaUnmanaged => "meta_unmanaged",
            Self::MetaId => "meta_id",
            Self::MetaType => "meta_type",
            Self::Payload => "payload",
            Self::Blob => "blob",
            Self::Fuel => "fuel",
            Self::Memory => "memory",
            Self::Trap => "trap",
        }
    }

    /// The reason a guest code names. Codes start at 1, in [`Self::ALL`]
    /// order; the last three come from the host.
    fn from_guest(code: u32) -> Self {
        let guest = &Self::ALL[..Self::ALL.len() - 3];
        code.checked_sub(1)
            .and_then(|index| guest.get(index as usize))
            .copied()
            .unwrap_or(Self::Trap)
    }
}

/// A verdict and the fuel it used, which its writer's allowance pays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verdict {
    /// `Ok` when the record is readable.
    pub outcome: Result<(), Unreadable>,
    /// The fuel the verdict used.
    pub fuel: u64,
}

impl Sandbox {
    /// Judges `record` in a fresh instance that holds no document.
    pub fn verdict(&self, record: &Record<'_>) -> Verdict {
        let blob_len = record.blob.map_or(0, <[u8]>::len);
        let budget = verdict_budget((record.payload.len() + blob_len) as u64);
        // An instance that fails to start spent what it was given.
        let mut fuel = budget;
        let outcome = (|| {
            let mut guest = Guest::new(self, VERDICT_MEMORY_BYTES, budget)?;
            let result = (|| {
                let payload = guest.write(record.payload)?;
                let blob = match record.blob {
                    Some(blob) => guest.write(blob)?,
                    None => 0,
                };
                guest.call::<(u32, u32, u32, u32, u32, u32, u32), u32>(
                    "verdict",
                    (
                        payload,
                        record.payload.len() as u32,
                        blob,
                        blob_len as u32,
                        u32::from(record.blob.is_some()),
                        record.card_index,
                        record.card_count,
                    ),
                )
            })();
            fuel = guest.used();
            result
        })();
        let outcome = match outcome {
            Ok(0) => Ok(()),
            Ok(code) => Err(Unreadable::from_guest(code)),
            Err(Failure::Fuel) => Err(Unreadable::Fuel),
            Err(Failure::Memory) => Err(Unreadable::Memory),
            Err(Failure::Trap(_)) => Err(Unreadable::Trap),
        };
        Verdict { outcome, fuel }
    }
}
