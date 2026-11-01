//! A strict reader for the lib0 v1 structure of a yrs update (sync-v2.md
//! §22), run before yrs sees the bytes. yrs 0.26.0 masks a decoded client ID
//! to its low 53 bits, wraps varints longer than their type and reads strings
//! without checking UTF-8, so this reader decides those cases itself.
//!
//! It reads every struct and delete range without allocating per count, and
//! refuses anything a text-mode note can't hold: GC structs, content other
//! than a string or a deletion, a parent other than the root `content`, and a
//! map key.

use std::collections::{BTreeMap, BTreeSet};

use crate::Unreadable;

/// The largest client ID yrs holds without `small-client`: below 2^53.
pub const CLIENT_LIMIT: u64 = 1 << 53;
/// Counts, clocks, lengths and parent markers stay below 2^32.
pub const U32_LIMIT: u64 = 1 << 32;
/// The most a client's first clock plus its lengths may reach, 2^32 − 1,
/// so the clock after its last element still fits yrs's `u32`.
pub const MAX_CLOCK: u64 = U32_LIMIT - 1;

/// The name of the one root of a text-mode note's document.
pub const ROOT: &str = "content";

const GC: u8 = 0;
const DELETED: u8 = 1;
const STRING: u8 = 4;
const SKIP: u8 = 10;
const HAS_ORIGIN: u8 = 0b1000_0000;
const HAS_RIGHT_ORIGIN: u8 = 0b0100_0000;
const HAS_PARENT_SUB: u8 = 0b0010_0000;
const REF_MASK: u8 = 0b0001_1111;

/// One client's run of structs in an update's struct section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    /// The client ID the structs sit under.
    pub client: u64,
    /// How many structs the run holds.
    pub structs: u64,
}

/// What a strict read of an update found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shape {
    /// The struct section's runs, in order.
    pub runs: Vec<Run>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8, Unreadable> {
        let byte = *self.bytes.get(self.at).ok_or(Unreadable::YrsParse)?;
        self.at += 1;
        Ok(byte)
    }

    /// A lib0 varuint below `limit`, in its shortest encoding.
    fn var(&mut self, limit: u64) -> Result<u64, Unreadable> {
        let mut value: u64 = 0;
        let mut shift = 0u32;
        loop {
            let byte = self.u8()?;
            let bits = u64::from(byte & 0x7f);
            if shift >= 64 || (shift > 0 && bits >> (64 - shift) != 0) {
                return Err(Unreadable::VarintRange);
            }
            value |= bits << shift;
            if byte & 0x80 == 0 {
                if shift > 0 && byte == 0 {
                    return Err(Unreadable::VarintNotShortest);
                }
                break;
            }
            shift += 7;
        }
        if value >= limit {
            return Err(Unreadable::VarintRange);
        }
        Ok(value)
    }

    fn bytes(&mut self, len: u64) -> Result<&'a [u8], Unreadable> {
        let len = usize::try_from(len).map_err(|_| Unreadable::YrsParse)?;
        let end = self.at.checked_add(len).ok_or(Unreadable::YrsParse)?;
        let bytes = self.bytes.get(self.at..end).ok_or(Unreadable::YrsParse)?;
        self.at = end;
        Ok(bytes)
    }

    /// A lib0 string: a varuint byte length, then that many bytes of UTF-8.
    fn string(&mut self) -> Result<&'a str, Unreadable> {
        let len = self.var(U32_LIMIT)?;
        let bytes = self.bytes(len)?;
        std::str::from_utf8(bytes).map_err(|_| Unreadable::Utf8)
    }

    fn id(&mut self) -> Result<(u64, u64), Unreadable> {
        let client = self.var(CLIENT_LIMIT)?;
        let clock = self.var(U32_LIMIT)?;
        Ok((client, clock))
    }
}

/// Reads `bytes` as a lib0 v1 update and checks every rule of §22 that
/// depends on the bytes alone.
pub fn read_update(bytes: &[u8]) -> Result<Shape, Unreadable> {
    let mut r = Reader { bytes, at: 0 };
    let mut shape = Shape::default();

    let clients = r.var(U32_LIMIT)?;
    let mut seen = BTreeSet::new();
    for _ in 0..clients {
        let structs = r.var(U32_LIMIT)?;
        let client = r.var(CLIENT_LIMIT)?;
        let mut clock = r.var(U32_LIMIT)?;
        if !seen.insert(client) {
            return Err(Unreadable::DuplicateClient);
        }
        for _ in 0..structs {
            let len = read_struct(&mut r, client, clock)?;
            clock += len;
            if clock > MAX_CLOCK {
                return Err(Unreadable::ClockOverflow);
            }
        }
        shape.runs.push(Run { client, structs });
    }

    let clients = r.var(U32_LIMIT)?;
    let mut seen = BTreeSet::new();
    for _ in 0..clients {
        let client = r.var(CLIENT_LIMIT)?;
        if !seen.insert(client) {
            return Err(Unreadable::DuplicateClient);
        }
        let ranges = r.var(U32_LIMIT)?;
        for _ in 0..ranges {
            let clock = r.var(U32_LIMIT)?;
            let len = r.var(U32_LIMIT)?;
            if len == 0 {
                return Err(Unreadable::ZeroLength);
            }
            if clock + len > MAX_CLOCK {
                return Err(Unreadable::DeleteRangeOverflow);
            }
        }
    }

    if r.at != bytes.len() {
        return Err(Unreadable::YrsParse);
    }
    Ok(shape)
}

/// A run of elements of one item, as an update carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item<'a> {
    /// Its first clock.
    pub clock: u64,
    /// How many clocks it takes.
    pub len: u64,
    /// The element on its left when it was written.
    pub origin: Option<(u64, u64)>,
    /// The element on its right when it was written.
    pub right: Option<(u64, u64)>,
    /// Its text, or `None` for a deleted item.
    pub text: Option<&'a str>,
}

/// The items and delete ranges of an update, by client.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Items<'a> {
    /// Each client's items, by rising clock.
    pub clients: BTreeMap<u64, Vec<Item<'a>>>,
    /// Each client's delete ranges, as clock and length.
    pub deletes: BTreeMap<u64, Vec<(u64, u64)>>,
}

/// Reads the items of an update that passed the verdict. Skips leave no
/// item.
pub fn read_items(bytes: &[u8]) -> Result<Items<'_>, Unreadable> {
    let mut r = Reader { bytes, at: 0 };
    let mut items = Items::default();
    for _ in 0..r.var(U32_LIMIT)? {
        let structs = r.var(U32_LIMIT)?;
        let client = r.var(CLIENT_LIMIT)?;
        let mut clock = r.var(U32_LIMIT)?;
        let list = items.clients.entry(client).or_default();
        for _ in 0..structs {
            let info = r.u8()?;
            if info & REF_MASK == SKIP {
                clock += r.var(U32_LIMIT)?;
                continue;
            }
            let origin = (info & HAS_ORIGIN != 0).then(|| r.id()).transpose()?;
            let right = (info & HAS_RIGHT_ORIGIN != 0).then(|| r.id()).transpose()?;
            if info & (HAS_ORIGIN | HAS_RIGHT_ORIGIN) == 0 {
                r.var(U32_LIMIT)?;
                r.string()?;
            }
            let (len, text) = match info & REF_MASK {
                DELETED => (r.var(U32_LIMIT)?, None),
                STRING => {
                    let text = r.string()?;
                    (text.encode_utf16().count() as u64, Some(text))
                }
                _ => return Err(Unreadable::Content),
            };
            list.push(Item {
                clock,
                len,
                origin,
                right,
                text,
            });
            clock += len;
        }
    }
    for _ in 0..r.var(U32_LIMIT)? {
        let client = r.var(CLIENT_LIMIT)?;
        let list = items.deletes.entry(client).or_default();
        for _ in 0..r.var(U32_LIMIT)? {
            let clock = r.var(U32_LIMIT)?;
            let len = r.var(U32_LIMIT)?;
            list.push((clock, len));
        }
    }
    Ok(items)
}

/// Reads one struct of `client`'s run, starting at `clock`, and returns the
/// clocks it takes.
fn read_struct(r: &mut Reader<'_>, client: u64, clock: u64) -> Result<u64, Unreadable> {
    // An item follows or precedes elements that existed when its writer
    // made it, so never one of its own client's at or after its clock.
    let earlier = |(origin_client, origin_clock): (u64, u64)| {
        if origin_client == client && origin_clock >= clock {
            Err(Unreadable::ForwardOrigin)
        } else {
            Ok(())
        }
    };
    let info = r.u8()?;
    let len = match info & REF_MASK {
        GC => return Err(Unreadable::GcStruct),
        SKIP => r.var(U32_LIMIT)?,
        content => {
            if info & HAS_PARENT_SUB != 0 {
                return Err(Unreadable::MapKey);
            }
            if info & HAS_ORIGIN != 0 {
                earlier(r.id()?)?;
            }
            if info & HAS_RIGHT_ORIGIN != 0 {
                earlier(r.id()?)?;
            }
            if info & (HAS_ORIGIN | HAS_RIGHT_ORIGIN) == 0 {
                let marker = r.var(U32_LIMIT)?;
                if marker != 1 || r.string()? != ROOT {
                    return Err(Unreadable::Parent);
                }
            }
            match content {
                DELETED => r.var(U32_LIMIT)?,
                STRING => r.string()?.encode_utf16().count() as u64,
                _ => return Err(Unreadable::Content),
            }
        }
    };
    if len == 0 {
        return Err(Unreadable::ZeroLength);
    }
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(mut value: u64, out: &mut Vec<u8>) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    fn string(text: &str, out: &mut Vec<u8>) {
        var(text.len() as u64, out);
        out.extend_from_slice(text.as_bytes());
    }

    /// One client inserting `text` at the start of `content`.
    fn insert(client: u64, clock: u64, text: &str) -> Vec<u8> {
        let mut out = Vec::new();
        var(1, &mut out);
        var(1, &mut out);
        var(client, &mut out);
        var(clock, &mut out);
        out.push(STRING);
        var(1, &mut out);
        string(ROOT, &mut out);
        string(text, &mut out);
        var(0, &mut out);
        out
    }

    #[test]
    fn a_plain_insert_reads() {
        let shape = read_update(&insert(7, 0, "hi")).unwrap();
        assert_eq!(
            shape.runs,
            [Run {
                client: 7,
                structs: 1
            }]
        );
    }

    #[test]
    fn a_padded_varint_is_not_shortest() {
        let mut bytes = insert(7, 0, "hi");
        // The client ID 7 as 0x87 0x00.
        bytes[2] = 0x87;
        bytes.insert(3, 0x00);
        assert_eq!(read_update(&bytes), Err(Unreadable::VarintNotShortest));
    }

    #[test]
    fn a_client_id_of_2_pow_53_is_out_of_range() {
        assert_eq!(
            read_update(&insert(1 << 53, 0, "hi")),
            Err(Unreadable::VarintRange)
        );
        assert!(read_update(&insert((1 << 53) - 1, 0, "hi")).is_ok());
    }

    #[test]
    fn clocks_past_2_pow_32_overflow() {
        assert!(read_update(&insert(1, MAX_CLOCK - 2, "hi")).is_ok());
        assert_eq!(
            read_update(&insert(1, MAX_CLOCK - 1, "hi")),
            Err(Unreadable::ClockOverflow)
        );
    }

    #[test]
    fn invalid_utf8_is_refused() {
        let mut bytes = insert(7, 0, "hi");
        let end = bytes.len() - 2;
        bytes[end] = 0xff;
        assert_eq!(read_update(&bytes), Err(Unreadable::Utf8));
    }

    #[test]
    fn trailing_bytes_fail_to_parse() {
        let mut bytes = insert(7, 0, "hi");
        bytes.push(0);
        assert_eq!(read_update(&bytes), Err(Unreadable::YrsParse));
    }

    #[test]
    fn a_varint_past_64_bits_is_out_of_range() {
        let mut bytes = vec![0xff; 10];
        bytes.push(0x01);
        assert_eq!(read_update(&bytes), Err(Unreadable::VarintRange));
    }
}
