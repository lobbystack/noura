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

use std::io::{Read, Write};

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
    /// The file yielded more or fewer bytes than its length while it was
    /// read, as when it changes meanwhile.
    #[error("the file changed while it was read")]
    FileChanged,
    /// Reading or writing the stream failed.
    #[error("blob stream I/O failed: {0:?}")]
    Io(std::io::ErrorKind),
}

fn stream(key: &SecretKey, prefix: &[u8; NONCE_PREFIX_LEN]) -> StreamBE32<XChaCha20Poly1305> {
    let aead = XChaCha20Poly1305::new(key.expose().into());
    StreamBE32::from_aead(aead, prefix.into())
}

/// Hashes ciphertext in 1 MiB segments as it passes, for
/// `ciphertext_digest`.
struct SegmentHasher {
    hashes: Vec<Digest>,
    current: blake3::Hasher,
    filled: usize,
    total: u64,
}

impl SegmentHasher {
    fn new() -> Self {
        Self {
            hashes: Vec::new(),
            current: blake3::Hasher::new(),
            filled: 0,
            total: 0,
        }
    }

    fn update(&mut self, mut bytes: &[u8]) {
        self.total += bytes.len() as u64;
        while !bytes.is_empty() {
            let take = (SEGMENT_LEN - self.filled).min(bytes.len());
            self.current.update(&bytes[..take]);
            self.filled += take;
            bytes = &bytes[take..];
            if self.filled == SEGMENT_LEN {
                let full = std::mem::replace(&mut self.current, blake3::Hasher::new());
                self.hashes.push(Digest(*full.finalize().as_bytes()));
                self.filled = 0;
            }
        }
    }

    fn finish(mut self) -> (Digest, u64) {
        if self.filled > 0 {
            self.hashes
                .push(Digest(*self.current.finalize().as_bytes()));
        }
        (digest_of_segment_hashes(&self.hashes), self.total)
    }
}

fn io(error: std::io::Error) -> BlobError {
    BlobError::Io(error.kind())
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
    let mut ciphertext = Vec::new();
    let (blob_ref, _) = encrypt_blob_to(file, file.len() as u64, blob, blob_key, &mut ciphertext)?;
    Ok((ciphertext, blob_ref))
}

/// Encrypts the `file_len` bytes `file` yields as blob `blob` under
/// `blob_key`, writing the ciphertext to `out` one 65,536-byte chunk at a
/// time, so memory stays bounded whatever the file's size. Returns the
/// `blob_ref` and the BLAKE3 hash of the bytes it read. A file that yields
/// more or fewer than `file_len` bytes fails with
/// [`BlobError::FileChanged`]. The bytes equal [`encrypt_blob`]'s.
pub fn encrypt_blob_to(
    mut file: impl Read,
    file_len: u64,
    blob: Id,
    blob_key: SecretKey,
    mut out: impl Write,
) -> Result<(BlobRef, Digest), BlobError> {
    if file_len > MAX_BLOB_FILE {
        return Err(BlobError::TooLarge);
    }
    let p = padded_length(file_len + 8);
    let prefix = nonce_prefix(&blob_key, &blob);
    let header = header(&blob, &prefix);
    let cipher = stream(&blob_key, &prefix);
    let chunk_count = p.div_ceil(CHUNK_LEN as u64);
    let mut segments = SegmentHasher::new();
    let mut file_hash = blake3::Hasher::new();
    out.write_all(&header).map_err(io)?;
    segments.update(&header);
    let mut chunk = zeroize::Zeroizing::new(vec![0u8; CHUNK_LEN]);
    let mut remaining = file_len;
    for i in 0..chunk_count {
        let len = (p - i * CHUNK_LEN as u64).min(CHUNK_LEN as u64) as usize;
        let mut filled = 0;
        if i == 0 {
            chunk[..8].copy_from_slice(&file_len.to_be_bytes());
            filled = 8;
        }
        let take = ((len - filled) as u64).min(remaining) as usize;
        file.read_exact(&mut chunk[filled..filled + take])
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::UnexpectedEof => BlobError::FileChanged,
                _ => io(error),
            })?;
        file_hash.update(&chunk[filled..filled + take]);
        remaining -= take as u64;
        filled += take;
        chunk[filled..len].fill(0);
        let sealed = cipher
            .encrypt(
                i as u32,
                i + 1 == chunk_count,
                Payload {
                    msg: &chunk[..len],
                    aad: &header,
                },
            )
            .map_err(|_| BlobError::TooLarge)?;
        out.write_all(&sealed).map_err(io)?;
        segments.update(&sealed);
    }
    let mut extra = [0u8; 1];
    match file.read(&mut extra) {
        Ok(0) => {}
        Ok(_) => return Err(BlobError::FileChanged),
        Err(error) => return Err(io(error)),
    }
    let (ciphertext_digest, ciphertext_length) = segments.finish();
    let blob_ref = BlobRef {
        blob,
        ciphertext_length,
        ciphertext_digest,
        blob_key,
        plaintext_length: file_len,
    };
    Ok((blob_ref, Digest(*file_hash.finalize().as_bytes())))
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
    let mut file = Vec::new();
    let file_hash = decrypt_blob_to(ciphertext, blob_ref, &mut file)?;
    if revision.is_some_and(|revision| file_hash != *revision) {
        return Err(BlobError::Revision);
    }
    Ok(file)
}

/// Decrypts the ciphertext `ciphertext` yields, whose segment hashes the
/// caller already checked against `ciphertext_digest`, one chunk at a time,
/// and writes the file's bytes to `out`. Checks the header, every chunk,
/// the padding and `plaintext_length`, and returns the file's BLAKE3 hash
/// for the caller to compare with the payload's `revision`. On an error,
/// `out` may hold part of the file, so the caller discards it.
pub fn decrypt_blob_to(
    mut ciphertext: impl Read,
    blob_ref: &BlobRef,
    mut out: impl Write,
) -> Result<Digest, BlobError> {
    let length = blob_ref.ciphertext_length;
    if !is_valid_blob_ciphertext_len(length) {
        return Err(BlobError::Length);
    }
    let read = |reader: &mut dyn Read, buffer: &mut [u8]| {
        reader
            .read_exact(buffer)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::UnexpectedEof => BlobError::Length,
                _ => io(error),
            })
    };
    let prefix = nonce_prefix(&blob_ref.blob_key, &blob_ref.blob);
    let expected_header = header(&blob_ref.blob, &prefix);
    let mut found_header = vec![0u8; expected_header.len()];
    read(&mut ciphertext, &mut found_header)?;
    if found_header != expected_header {
        return Err(BlobError::Header);
    }
    let cipher = stream(&blob_ref.blob_key, &prefix);
    let sealed_chunk = (CHUNK_LEN + AEAD_TAG_LEN) as u64;
    let body = length - expected_header.len() as u64;
    let chunk_count = body.div_ceil(sealed_chunk);
    let mut sealed = vec![0u8; sealed_chunk as usize];
    let mut file_hash = blake3::Hasher::new();
    // The file's length, and how many of its bytes are still to come.
    let mut file_len = 0u64;
    let mut remaining = 0u64;
    let mut padding_ok = true;
    for i in 0..chunk_count {
        let len = (body - i * sealed_chunk).min(sealed_chunk) as usize;
        read(&mut ciphertext, &mut sealed[..len])?;
        let opened = zeroize::Zeroizing::new(
            cipher
                .decrypt(
                    i as u32,
                    i + 1 == chunk_count,
                    Payload {
                        msg: &sealed[..len],
                        aad: &expected_header,
                    },
                )
                .map_err(|_| BlobError::Decrypt)?,
        );
        let mut start = 0;
        if i == 0 {
            file_len = u64::from_be_bytes(opened[..8].try_into().expect("at least 256 bytes"));
            let padded = body - chunk_count * AEAD_TAG_LEN as u64;
            if file_len.checked_add(8).is_none_or(|end| end > padded) {
                return Err(BlobError::Padding);
            }
            remaining = file_len;
            start = 8;
        }
        let take = ((opened.len() - start) as u64).min(remaining) as usize;
        let bytes = &opened[start..start + take];
        file_hash.update(bytes);
        out.write_all(bytes).map_err(io)?;
        remaining -= take as u64;
        padding_ok &= opened[start + take..].iter().all(|&b| b == 0);
    }
    let mut extra = [0u8; 1];
    match ciphertext.read(&mut extra) {
        Ok(0) => {}
        Ok(_) => return Err(BlobError::Length),
        Err(error) => return Err(io(error)),
    }
    if !padding_ok {
        return Err(BlobError::Padding);
    }
    if file_len != blob_ref.plaintext_length {
        return Err(BlobError::PlaintextLength);
    }
    Ok(Digest(*file_hash.finalize().as_bytes()))
}
