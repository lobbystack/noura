//! Padmé padding (§6), against the figures the spec states.

use proptest::prelude::*;
use sync_protocol::padding::{
    PaddingError, is_valid_padded_length, pad, padded_length, padme, unpad,
};

#[test]
fn lengths_below_256_pad_to_256() {
    for length in [0, 1, 4, 100, 255, 256] {
        assert_eq!(padded_length(length), 256, "{length}");
    }
}

#[test]
fn the_largest_operation_payload_pads_to_the_section_13_limit() {
    // §13: padded_length(262,148) plus the 16-byte tag is 270,352.
    assert_eq!(padded_length(262_148) + 16, 270_352);
}

#[test]
fn the_worst_case_overhead_sits_at_32769_bytes() {
    // §6: "the worst case is 6.247%, at a framed length of 32,769 bytes".
    assert_eq!(padme(32_769), 34_816);
    let overhead = (34_816.0 - 32_769.0) / 32_769.0;
    assert!((overhead - 0.06247_f64).abs() < 0.00001, "{overhead}");
}

#[test]
fn a_two_mib_attachment_pads_as_section_28_counts() {
    // §28: a 2 MiB attachment pads to a 2,163,269-byte blob, whose padded
    // plaintext of P bytes plus header and chunk tags makes up that length.
    let p = padded_length(2 * 1024 * 1024 + 8);
    let chunks = p.div_ceil(65_536);
    assert_eq!(53 + p + 16 * chunks, 2_163_269);
}

#[test]
fn padding_round_trips_and_rejects_malformed_input() {
    let padded = pad(b"hello").unwrap();
    assert_eq!(padded.len(), 256);
    assert_eq!(&padded[..4], &[0, 0, 0, 5]);
    assert_eq!(unpad(&padded).unwrap(), b"hello");

    // A total that isn't a valid padded length.
    assert_eq!(unpad(&padded[..255]), Err(PaddingError::Length));
    let mut long = padded.clone();
    long.push(0);
    assert_eq!(unpad(&long), Err(PaddingError::Length));
    // An n larger than the space after the prefix.
    let mut oversized = padded.clone();
    oversized[..4].copy_from_slice(&253u32.to_be_bytes());
    assert_eq!(unpad(&oversized), Err(PaddingError::Oversized));
    oversized[..4].copy_from_slice(&252u32.to_be_bytes());
    assert!(unpad(&oversized).is_ok());
    // A nonzero padding byte.
    let mut dirty = padded;
    dirty[255] = 1;
    assert_eq!(unpad(&dirty), Err(PaddingError::NonzeroPadding));
}

fn bound(framed: u64) -> f64 {
    if framed < 64 * 1024 {
        0.0625
    } else if framed < 4 * 1024 * 1024 * 1024 {
        0.03125
    } else {
        0.015625
    }
}

proptest! {
    #[test]
    fn padme_stays_within_its_overhead_bound(framed in 256u64..(1u64 << 40)) {
        let padded = padme(framed);
        prop_assert!(padded >= framed);
        let overhead = (padded - framed) as f64 / framed as f64;
        prop_assert!(overhead <= bound(framed), "{} -> {}", framed, padded);
        prop_assert!(overhead < 0.0625);
    }

    #[test]
    fn every_padded_length_is_valid_and_stable(framed in 0u64..(1u64 << 40)) {
        let padded = padded_length(framed);
        prop_assert!(is_valid_padded_length(padded));
        prop_assert_eq!(padded_length(padded), padded);
    }

    #[test]
    fn pad_and_unpad_round_trip(content in proptest::collection::vec(any::<u8>(), 0..5_000)) {
        let padded = pad(&content).unwrap();
        prop_assert!(is_valid_padded_length(padded.len() as u64));
        prop_assert_eq!(unpad(&padded).unwrap(), &content[..]);
    }

    #[test]
    fn unpad_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..2_000)) {
        let _ = unpad(&bytes);
    }
}
