//! The binary encoding of §4: fixed-width big-endian integers, fixed-size
//! byte fields, length-prefixed `bytes` and `str`, counted `list`s and `opt`
//! discriminators.
//!
//! Decoders are strict. They reject trailing bytes, lengths above a field's
//! limit before allocating, unknown tags, versions and enum values, a tag
//! that doesn't match its position, `opt` discriminators other than 0 and 1,
//! unsorted sorted lists, forbidden all-zero IDs and invalid UTF-8, each with
//! a [`DecodeError`] that names the offset. Every value a decoder accepts
//! re-encodes to exactly the bytes it came from.

use std::fmt;
use std::marker::PhantomData;
use std::ops::Deref;

use serde::{Deserialize, Serialize};

use crate::PROTOCOL_VERSION;
use crate::tag::Tag;

/// No limit of the field's own: the general rule of §4 applies, so a length
/// or count can't exceed the bytes left in the body.
pub const UNBOUNDED: usize = usize::MAX;

/// The limit of a `str` that §27 gives no limit of its own.
pub const STR_MAX: usize = 4_096;

/// What made a decoder refuse its input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecodeErrorKind {
    /// The input ended inside a field.
    Truncated,
    /// Bytes follow the last field.
    TrailingBytes,
    /// A length or count is above its field's limit.
    TooLong,
    /// A `bytes` field whose length the spec fixes has another length.
    WrongLength,
    /// The tag names no domain that version 2 defines.
    UnknownTag,
    /// The tag is a version 2 tag, but not the one this position names.
    WrongTag,
    /// The tag's version isn't 2.
    UnsupportedVersion,
    /// An enum, kind or type byte holds a value the spec doesn't define.
    UnknownValue,
    /// An `opt` discriminator other than 0 or 1.
    BadOption,
    /// A sorted list whose keys don't rise strictly.
    NotSorted,
    /// An all-zero ID where the spec forbids one.
    ZeroId,
    /// A `str` that isn't valid UTF-8.
    InvalidUtf8,
    /// A card whose device ID doesn't derive from its signing key (§5).
    DeviceIdMismatch,
    /// An integer outside the range its field allows.
    OutOfRange,
    /// A path that breaks the rules of §13.
    InvalidPath,
}

impl DecodeErrorKind {
    /// The snake-case name fixtures use.
    pub fn name(self) -> &'static str {
        match self {
            DecodeErrorKind::Truncated => "truncated",
            DecodeErrorKind::TrailingBytes => "trailing_bytes",
            DecodeErrorKind::TooLong => "too_long",
            DecodeErrorKind::WrongLength => "wrong_length",
            DecodeErrorKind::UnknownTag => "unknown_tag",
            DecodeErrorKind::WrongTag => "wrong_tag",
            DecodeErrorKind::UnsupportedVersion => "unsupported_version",
            DecodeErrorKind::UnknownValue => "unknown_value",
            DecodeErrorKind::BadOption => "bad_option",
            DecodeErrorKind::NotSorted => "not_sorted",
            DecodeErrorKind::ZeroId => "zero_id",
            DecodeErrorKind::InvalidUtf8 => "invalid_utf8",
            DecodeErrorKind::DeviceIdMismatch => "device_id_mismatch",
            DecodeErrorKind::OutOfRange => "out_of_range",
            DecodeErrorKind::InvalidPath => "invalid_path",
        }
    }

    /// The server error code a request body failing this way answers with
    /// (§26): `version_unsupported`, `too_large` or `bad_request`.
    pub fn server_code(self) -> &'static str {
        match self {
            DecodeErrorKind::UnsupportedVersion => "version_unsupported",
            DecodeErrorKind::TooLong => "too_large",
            _ => "bad_request",
        }
    }
}

/// A decoder's refusal: what went wrong and at which byte offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{} at byte {offset}", kind.name())]
pub struct DecodeError {
    /// What went wrong.
    pub kind: DecodeErrorKind,
    /// The offset in the decoded input where the failing field starts.
    pub offset: usize,
}

/// Appends encoded fields to a buffer.
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// An empty writer.
    pub fn new() -> Self {
        Self::default()
    }

    /// The bytes written so far.
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// The bytes written so far, borrowed.
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Appends raw bytes with no length prefix.
    pub fn raw(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Appends a `u8`.
    pub fn u8(&mut self, value: u8) {
        self.buf.push(value);
    }

    /// Appends a big-endian `u16`.
    pub fn u16(&mut self, value: u16) {
        self.raw(&value.to_be_bytes());
    }

    /// Appends a big-endian `u32`.
    pub fn u32(&mut self, value: u32) {
        self.raw(&value.to_be_bytes());
    }

    /// Appends a big-endian `u64`.
    pub fn u64(&mut self, value: u64) {
        self.raw(&value.to_be_bytes());
    }

    /// Appends a `bytes` field: a `u32` length, then the bytes.
    ///
    /// # Panics
    ///
    /// Panics when `bytes` is 4 GiB or longer, which no record reaches.
    pub fn bytes(&mut self, bytes: &[u8]) {
        let len = u32::try_from(bytes.len()).expect("a bytes field fits a u32 length");
        self.u32(len);
        self.raw(bytes);
    }

    /// Appends a `str` field: a `u16` length, then the UTF-8 bytes.
    ///
    /// # Panics
    ///
    /// Panics when `value` is 64 KiB or longer, which no record reaches.
    pub fn str(&mut self, value: &str) {
        let len = u16::try_from(value.len()).expect("a str field fits a u16 length");
        self.u16(len);
        self.raw(value.as_bytes());
    }

    /// Appends a tag.
    pub fn tag(&mut self, tag: Tag) {
        self.raw(&tag.bytes());
    }

    /// Appends any encodable value.
    pub fn put<T: Encode + ?Sized>(&mut self, value: &T) {
        value.encode(self);
    }
}

/// Reads encoded fields from a byte slice.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// A reader at the start of `buf`.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// The current offset.
    pub fn offset(&self) -> usize {
        self.pos
    }

    /// The number of bytes left.
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// An error of `kind` at `offset`.
    pub fn error_at(&self, kind: DecodeErrorKind, offset: usize) -> DecodeError {
        DecodeError { kind, offset }
    }

    /// An error of `kind` at the current offset.
    pub fn error(&self, kind: DecodeErrorKind) -> DecodeError {
        self.error_at(kind, self.pos)
    }

    /// Takes `n` raw bytes.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if n > self.remaining() {
            return Err(self.error(DecodeErrorKind::Truncated));
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    /// Takes a fixed-size array.
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    /// Reads a `u8`.
    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.array::<1>()?[0])
    }

    /// Reads a big-endian `u16`.
    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    /// Reads a big-endian `u32`.
    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    /// Reads a big-endian `u64`.
    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    /// Reads a `bytes` field of at most `max` bytes, checking the length
    /// before taking anything.
    pub fn bytes(&mut self, max: usize) -> Result<&'a [u8], DecodeError> {
        let start = self.pos;
        let len = self.u32()? as usize;
        if len > max {
            return Err(self.error_at(DecodeErrorKind::TooLong, start));
        }
        if len > self.remaining() {
            return Err(self.error_at(DecodeErrorKind::Truncated, start));
        }
        self.take(len)
    }

    /// Reads a `bytes` field whose length the spec fixes at `len`.
    pub fn bytes_exact(&mut self, len: usize) -> Result<&'a [u8], DecodeError> {
        let start = self.pos;
        let found = self.u32()? as usize;
        if found != len {
            return Err(self.error_at(DecodeErrorKind::WrongLength, start));
        }
        self.take(len)
    }

    /// Reads a `str` field of at most `max` bytes of UTF-8.
    pub fn str(&mut self, max: usize) -> Result<&'a str, DecodeError> {
        let start = self.pos;
        let len = self.u16()? as usize;
        if len > max {
            return Err(self.error_at(DecodeErrorKind::TooLong, start));
        }
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes).map_err(|_| self.error_at(DecodeErrorKind::InvalidUtf8, start))
    }

    /// Reads a `list` count of at most `max` elements whose smallest
    /// encoding takes `min_size` bytes, checking it against the bytes left
    /// before anything is allocated.
    pub fn count(&mut self, max: usize, min_size: usize) -> Result<usize, DecodeError> {
        let start = self.pos;
        let count = self.u32()? as usize;
        if count > max {
            return Err(self.error_at(DecodeErrorKind::TooLong, start));
        }
        if count.saturating_mul(min_size.max(1)) > self.remaining() {
            return Err(self.error_at(DecodeErrorKind::Truncated, start));
        }
        Ok(count)
    }

    /// Reads an `opt` discriminator: `false` for absent, `true` for present.
    pub fn opt(&mut self) -> Result<bool, DecodeError> {
        let start = self.pos;
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(self.error_at(DecodeErrorKind::BadOption, start)),
        }
    }

    /// Reads a tag and checks that it is `expected`.
    pub fn tag(&mut self, expected: Tag) -> Result<(), DecodeError> {
        let start = self.pos;
        let found = self.any_tag()?;
        if found != expected {
            return Err(self.error_at(DecodeErrorKind::WrongTag, start));
        }
        Ok(())
    }

    /// Reads any version 2 tag.
    pub fn any_tag(&mut self) -> Result<Tag, DecodeError> {
        let start = self.pos;
        let len = self.u8()? as usize;
        // Every v2 tag starts with a byte below 0x40 (§4); a v1 JSON tuple
        // starts with `[`.
        if len >= 0x40 {
            return Err(self.error_at(DecodeErrorKind::UnknownTag, start));
        }
        let domain = self.take(len)?;
        let tag = std::str::from_utf8(domain)
            .ok()
            .and_then(Tag::from_domain)
            .ok_or_else(|| self.error_at(DecodeErrorKind::UnknownTag, start))?;
        if self.u16()? != PROTOCOL_VERSION {
            return Err(self.error_at(DecodeErrorKind::UnsupportedVersion, start));
        }
        Ok(tag)
    }

    /// Checks that no bytes follow the last field.
    pub fn finish(&self) -> Result<(), DecodeError> {
        if self.remaining() != 0 {
            return Err(self.error(DecodeErrorKind::TrailingBytes));
        }
        Ok(())
    }

    /// Reads any decodable value.
    pub fn get<T: Decode>(&mut self) -> Result<T, DecodeError> {
        T::decode(self)
    }
}

/// A value with a §4 encoding.
pub trait Encode {
    /// Appends the encoding to `w`.
    fn encode(&self, w: &mut Writer);

    /// The encoding as a new buffer.
    fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.encode(&mut w);
        w.into_bytes()
    }
}

/// A value that decodes from its §4 encoding.
pub trait Decode: Sized {
    /// The length of the smallest encoding, which bounds list counts.
    const MIN_SIZE: usize;

    /// Reads one value.
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError>;

    /// Decodes a whole buffer, rejecting trailing bytes.
    fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let value = Self::decode(&mut r)?;
        r.finish()?;
        Ok(value)
    }
}

impl<T: Encode + ?Sized> Encode for &T {
    fn encode(&self, w: &mut Writer) {
        (**self).encode(w);
    }
}

macro_rules! int_codec {
    ($($ty:ident),*) => {$(
        impl Encode for $ty {
            fn encode(&self, w: &mut Writer) {
                w.$ty(*self);
            }
        }
        impl Decode for $ty {
            const MIN_SIZE: usize = std::mem::size_of::<$ty>();
            fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
                r.$ty()
            }
        }
    )*};
}

int_codec!(u8, u16, u32, u64);

/// A one-byte flag that holds 0 or 1.
impl Encode for bool {
    fn encode(&self, w: &mut Writer) {
        w.u8(u8::from(*self));
    }
}

impl Decode for bool {
    const MIN_SIZE: usize = 1;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let start = r.offset();
        match r.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(r.error_at(DecodeErrorKind::UnknownValue, start)),
        }
    }
}

impl<T: Encode> Encode for Option<T> {
    fn encode(&self, w: &mut Writer) {
        match self {
            None => w.u8(0),
            Some(value) => {
                w.u8(1);
                value.encode(w);
            }
        }
    }
}

impl<T: Decode> Decode for Option<T> {
    const MIN_SIZE: usize = 1;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        if r.opt()? {
            Ok(Some(T::decode(r)?))
        } else {
            Ok(None)
        }
    }
}

/// A `bytes` field of at most `MAX` bytes.
#[derive(Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Bytes<const MAX: usize = UNBOUNDED>(
    #[serde(with = "crate::types::hex_vec")] pub Vec<u8>,
);

impl<const MAX: usize> Bytes<MAX> {
    /// The field's limit.
    pub const MAX: usize = MAX;

    /// Wraps `bytes`, refusing more than `MAX`.
    pub fn new(bytes: Vec<u8>) -> Result<Self, DecodeErrorKind> {
        if bytes.len() > MAX {
            return Err(DecodeErrorKind::TooLong);
        }
        Ok(Self(bytes))
    }
}

impl<const MAX: usize> fmt::Debug for Bytes<MAX> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes({} bytes)", self.0.len())
    }
}

impl<const MAX: usize> Deref for Bytes<MAX> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl<const MAX: usize> Encode for Bytes<MAX> {
    fn encode(&self, w: &mut Writer) {
        w.bytes(&self.0);
    }
}

impl<const MAX: usize> Decode for Bytes<MAX> {
    const MIN_SIZE: usize = 4;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self(r.bytes(MAX)?.to_vec()))
    }
}

/// A `bytes` field whose length the spec fixes at `N`.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExactBytes<const N: usize>(#[serde(with = "crate::types::hex_vec")] Vec<u8>);

impl<const N: usize> ExactBytes<N> {
    /// The fixed length.
    pub const LEN: usize = N;

    /// Wraps `bytes`, refusing any length but `N`.
    pub fn new(bytes: Vec<u8>) -> Result<Self, DecodeErrorKind> {
        if bytes.len() != N {
            return Err(DecodeErrorKind::WrongLength);
        }
        Ok(Self(bytes))
    }

    /// The bytes.
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl<const N: usize> fmt::Debug for ExactBytes<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ExactBytes<{N}>")
    }
}

impl<const N: usize> Deref for ExactBytes<N> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl<const N: usize> Encode for ExactBytes<N> {
    fn encode(&self, w: &mut Writer) {
        w.bytes(&self.0);
    }
}

impl<const N: usize> Decode for ExactBytes<N> {
    const MIN_SIZE: usize = 4 + N;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self(r.bytes_exact(N)?.to_vec()))
    }
}

/// A `str` field of at most `MAX` bytes of UTF-8.
#[derive(Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Text<const MAX: usize = STR_MAX>(pub String);

impl<const MAX: usize> Text<MAX> {
    /// The field's limit in bytes.
    pub const MAX: usize = MAX;

    /// Wraps `value`, refusing more than `MAX` bytes.
    pub fn new(value: impl Into<String>) -> Result<Self, DecodeErrorKind> {
        let value = value.into();
        if value.len() > MAX || value.len() > usize::from(u16::MAX) {
            return Err(DecodeErrorKind::TooLong);
        }
        Ok(Self(value))
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<const MAX: usize> fmt::Debug for Text<MAX> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl<const MAX: usize> Deref for Text<MAX> {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl<const MAX: usize> Encode for Text<MAX> {
    fn encode(&self, w: &mut Writer) {
        w.str(&self.0);
    }
}

impl<const MAX: usize> Decode for Text<MAX> {
    const MIN_SIZE: usize = 2;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self(r.str(MAX)?.to_owned()))
    }
}

/// A `list<T>` of at most `MAX` elements, in any order.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct List<T, const MAX: usize = UNBOUNDED>(pub Vec<T>);

impl<T, const MAX: usize> Default for List<T, MAX> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T, const MAX: usize> List<T, MAX> {
    /// The field's limit.
    pub const MAX: usize = MAX;

    /// Wraps `items`, refusing more than `MAX`.
    pub fn new(items: Vec<T>) -> Result<Self, DecodeErrorKind> {
        if items.len() > MAX {
            return Err(DecodeErrorKind::TooLong);
        }
        Ok(Self(items))
    }
}

impl<T: fmt::Debug, const MAX: usize> fmt::Debug for List<T, MAX> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(&self.0).finish()
    }
}

impl<T, const MAX: usize> Deref for List<T, MAX> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T: Encode, const MAX: usize> Encode for List<T, MAX> {
    fn encode(&self, w: &mut Writer) {
        encode_list(w, &self.0);
    }
}

impl<T: Decode, const MAX: usize> Decode for List<T, MAX> {
    const MIN_SIZE: usize = 4;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self(decode_list(r, MAX)?))
    }
}

/// Encodes a `list<T>`.
///
/// # Panics
///
/// Panics for 2^32 elements or more, which no record reaches.
pub fn encode_list<T: Encode>(w: &mut Writer, items: &[T]) {
    let count = u32::try_from(items.len()).expect("a list count fits a u32");
    w.u32(count);
    for item in items {
        item.encode(w);
    }
}

/// Decodes a `list<T>` of at most `max` elements.
pub fn decode_list<T: Decode>(r: &mut Reader<'_>, max: usize) -> Result<Vec<T>, DecodeError> {
    let count = r.count(max, T::MIN_SIZE)?;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        items.push(T::decode(r)?);
    }
    Ok(items)
}

/// The sort key of a sorted list's elements (§4): the element's key fields
/// as bytes, which compare as the spec orders them: IDs, digests and strings
/// as unsigned bytes, fixed-width big-endian integers by value.
pub trait SortSpec<T> {
    /// The key of `item`.
    fn key(item: &T) -> Vec<u8>;
}

/// Sorts by the whole element's encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Whole;

impl<T: Encode> SortSpec<T> for Whole {
    fn key(item: &T) -> Vec<u8> {
        item.to_bytes()
    }
}

/// A `list<T>` that the spec calls sorted: its keys under `S` rise strictly,
/// so it holds no duplicate key.
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub struct Sorted<T, S, const MAX: usize = UNBOUNDED>(
    Vec<T>,
    #[serde(skip)] PhantomData<fn() -> S>,
);

impl<T: Clone, S, const MAX: usize> Clone for Sorted<T, S, MAX> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), PhantomData)
    }
}

impl<T: PartialEq, S, const MAX: usize> PartialEq for Sorted<T, S, MAX> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: Eq, S, const MAX: usize> Eq for Sorted<T, S, MAX> {}

impl<T: std::hash::Hash, S, const MAX: usize> std::hash::Hash for Sorted<T, S, MAX> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<T, S, const MAX: usize> Default for Sorted<T, S, MAX> {
    fn default() -> Self {
        Self(Vec::new(), PhantomData)
    }
}

impl<T: fmt::Debug, S, const MAX: usize> fmt::Debug for Sorted<T, S, MAX> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(&self.0).finish()
    }
}

impl<T, S, const MAX: usize> Deref for Sorted<T, S, MAX> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T, S: SortSpec<T>, const MAX: usize> Sorted<T, S, MAX> {
    /// The field's limit.
    pub const MAX: usize = MAX;

    /// Sorts `items` by key, refusing duplicate keys and more than `MAX`.
    pub fn new(mut items: Vec<T>) -> Result<Self, DecodeErrorKind> {
        if items.len() > MAX {
            return Err(DecodeErrorKind::TooLong);
        }
        items.sort_by_cached_key(|item| S::key(item));
        if !rises_strictly::<T, S>(&items) {
            return Err(DecodeErrorKind::NotSorted);
        }
        Ok(Self(items, PhantomData))
    }

    /// Wraps `items` as they are, unchecked. Only fixtures use it, to build
    /// lists that decoders must refuse.
    pub fn unchecked(items: Vec<T>) -> Self {
        Self(items, PhantomData)
    }

    /// The elements.
    pub fn into_vec(self) -> Vec<T> {
        self.0
    }
}

fn rises_strictly<T, S: SortSpec<T>>(items: &[T]) -> bool {
    items
        .windows(2)
        .all(|pair| S::key(&pair[0]) < S::key(&pair[1]))
}

impl<T: Encode, S, const MAX: usize> Encode for Sorted<T, S, MAX> {
    fn encode(&self, w: &mut Writer) {
        encode_list(w, &self.0);
    }
}

impl<T: Decode, S: SortSpec<T>, const MAX: usize> Decode for Sorted<T, S, MAX> {
    const MIN_SIZE: usize = 4;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let start = r.offset();
        let items = decode_list::<T>(r, MAX)?;
        if !rises_strictly::<T, S>(&items) {
            return Err(r.error_at(DecodeErrorKind::NotSorted, start));
        }
        Ok(Self(items, PhantomData))
    }
}

/// A `bytes` field that holds another encoded value, such as the card inside
/// an account link.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Nested<T>(pub T);

impl<T: fmt::Debug> fmt::Debug for Nested<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<T> Deref for Nested<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Encode> Encode for Nested<T> {
    fn encode(&self, w: &mut Writer) {
        w.bytes(&self.0.to_bytes());
    }
}

impl<T: Decode> Decode for Nested<T> {
    const MIN_SIZE: usize = 4 + T::MIN_SIZE;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let inner = r.bytes(UNBOUNDED)?;
        let base = r.offset() - inner.len();
        T::from_bytes(inner).map(Nested).map_err(|mut e| {
            e.offset += base;
            e
        })
    }
}

/// Declares a struct whose encoding is its fields in order, optionally
/// after a tag, with [`Encode`] and [`Decode`] implementations.
#[macro_export]
#[doc(hidden)]
macro_rules! wire_struct {
    (
        $(#[$meta:meta])*
        pub struct $name:ident $(: tag $tag:ident)? {
            $( $(#[$fmeta:meta])* pub $field:ident : $ty:ty ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        pub struct $name {
            $( $(#[$fmeta])* pub $field: $ty, )*
        }

        impl $crate::codec::Encode for $name {
            fn encode(&self, w: &mut $crate::codec::Writer) {
                $( w.tag($crate::tag::Tag::$tag); )?
                $( $crate::codec::Encode::encode(&self.$field, w); )*
            }
        }

        impl $crate::codec::Decode for $name {
            const MIN_SIZE: usize = 0 $( + $crate::tag::Tag::$tag.encoded_len() )?
                $( + <$ty as $crate::codec::Decode>::MIN_SIZE )*;
            fn decode(
                r: &mut $crate::codec::Reader<'_>,
            ) -> Result<Self, $crate::codec::DecodeError> {
                $( r.tag($crate::tag::Tag::$tag)?; )?
                Ok(Self { $( $field: r.get()?, )* })
            }
        }
    };
}

pub use wire_struct;

/// Declares a one-byte enum with its encoding.
#[macro_export]
#[doc(hidden)]
macro_rules! byte_enum {
    ($(#[$meta:meta])* pub enum $name:ident { $($(#[$vmeta:meta])* $variant:ident = $value:literal,)* }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($(#[$vmeta])* $variant = $value,)*
        }

        impl $name {
            /// The encoded byte.
            pub fn code(self) -> u8 {
                self as u8
            }

            /// The value whose byte is `code`.
            pub fn from_code(code: u8) -> Option<Self> {
                match code {
                    $($value => Some($name::$variant),)*
                    _ => None,
                }
            }
        }

        impl $crate::codec::Encode for $name {
            fn encode(&self, w: &mut $crate::codec::Writer) {
                w.u8(self.code());
            }
        }

        impl $crate::codec::Decode for $name {
            const MIN_SIZE: usize = 1;
            fn decode(r: &mut $crate::codec::Reader<'_>) -> Result<Self, $crate::codec::DecodeError> {
                let offset = r.offset();
                let code = r.u8()?;
                Self::from_code(code).ok_or_else(|| r.error_at($crate::codec::DecodeErrorKind::UnknownValue, offset))
            }
        }
    };
}

pub use byte_enum;
