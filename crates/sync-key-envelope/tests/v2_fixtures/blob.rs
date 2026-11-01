//! `sync-v2-blob.json`: the blob format (§21).

use serde_json::json;
use sync_protocol::blob::{
    CHUNK_LEN, blob_ciphertext_len, ciphertext_digest, encrypt_blob, header, nonce_prefix,
};
use sync_protocol::crypto::{context, hash};
use sync_protocol::padding::padded_length;
use sync_protocol::types::to_hex;

use super::common::{Fixture, id, secret};

pub fn fixture() -> Fixture {
    let mut f = Fixture::new(
        "sync-v2-blob.json",
        "Blobs (sync-v2.md §21): the header and nonce prefix, chunk nonces, padded and ciphertext lengths, the segment digest, a blob rebuilt byte for byte from its file and blob_ref, and a plaintext_length that differs from the file's length.",
    );
    let blob = id("blob");
    let key = secret("blob key");
    let prefix = nonce_prefix(&key, &blob);
    let mut material = key.0.to_vec();
    material.extend_from_slice(&blob.0);
    f.case(json!({
        "kind": "nonce_prefix",
        "name": "nonce_prefix",
        "context": context::BLOB_NONCE,
        "blob_key": to_hex(key.expose()),
        "blob": blob.to_hex(),
        "expect": to_hex(&prefix),
    }));
    f.case(json!({
        "kind": "blob_header",
        "name": "header",
        "blob_key": to_hex(key.expose()),
        "blob": blob.to_hex(),
        "expect": to_hex(&header(&blob, &prefix)),
    }));
    for (i, last) in [(0u32, false), (1, false), (2, true)] {
        let mut nonce = prefix.to_vec();
        nonce.extend_from_slice(&i.to_be_bytes());
        nonce.push(u8::from(last));
        f.case(json!({"kind": "chunk_nonce", "name": format!("chunk_{i}"), "prefix": to_hex(&prefix), "index": i, "last": last, "expect": to_hex(&nonce)}));
    }
    for len in [
        0u64,
        1,
        248,
        249,
        65_528,
        65_529,
        200_000,
        2_097_152,
        2_147_483_648,
    ] {
        let p = padded_length(len + 8);
        f.case(json!({
            "kind": "blob_lengths",
            "name": format!("file_of_{len}_bytes"),
            "file_length": len,
            "padded": p,
            "chunks": p.div_ceil(CHUNK_LEN as u64),
            "ciphertext_length": blob_ciphertext_len(len),
        }));
    }
    let file: Vec<u8> = (0..1_000u32).map(|i| (i % 251) as u8).collect();
    let (ciphertext, blob_ref) = encrypt_blob(&file, blob, key.clone()).unwrap();
    f.case(json!({
        "kind": "blob",
        "name": "a_1000_byte_file",
        "file": to_hex(&file),
        "revision": hash(&file).to_hex(),
        "blob_ref": serde_json::to_value(&blob_ref).unwrap(),
        "ciphertext": to_hex(&ciphertext),
        "expect": "ok",
    }));
    let mut wrong_length = blob_ref.clone();
    wrong_length.plaintext_length = 999;
    f.case(json!({
        "kind": "blob",
        "name": "plaintext_length_that_differs_from_the_file",
        "file": to_hex(&file),
        "revision": hash(&file).to_hex(),
        "blob_ref": serde_json::to_value(&wrong_length).unwrap(),
        "ciphertext": to_hex(&ciphertext),
        "expect": "plaintext_length",
    }));
    let mut tampered = ciphertext.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    f.case(json!({
        "kind": "blob",
        "name": "tampered_segment",
        "file": to_hex(&file),
        "revision": hash(&file).to_hex(),
        "blob_ref": serde_json::to_value(&blob_ref).unwrap(),
        "ciphertext": to_hex(&tampered),
        "expect": "digest",
    }));
    let big: Vec<u8> = (0..(2 * 1024 * 1024 + 100u32))
        .map(|i| (i % 253) as u8)
        .collect();
    let (big_ciphertext, big_ref) =
        encrypt_blob(&big, id("big blob"), secret("big blob key")).unwrap();
    f.case(json!({
        "kind": "blob_digest",
        "name": "segments_of_1_mib",
        "file_pattern": "byte i is i mod 253",
        "file_length": big.len(),
        "blob": id("big blob").to_hex(),
        "blob_key": to_hex(secret("big blob key").expose()),
        "segments": big_ciphertext.len().div_ceil(1_048_576),
        "ciphertext_length": big_ciphertext.len(),
        "expect": ciphertext_digest(&big_ciphertext).to_hex(),
    }));
    assert_eq!(
        big_ref.ciphertext_digest,
        ciphertext_digest(&big_ciphertext)
    );
    f
}
