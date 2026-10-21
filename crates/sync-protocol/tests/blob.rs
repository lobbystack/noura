//! The blob format of §21.

use sync_protocol::blob::{
    BlobError, CHUNK_LEN, SEGMENT_LEN, blob_ciphertext_len, ciphertext_digest, decrypt_blob,
    encrypt_blob, is_valid_blob_ciphertext_len, nonce_prefix,
};
use sync_protocol::crypto::{context, derive_key, hash};
use sync_protocol::padding::padded_length;
use sync_protocol::tag::Tag;
use sync_protocol::types::{Id, SecretKey};

fn file(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

const BLOB: Id = Id([0x42; 16]);

fn key() -> SecretKey {
    SecretKey([0x24; 32])
}

#[test]
fn the_nonce_prefix_is_the_first_19_bytes_of_the_derived_key() {
    let mut material = key().0.to_vec();
    material.extend_from_slice(&BLOB.0);
    let derived = derive_key(context::BLOB_NONCE, &material);
    assert_eq!(nonce_prefix(&key(), &BLOB), derived.0[..19]);
}

#[test]
fn the_ciphertext_starts_with_the_header_and_has_the_stated_length() {
    for len in [0usize, 1, 100, 65_528, 65_529, 200_000, 2 * 1024 * 1024] {
        let (ciphertext, blob_ref) = encrypt_blob(&file(len), BLOB, key()).unwrap();
        let mut header = Tag::Blob.bytes();
        header.extend_from_slice(&BLOB.0);
        header.extend_from_slice(&nonce_prefix(&key(), &BLOB));
        assert_eq!(&ciphertext[..header.len()], &header[..]);
        let p = padded_length(len as u64 + 8);
        let expected = header.len() as u64 + p + 16 * p.div_ceil(CHUNK_LEN as u64);
        assert_eq!(ciphertext.len() as u64, expected, "{len}");
        assert_eq!(blob_ciphertext_len(len as u64), expected);
        assert!(is_valid_blob_ciphertext_len(expected));
        assert!(!is_valid_blob_ciphertext_len(expected + 1));
        assert_eq!(blob_ref.ciphertext_length, expected);
        assert_eq!(blob_ref.plaintext_length, len as u64);
    }
}

#[test]
fn the_ciphertext_digest_covers_the_hashes_of_1_mib_segments() {
    let (ciphertext, blob_ref) = encrypt_blob(&file(3 * 1024 * 1024), BLOB, key()).unwrap();
    let mut input = Tag::BlobParts.bytes();
    let segments: Vec<_> = ciphertext.chunks(SEGMENT_LEN).collect();
    assert_eq!(segments.len(), 4);
    input.extend_from_slice(&(segments.len() as u32).to_be_bytes());
    for segment in segments {
        input.extend_from_slice(&hash(segment).0);
    }
    assert_eq!(ciphertext_digest(&ciphertext), hash(&input));
    assert_eq!(blob_ref.ciphertext_digest, hash(&input));
}

#[test]
fn a_blob_decrypts_and_rebuilds_byte_for_byte() {
    let bytes = file(300_000);
    let (ciphertext, blob_ref) = encrypt_blob(&bytes, BLOB, key()).unwrap();
    let revision = hash(&bytes);
    assert_eq!(
        decrypt_blob(&ciphertext, &blob_ref, Some(&revision)).unwrap(),
        bytes
    );
    let (rebuilt, rebuilt_ref) = encrypt_blob(&bytes, BLOB, key()).unwrap();
    assert_eq!(rebuilt, ciphertext);
    assert_eq!(rebuilt_ref, blob_ref);
}

#[test]
fn a_plaintext_length_that_differs_from_the_file_fails() {
    let (ciphertext, mut blob_ref) = encrypt_blob(&file(1_000), BLOB, key()).unwrap();
    blob_ref.plaintext_length = 999;
    assert_eq!(
        decrypt_blob(&ciphertext, &blob_ref, None),
        Err(BlobError::PlaintextLength)
    );
}

#[test]
fn tampering_fails_before_decryption() {
    let (mut ciphertext, blob_ref) = encrypt_blob(&file(1_000), BLOB, key()).unwrap();
    let last = ciphertext.len() - 1;
    ciphertext[last] ^= 1;
    assert_eq!(
        decrypt_blob(&ciphertext, &blob_ref, None),
        Err(BlobError::Digest)
    );
    let (ciphertext, blob_ref) = encrypt_blob(&file(1_000), BLOB, key()).unwrap();
    assert_eq!(
        decrypt_blob(&ciphertext[..ciphertext.len() - 1], &blob_ref, None),
        Err(BlobError::Length)
    );
    let wrong = hash(b"other");
    assert_eq!(
        decrypt_blob(&ciphertext, &blob_ref, Some(&wrong)),
        Err(BlobError::Revision)
    );
}

#[test]
fn a_wrong_key_fails_to_decrypt() {
    let (ciphertext, mut blob_ref) = encrypt_blob(&file(1_000), BLOB, key()).unwrap();
    blob_ref.blob_key = SecretKey([1; 32]);
    assert_eq!(
        decrypt_blob(&ciphertext, &blob_ref, None),
        Err(BlobError::Header)
    );
}
