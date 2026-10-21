//! Published test vectors for this crate's primitives (sync-v2.md §3):
//! Wycheproof's XChaCha20-Poly1305 and Ed25519 cases, and the BLAKE3
//! reference vectors' published outputs.

use sync_protocol::crypto::{CryptoError, open, seal, verify};
use sync_protocol::types::{Key, Nonce, SecretKey, Signature};
use wycheproof::TestResult;

#[test]
fn wycheproof_xchacha20_poly1305() {
    use wycheproof::aead::{TestName, TestSet};
    let set = TestSet::load(TestName::XChaCha20Poly1305).unwrap();
    let mut checked = 0;
    for group in set.test_groups {
        if group.nonce_size != 192 || group.key_size != 256 || group.tag_size != 128 {
            continue;
        }
        for test in group.tests {
            let key = SecretKey(test.key.as_ref().try_into().unwrap());
            let nonce = Nonce(test.nonce.as_ref().try_into().unwrap());
            let mut sealed = test.ct.to_vec();
            sealed.extend_from_slice(&test.tag);
            let opened = open(&key, &nonce, &test.aad, &sealed);
            match test.result {
                TestResult::Valid => {
                    assert_eq!(opened.as_deref(), Ok(test.pt.as_ref()), "tc {}", test.tc_id);
                    assert_eq!(
                        seal(&key, &nonce, &test.aad, &test.pt),
                        sealed,
                        "tc {}",
                        test.tc_id
                    );
                }
                TestResult::Invalid => {
                    assert_eq!(opened, Err(CryptoError::Decrypt), "tc {}", test.tc_id);
                }
                TestResult::Acceptable => {}
            }
            checked += 1;
        }
    }
    assert!(checked > 100, "{checked}");
}

#[test]
fn wycheproof_ed25519_under_verify_strict() {
    use wycheproof::eddsa::{TestName, TestSet};
    let set = TestSet::load(TestName::Ed25519).unwrap();
    let mut checked = 0;
    for group in set.test_groups {
        let public = Key(group.key.pk.as_ref().try_into().unwrap());
        for test in group.tests {
            let result = <[u8; 64]>::try_from(test.sig.as_ref())
                .map_err(|_| CryptoError::Signature)
                .and_then(|sig| verify(&public, &test.msg, &Signature(sig)));
            match test.result {
                TestResult::Valid => assert_eq!(result, Ok(()), "tc {}", test.tc_id),
                TestResult::Invalid => assert!(result.is_err(), "tc {}", test.tc_id),
                TestResult::Acceptable => {}
            }
            checked += 1;
        }
    }
    assert!(checked > 100, "{checked}");
}

/// Outputs from the BLAKE3 reference test vectors
/// (BLAKE3-team/BLAKE3, `test_vectors/test_vectors.json`): the input is
/// bytes 0, 1, ..., 250 repeated to the given length, the key is "whats the
/// Elvish word for friend", and the context is "BLAKE3 2019-12-27 16:29:52
/// test vectors context". Each output is the first 32 bytes of the
/// extended output the file lists.
const BLAKE3_KEY: &[u8; 32] = b"whats the Elvish word for friend";
const BLAKE3_CONTEXT: &str = "BLAKE3 2019-12-27 16:29:52 test vectors context";
const BLAKE3_VECTORS: &[(usize, &str, &str, &str)] = &[(
    0,
    "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
    "92b2b75604ed3c761f9d6f62392c8a9227ad0ea3f09573e783f1498a4ed60d26",
    "2cc39783c223154fea8dfb7c1b1660f2ac2dcbd1c1de8277b0b0dd39b7e50d7d",
)];

/// Hash outputs of further reference lengths.
const BLAKE3_HASHES: &[(usize, &str)] = &[(
    1,
    "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213",
)];

#[test]
fn blake3_reference_hashes() {
    for (len, hash) in BLAKE3_HASHES {
        let input: Vec<u8> = (0..*len).map(|i| (i % 251) as u8).collect();
        assert_eq!(blake3::hash(&input).to_hex().as_str(), *hash, "{len}");
    }
}

#[test]
fn blake3_reference_vectors() {
    for (len, hash, keyed, derived) in BLAKE3_VECTORS {
        let input: Vec<u8> = (0..*len).map(|i| (i % 251) as u8).collect();
        assert_eq!(blake3::hash(&input).to_hex().as_str(), *hash, "{len}");
        assert_eq!(
            blake3::keyed_hash(BLAKE3_KEY, &input).to_hex().as_str(),
            *keyed,
            "{len}"
        );
        assert_eq!(
            sync_protocol::types::to_hex(&blake3::derive_key(BLAKE3_CONTEXT, &input)),
            *derived,
            "{len}"
        );
    }
}
