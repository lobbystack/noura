//! Published test vectors for the primitives this crate adds (sync-v2.md
//! §3): the draft-ietf-hpke-pq base-mode vector for X-Wing, HKDF-SHA256 and
//! ChaCha20Poly1305 that ships with `hpke` 0.14.1, and Wycheproof's X25519
//! cases, low-order points included. docs/provenance/hpke-pq-vectors.md
//! records where the HPKE vector comes from.

use hpke::aead::ChaCha20Poly1305;
use hpke::kdf::HkdfSha256;
use hpke::kem::XWing;
use hpke::{Deserializable, Kem as _, OpModeR, OpModeS, Serializable};
use serde::Deserialize;

fn hex(text: &str) -> Vec<u8> {
    sync_protocol::types::from_hex(text).expect("hexadecimal")
}

#[derive(Deserialize)]
struct Encryption {
    aad: String,
    ct: String,
    pt: String,
}

#[derive(Deserialize)]
struct Export {
    #[serde(rename = "L")]
    len: usize,
    exported_value: String,
    exporter_context: String,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct Vector {
    mode: u8,
    kem_id: u16,
    kdf_id: u16,
    aead_id: u16,
    info: String,
    ikmR: String,
    ikmE: String,
    skRm: String,
    pkRm: String,
    enc: String,
    encryptions: Vec<Encryption>,
    exports: Vec<Export>,
}

/// Yields fixed bytes, so encapsulation reproduces the vector's `enc`.
struct FixedRng(Vec<u8>);

impl rand_core::TryRng for FixedRng {
    type Error = std::convert::Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        unreachable!("X-Wing draws bytes")
    }
    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        unreachable!("X-Wing draws bytes")
    }
    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        assert!(dst.len() <= self.0.len(), "the vector's randomness ran out");
        let rest = self.0.split_off(dst.len());
        dst.copy_from_slice(&self.0);
        self.0 = rest;
        Ok(())
    }
}

impl rand_core::TryCryptoRng for FixedRng {}

const VECTORS: &str =
    include_str!("vectors/hpke-pq-6433c8f-xwing-hkdfsha256-chacha20poly1305.json");

#[test]
fn the_hpke_pq_base_mode_vector_for_our_suite_passes() {
    let vectors: Vec<Vector> = serde_json::from_str(VECTORS).unwrap();
    assert_eq!(vectors.len(), 1);
    for v in vectors {
        assert_eq!(
            (v.mode, v.kem_id, v.kdf_id, v.aead_id),
            (0, 0x647a, 0x0001, 0x0003)
        );
        let (sk, pk) = XWing::derive_keypair(&hex(&v.ikmR));
        assert_eq!(sk.to_bytes().to_vec(), hex(&v.skRm));
        assert_eq!(pk.to_bytes().to_vec(), hex(&v.pkRm));
        // The X-Wing secret key is its 32-byte seed (§7).
        let from_seed = <XWing as hpke::Kem>::PrivateKey::from_bytes(&hex(&v.skRm)).unwrap();
        assert_eq!(
            XWing::sk_to_pk(&from_seed).to_bytes().to_vec(),
            hex(&v.pkRm)
        );

        let info = hex(&v.info);
        let mut rng = FixedRng(hex(&v.ikmE));
        let (enc, mut sender) = hpke::setup_sender_with_rng::<ChaCha20Poly1305, HkdfSha256, XWing>(
            &OpModeS::Base,
            &pk,
            &info,
            &mut rng,
        )
        .unwrap();
        assert_eq!(enc.to_bytes().to_vec(), hex(&v.enc));
        let mut receiver = hpke::setup_receiver::<ChaCha20Poly1305, HkdfSha256, XWing>(
            &OpModeR::Base,
            &sk,
            &<XWing as hpke::Kem>::EncappedKey::from_bytes(&hex(&v.enc)).unwrap(),
            &info,
        )
        .unwrap();
        for e in &v.encryptions {
            let ct = sender.seal(&hex(&e.pt), &hex(&e.aad)).unwrap();
            assert_eq!(ct, hex(&e.ct));
            assert_eq!(
                receiver.open(&hex(&e.ct), &hex(&e.aad)).unwrap(),
                hex(&e.pt)
            );
        }
        for x in &v.exports {
            let mut out = vec![0u8; x.len];
            receiver
                .export(&hex(&x.exporter_context), &mut out)
                .unwrap();
            assert_eq!(out, hex(&x.exported_value));
        }
    }
}

#[test]
fn wycheproof_x25519_including_low_order_points() {
    use wycheproof::TestResult;
    use wycheproof::xdh::{TestFlag, TestName, TestSet};
    use x25519_dalek_v3::{PublicKey, StaticSecret};

    let set = TestSet::load(TestName::X25519).unwrap();
    let mut low_order = 0;
    let mut checked = 0;
    for group in set.test_groups {
        for test in group.tests {
            let (Ok(private), Ok(public)) = (
                <[u8; 32]>::try_from(test.private_key.as_ref()),
                <[u8; 32]>::try_from(test.public_key.as_ref()),
            ) else {
                assert_eq!(test.result, TestResult::Invalid, "tc {}", test.tc_id);
                continue;
            };
            let shared = StaticSecret::from(private).diffie_hellman(&PublicKey::from(public));
            match test.result {
                TestResult::Valid | TestResult::Acceptable => {
                    assert_eq!(
                        shared.as_bytes(),
                        test.shared_secret.as_ref(),
                        "tc {}",
                        test.tc_id
                    );
                }
                TestResult::Invalid => {
                    assert_ne!(
                        shared.as_bytes(),
                        test.shared_secret.as_ref(),
                        "tc {}",
                        test.tc_id
                    );
                }
            }
            // A low-order public key yields the all-zero secret, which the
            // contributory check reports.
            if test.flags.contains(&TestFlag::ZeroSharedSecret) {
                assert!(!shared.was_contributory(), "tc {}", test.tc_id);
                low_order += 1;
            }
            checked += 1;
        }
    }
    assert!(checked > 400, "{checked}");
    assert!(low_order > 0);
}
