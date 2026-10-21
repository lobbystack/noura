//! Fixed-size field types (§4) and their text forms (§5).
//!
//! In URL paths, query parameters and headers, binary values are base64url
//! without padding, and integers are unsigned decimal with no sign or leading
//! zeros.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::codec::{Decode, DecodeError, DecodeErrorKind, Encode, Reader, Writer};

/// Lowercase hexadecimal.
pub fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

/// Parses lowercase or uppercase hexadecimal.
pub fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some((high * 16 + low) as u8)
        })
        .collect()
}

/// Serde helpers that write a byte vector as a hexadecimal string.
pub mod hex_vec {
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serializes `bytes` as hexadecimal.
    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::to_hex(bytes))
    }

    /// Deserializes hexadecimal.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        super::from_hex(&text).ok_or_else(|| serde::de::Error::custom("invalid hexadecimal"))
    }
}

/// Encodes `bytes` as base64url without padding (RFC 4648 §5).
pub fn to_base64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Why a text form failed to parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TextFormError {
    /// The text isn't canonical base64url without padding.
    #[error("not base64url without padding")]
    Base64,
    /// The decoded value has the wrong length.
    #[error("wrong length")]
    Length,
    /// The text isn't an unsigned decimal integer without leading zeros.
    #[error("not a canonical decimal integer")]
    Decimal,
    /// An ID field holds the reserved all-zero value.
    #[error("all-zero ID")]
    ZeroId,
}

/// Decodes base64url without padding into exactly `N` bytes. It refuses
/// padding, the standard alphabet's `+` and `/`, and non-canonical trailing
/// bits, so each value has one text form.
pub fn from_base64url<const N: usize>(text: &str) -> Result<[u8; N], TextFormError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| TextFormError::Base64)?;
    bytes.try_into().map_err(|_| TextFormError::Length)
}

/// Formats an integer as unsigned decimal.
pub fn to_decimal(value: u64) -> String {
    value.to_string()
}

/// Parses unsigned decimal with no sign, no leading zeros and no overflow.
pub fn from_decimal(text: &str) -> Result<u64, TextFormError> {
    let digits = text.as_bytes();
    if digits.is_empty()
        || !digits.iter().all(u8::is_ascii_digit)
        || (digits.len() > 1 && digits[0] == b'0')
    {
        return Err(TextFormError::Decimal);
    }
    text.parse().map_err(|_| TextFormError::Decimal)
}

macro_rules! fixed_bytes {
    ($(#[$meta:meta])* $name:ident, $len:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub [u8; $len]);

        impl $name {
            /// The length in bytes.
            pub const LEN: usize = $len;
            /// The all-zero value.
            pub const ZERO: Self = Self([0; $len]);

            /// Whether every byte is zero.
            pub fn is_zero(&self) -> bool {
                self.0 == [0; $len]
            }

            /// The value as base64url without padding.
            pub fn to_base64url(&self) -> String {
                to_base64url(&self.0)
            }

            /// Parses base64url without padding.
            pub fn from_base64url(text: &str) -> Result<Self, TextFormError> {
                from_base64url(text).map(Self)
            }

            /// The value as lowercase hexadecimal.
            pub fn to_hex(&self) -> String {
                to_hex(&self.0)
            }

            /// Parses hexadecimal.
            pub fn from_hex(text: &str) -> Option<Self> {
                from_hex(text)?.try_into().ok().map(Self)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.to_hex())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.to_hex())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let text = String::deserialize(d)?;
                Self::from_hex(&text)
                    .ok_or_else(|| serde::de::Error::custom(concat!("invalid ", stringify!($name))))
            }
        }

        impl AsRef<[u8]> for $name {
            fn as_ref(&self) -> &[u8] {
                &self.0
            }
        }
    };
}

fixed_bytes!(
    /// A 16-byte remote ID (§5). Decoders refuse the all-zero value, which is
    /// reserved; [`IdOrZero`] is the one field type that admits it.
    Id,
    16
);
fixed_bytes!(
    /// A 16-byte ID field that may hold the all-zero value, as an error
    /// frame's IDs do for a refused subscribe (§23).
    IdOrZero,
    16
);
fixed_bytes!(
    /// A 32-byte BLAKE3-256 output.
    Digest,
    32
);
fixed_bytes!(
    /// A 32-byte public value: an Ed25519 public key, a challenge or a
    /// nonce that both sides see.
    Key,
    32
);
fixed_bytes!(
    /// A 24-byte XChaCha20-Poly1305 nonce.
    Nonce,
    24
);
fixed_bytes!(
    /// A 64-byte Ed25519 signature.
    Signature,
    64
);

impl Id {
    /// A fresh random ID from the operating system's CSPRNG (§5). It carries
    /// no timestamp, version bits, type or local object ID, and is never the
    /// reserved all-zero value.
    pub fn random() -> Result<Self, getrandom::Error> {
        loop {
            let id = Self(crate::crypto::random_array()?);
            if !id.is_zero() {
                return Ok(id);
            }
        }
    }
}

macro_rules! fixed_codec {
    ($($name:ident),*) => {$(
        impl Encode for $name {
            fn encode(&self, w: &mut Writer) {
                w.raw(&self.0);
            }
        }
    )*};
}

fixed_codec!(Id, IdOrZero, Digest, Key, Nonce, Signature);

impl Decode for Id {
    const MIN_SIZE: usize = 16;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let start = r.offset();
        let id = Self(r.array()?);
        if id.is_zero() {
            return Err(r.error_at(DecodeErrorKind::ZeroId, start));
        }
        Ok(id)
    }
}

macro_rules! plain_decode {
    ($($name:ident),*) => {$(
        impl Decode for $name {
            const MIN_SIZE: usize = $name::LEN;
            fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
                Ok(Self(r.array()?))
            }
        }
    )*};
}

plain_decode!(IdOrZero, Digest, Key, Nonce, Signature);

/// A 32-byte secret: an epoch key, a blob key, a link key or a seed. It
/// zeroes its bytes when dropped and never prints them.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct SecretKey(pub [u8; 32]);

impl SecretKey {
    /// A fresh random secret from the operating system's CSPRNG.
    pub fn random() -> Result<Self, getrandom::Error> {
        Ok(Self(crate::crypto::random_array()?))
    }

    /// The secret's bytes.
    pub fn expose(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretKey(..)")
    }
}

impl std::hash::Hash for SecretKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

/// Fixtures carry test secrets in the clear, as hexadecimal.
impl Serialize for SecretKey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&to_hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for SecretKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        let bytes: [u8; 32] = from_hex(&text)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| serde::de::Error::custom("invalid SecretKey"))?;
        Ok(Self(bytes))
    }
}

impl Encode for SecretKey {
    fn encode(&self, w: &mut Writer) {
        w.raw(&self.0);
    }
}

impl Decode for SecretKey {
    const MIN_SIZE: usize = 32;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self(r.array()?))
    }
}
