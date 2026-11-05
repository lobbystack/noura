//! `sync-v2-padding.json`: Padmé padding (§6).

use serde_json::json;
use sync_protocol::padding::{is_valid_padded_length, pad, padded_length, padme};
use sync_protocol::types::to_hex;

use super::common::Fixture;

pub fn fixture() -> Fixture {
    let mut f = Fixture::new(
        "sync-v2-padding.json",
        "Padmé padding (sync-v2.md §6): padme and padded_length at boundary lengths, valid padded lengths, padded plaintexts, and malformed padding that unpadding refuses before any parsing.",
    );
    for length in [
        0u64,
        1,
        2,
        3,
        4,
        5,
        100,
        252,
        253,
        255,
        256,
        257,
        300,
        383,
        384,
        385,
        511,
        512,
        513,
        1_000,
        1_023,
        1_024,
        1_025,
        3_200,
        4_095,
        4_096,
        4_097,
        32_768,
        32_769,
        65_535,
        65_536,
        65_537,
        262_148,
        1_048_576,
        2_097_160,
        4_294_967_295,
        4_294_967_296,
        1 << 40,
    ] {
        f.case(json!({
            "kind": "padded_length",
            "name": format!("framed_{length}"),
            "framed": length,
            "padme": padme(length),
            "expect": padded_length(length),
        }));
    }
    for (x, valid) in [
        (255u64, false),
        (256, true),
        (257, false),
        (264, false),
        (272, true),
        (270_336, true),
        (270_337, false),
        (34_816, true),
        (34_817, false),
    ] {
        assert_eq!(is_valid_padded_length(x), valid);
        f.case(json!({"kind": "valid_padded_length", "name": format!("length_{x}"), "length": x, "expect": valid}));
    }
    let hello = pad(b"hello").unwrap();
    f.case(json!({
        "kind": "pad",
        "name": "hello",
        "content": to_hex(b"hello"),
        "expect": to_hex(&hello),
    }));
    let long = vec![0x61u8; 300];
    f.case(json!({
        "kind": "pad",
        "name": "300_bytes",
        "content": to_hex(&long),
        "expect": to_hex(&pad(&long).unwrap()),
    }));
    let unpad_case = |f: &mut Fixture, name: &str, bytes: &[u8], expect: serde_json::Value| {
        f.case(json!({"kind": "unpad", "name": name, "padded": to_hex(bytes), "expect": expect}));
    };
    unpad_case(&mut f, "valid", &hello, json!(to_hex(b"hello")));
    unpad_case(&mut f, "total_255_bytes", &hello[..255], json!("length"));
    let mut longer = hello.clone();
    longer.push(0);
    unpad_case(&mut f, "total_257_bytes", &longer, json!("length"));
    let mut oversized = hello.clone();
    oversized[..4].copy_from_slice(&253u32.to_be_bytes());
    unpad_case(&mut f, "n_past_the_end", &oversized, json!("oversized"));
    let mut full = hello.clone();
    full[..4].copy_from_slice(&252u32.to_be_bytes());
    unpad_case(
        &mut f,
        "n_filling_the_space",
        &full,
        json!(to_hex(&full[4..])),
    );
    let mut dirty = hello.clone();
    dirty[200] = 1;
    unpad_case(&mut f, "nonzero_padding", &dirty, json!("nonzero_padding"));
    f.case(json!({
        "kind": "operation_ciphertext_limit",
        "name": "largest_payload",
        "payload_length": 262_144,
        "expect": 270_352,
    }));
    f.set(
        "overhead_bound",
        json!({
            "below_64_kib": 0.0625,
            "below_4_gib": 0.03125,
            "above": 0.015625,
            "worst_case_framed_length": 32_769,
            "worst_case_padded_length": 34_816,
        }),
    );
    f
}
