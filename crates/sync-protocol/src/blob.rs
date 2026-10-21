//! The blob format (§21): a STREAM of 65,536-byte XChaCha20-Poly1305 chunks
//! with a 32-bit big-endian counter and a last-chunk flag, under a nonce
//! prefix derived from the blob's key and ID.
//!
//! ```text
//! header      = tag(noura.sync.blob) || id blob || 19-byte nonce_prefix
//! plaintext   = u64 L || file bytes || zero bytes   total P = padded_length(L + 8)
//! chunk i     = XChaCha20-Poly1305(blob_key, nonce_prefix || u32 i || u8 last,
//!                                  aad = header, 65,536 plaintext bytes)
//! ciphertext  = header || chunk 0 || chunk 1 || ...
//! ```

use aead_stream::{NewStream, StreamBE32, StreamPrimitive};
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{KeyInit, Payload};

use crate::codec::Writer;
use crate::crypto::{AEAD_TAG_LEN, context, derive_key, hash, hash_parts};
use crate::padding::{is_valid_padded_length, padded_length};
use crate::records::BlobRef;
use crate::tag::Tag;
use crate::types::{Digest, Id, SecretKey};

/// Plaintext bytes per chunk.
pub const CHUNK_LEN: usize = 65_536;
/// Ciphertext bytes per segment that `ciphertext_digest` hashes.
pub const SEGMENT_LEN: usize = 1_048_576;
/// The largest file a blob carries, 2 GiB.
pub const MAX_BLOB_FILE: u64 = 1 << 31;
/// The length of the nonce prefix.
pub const NONCE_PREFIX_LEN: usize = 19;

/// The length of a blob header.
pub fn header_len() -> usize {
    Tag::Blob.encoded_len() + 16 + NONCE_PREFIX_LEN
}

/// `nonce_prefix`: the first 19 bytes of
/// `derive_key("noura.sync.v2 blob nonce", blob_key || blob)`.
pub fn nonce_prefix(blob_key: &SecretKey, blob: &Id) -> [u8; NONCE_PREFIX_LEN] {
    let mut material = [0u8; 48];
    material[..32].copy_from_slice(blob_key.expose());
    material[32..].copy_from_slice(&blob.0);
    let derived = derive_key(context::BLOB_NONCE, &material);
    zeroize::Zeroize::zeroize(&mut material);
    let mut prefix = [0u8; NONCE_PREFIX_LEN];
    prefix.copy_from_slice(&derived.expose()[..NONCE_PREFIX_LEN]);
    prefix
}

/// The blob header.
pub fn header(blob: &Id, prefix: &[u8; NONCE_PREFIX_LEN]) -> Vec<u8> {
    let mut w = Writer::new();
    w.tag(Tag::Blob);
    w.put(blob);
    w.raw(prefix);
    w.into_bytes()
}

/// The ciphertext length of a blob whose padded plaintext is `p` bytes.
fn ciphertext_len_for_padded(p: u64) -> u64 {
    header_len() as u64 + p + AEAD_TAG_LEN as u64 * p.div_ceil(CHUNK_LEN as u64)
}

/// The ciphertext length of a blob for a file of `file_len` bytes.
pub fn blob_ciphertext_len(file_len: u64) -> u64 {
    ciphertext_len_for_padded(padded_length(file_len + 8))
}

/// Whether `len` is the ciphertext length of some valid padded plaintext,
/// the check the server makes from the length alone.
pub fn is_valid_blob_ciphertext_len(len: u64) -> bool {
    let Some(body) = len.checked_sub(header_len() as u64) else {
        return false;
    };
    let per_chunk = (CHUNK_LEN + AEAD_TAG_LEN) as u64;
    let chunks = body.div_ceil(per_chunk);
    let Some(p) = body.checked_sub(chunks * AEAD_TAG_LEN as u64) else {
        return false;
    };
    is_valid_padded_length(p) && ciphertext_len_for_padded(p) == len
}

/// `ciphertext_digest`: `BLAKE3(tag(noura.sync.blob.parts) || list<digest>)`
/// over the BLAKE3 hash of each 1 MiB segment of the ciphertext.
pub fn ciphertext_digest(ciphertext: &[u8]) -> Digest {
    let hashes: Vec<Digest> = ciphertext.chunks(SEGMENT_LEN).map(hash).collect();
    digest_of_segment_hashes(&hashes)
}

/// `ciphertext_digest` from segment hashes the server stored as each
/// segment arrived.
pub fn digest_of_segment_hashes(hashes: &[Digest]) -> Digest {
    let mut w = Writer::new();
    w.tag(Tag::BlobParts);
    crate::codec::encode_list(&mut w, hashes);
    hash_parts(&[w.as_bytes()])
}

/// Why a blob failed to encrypt or decrypt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BlobError {
    /// The file exceeds 2 GiB.
    #[error("file exceeds 2 GiB")]
    TooLarge,
    /// The ciphertext length isn't the one the `blob_ref` names.
    #[error("ciphertext length differs from the blob_ref")]
    Length,
    /// The segment hashes don't match `ciphertext_digest`.
    #[error("ciphertext digest mismatch")]
    Digest,
    /// The header doesn't name this blob and key.
    #[error("blob header mismatch")]
    Header,
    /// A chunk fails to decrypt.
    #[error("chunk fails to decrypt")]
    Decrypt,
    /// The length prefix exceeds the padded space, or padding isn't zero.
    #[error("malformed blob padding")]
    Padding,
    /// The decrypted length `L` differs from `plaintext_length`.
    #[error("plaintext_length differs from the file's length")]
    PlaintextLength,
    /// The file's BLAKE3 hash differs from the payload's `revision`.
    #[error("file revision mismatch")]
    Revision,
}

fn stream(key: &SecretKey, prefix: &[u8; NONCE_PREFIX_LEN]) -> StreamBE32<XChaCha20Poly1305> {
    let aead = XChaCha20Poly1305::new(key.expose().into());
    StreamBE32::from_aead(aead, prefix.into())
}

/// Encrypts `file` as blob `blob` under its fresh random `blob_key`,
/// returning the ciphertext and the `blob_ref` that names it. The same file,
/// ID and key always give the same bytes, so a device that holds the file
/// rebuilds the blob byte for byte.
pub fn encrypt_blob(
    file: &[u8],
    blob: Id,
    blob_key: SecretKey,
) -> Result<(Vec<u8>, BlobRef), BlobError> {
    let file_len = file.len() as u64;
    if file_len > MAX_BLOB_FILE {
        return Err(BlobError::TooLarge);
    }
    let p = usize::try_from(padded_length(file_len + 8)).map_err(|_| BlobError::TooLarge)?;
    let mut plaintext = Vec::with_capacity(p);
    plaintext.extend_from_slice(&file_len.to_be_bytes());
    plaintext.extend_from_slice(file);
    plaintext.resize(p, 0);

    let prefix = nonce_prefix(&blob_key, &blob);
    let header = header(&blob, &prefix);
    let cipher = stream(&blob_key, &prefix);
    let chunk_count = p.div_ceil(CHUNK_LEN);
    let mut ciphertext = header.clone();
    ciphertext.reserve(p + chunk_count * AEAD_TAG_LEN);
    for (i, chunk) in plaintext.chunks(CHUNK_LEN).enumerate() {
        let last = i + 1 == chunk_count;
        let sealed = cipher
            .encrypt(
                i as u32,
                last,
                Payload {
                    msg: chunk,
                    aad: &header,
                },
            )
            .map_err(|_| BlobError::TooLarge)?;
        ciphertext.extend_from_slice(&sealed);
    }
    zeroize::Zeroize::zeroize(&mut plaintext);
    let blob_ref = BlobRef {
        blob,
        ciphertext_length: ciphertext.len() as u64,
        ciphertext_digest: ciphertext_digest(&ciphertext),
        blob_key,
        plaintext_length: file_len,
    };
    Ok((ciphertext, blob_ref))
}

/// Decrypts a downloaded blob: checks its length and segment hashes
/// against the `blob_ref` before decrypting, then checks that
/// `plaintext_length` equals the file's length `L` and, when given, that the
/// file's hash equals the payload's `revision`.
pub fn decrypt_blob(
    ciphertext: &[u8],
    blob_ref: &BlobRef,
    revision: Option<&Digest>,
) -> Result<Vec<u8>, BlobError> {
    if ciphertext.len() as u64 != blob_ref.ciphertext_length
        || !is_valid_blob_ciphertext_len(ciphertext.len() as u64)
    {
        return Err(BlobError::Length);
    }
    if ciphertext_digest(ciphertext) != blob_ref.ciphertext_digest {
        return Err(BlobError::Digest);
    }
    let prefix = nonce_prefix(&blob_ref.blob_key, &blob_ref.blob);
    let expected_header = header(&blob_ref.blob, &prefix);
    let (found_header, body) = ciphertext.split_at(expected_header.len());
    if found_header != expected_header {
        return Err(BlobError::Header);
    }
    let cipher = stream(&blob_ref.blob_key, &prefix);
    let sealed_chunk = CHUNK_LEN + AEAD_TAG_LEN;
    let chunk_count = body.len().div_ceil(sealed_chunk);
    let mut plaintext = Vec::with_capacity(body.len());
    for (i, chunk) in body.chunks(sealed_chunk).enumerate() {
        let last = i + 1 == chunk_count;
        let opened = cipher
            .decrypt(
                i as u32,
                last,
                Payload {
                    msg: chunk,
                    aad: &expected_header,
                },
            )
            .map_err(|_| BlobError::Decrypt)?;
        plaintext.extend_from_slice(&opened);
    }
    let file_len = u64::from_be_bytes(plaintext[..8].try_into().expect("at least 256 bytes"));
    let end = usize::try_from(file_len)
        .ok()
        .and_then(|n| n.checked_add(8))
        .filter(|&end| end <= plaintext.len())
        .ok_or(BlobError::Padding)?;
    if plaintext[end..].iter().any(|&b| b != 0) {
        return Err(BlobError::Padding);
    }
    if file_len != blob_ref.plaintext_length {
        return Err(BlobError::PlaintextLength);
    }
    plaintext.truncate(end);
    let file = plaintext.split_off(8);
    if revision.is_some_and(|revision| hash(&file) != *revision) {
        return Err(BlobError::Revision);
    }
    Ok(file)
}
