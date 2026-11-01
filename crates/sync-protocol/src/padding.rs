//! Padmé padding (§6).
//!
//! A padded plaintext is `u32 n || content || zero bytes`, where `n` is the
//! content length and the total is `padded_length(n + 4)`. Blobs use a `u64`
//! prefix instead (§21). Unpadding runs after AEAD verification and before
//! any parsing.

/// The smallest padded length.
pub const MIN_PADDED_LEN: u64 = 256;

/// `padme(L)`: rounds `L` up so that only its top `floor(log2(E)) + 1` bits,
/// where `E = floor(log2(L))`, may be nonzero.
pub fn padme(length: u64) -> u64 {
    if length < 2 {
        return length;
    }
    let e = u64::from(63 - length.leading_zeros());
    let s = u64::from(63 - e.leading_zeros()) + 1;
    let mask = (1u64 << (e - s)) - 1;
    (length + mask) & !mask
}

/// `max(256, padme(L))`.
pub fn padded_length(length: u64) -> u64 {
    padme(length).max(MIN_PADDED_LEN)
}

/// Whether `x` is a length that padding produces: at least 256 and fixed by
/// `padme`.
pub fn is_valid_padded_length(x: u64) -> bool {
    x >= MIN_PADDED_LEN && padme(x) == x
}

/// Why unpadding refused its input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PaddingError {
    /// The total isn't a valid padded length.
    #[error("not a valid padded length")]
    Length,
    /// The content length `n` exceeds the space after the prefix.
    #[error("content length exceeds the padded space")]
    Oversized,
    /// A padding byte isn't zero.
    #[error("nonzero padding")]
    NonzeroPadding,
    /// The content is too long for a `u32` prefix.
    #[error("content too long to pad")]
    TooLong,
}

/// Pads `content` with a `u32` length prefix.
pub fn pad(content: &[u8]) -> Result<Vec<u8>, PaddingError> {
    let n = u32::try_from(content.len()).map_err(|_| PaddingError::TooLong)?;
    let total =
        usize::try_from(padded_length(u64::from(n) + 4)).map_err(|_| PaddingError::TooLong)?;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&n.to_be_bytes());
    out.extend_from_slice(content);
    out.resize(total, 0);
    Ok(out)
}

/// Removes padding with a `u32` length prefix, checking the total length,
/// the prefix and every padding byte before returning the content.
pub fn unpad(padded: &[u8]) -> Result<&[u8], PaddingError> {
    if !is_valid_padded_length(padded.len() as u64) {
        return Err(PaddingError::Length);
    }
    let n = u32::from_be_bytes(padded[..4].try_into().expect("at least 256 bytes")) as usize;
    let rest = &padded[4..];
    if n > rest.len() {
        return Err(PaddingError::Oversized);
    }
    let (content, padding) = rest.split_at(n);
    if padding.iter().any(|&b| b != 0) {
        return Err(PaddingError::NonzeroPadding);
    }
    Ok(content)
}

/// Whether an AEAD ciphertext's length minus its 16-byte tag is a valid
/// padded length, the check the server runs without reading content.
pub fn is_padded_ciphertext_length(ciphertext_len: u64) -> bool {
    ciphertext_len
        .checked_sub(crate::crypto::AEAD_TAG_LEN as u64)
        .is_some_and(is_valid_padded_length)
}
