// The records the fuzz suite builds from its inputs, with sync_protocol
// alone. `collab_sandbox::fuzzing`, the guest targets' native harness
// (`fuzz_targets/native.rs`) and the guest's own replay of `regressions`
// each include this file, so a kept input builds the same record in the
// sandbox and in the guest's code built natively.

use sync_protocol::codec::{Bytes, Encode, Sorted, Text};
use sync_protocol::records::{BlobRef, Content, Meta, Payload, PayloadKind};
use sync_protocol::types::{Digest, Id, SecretKey};

/// Card 1's first load, which the records the suite builds come from.
const WRITER: u64 = (1 << 29) + 1;

fn path() -> Meta {
    Meta {
        key: Text::new("path").expect("a short key"),
        seq: 0,
        value: Some(Bytes(b"Notes/Fuzz.md".to_vec())),
    }
}

/// A `text_update` of card 1's first load holding `yrs`, inline or in a
/// blob.
pub fn update_payload(yrs: &[u8], in_blob: bool) -> Vec<u8> {
    Payload {
        rid: Id([7; 16]),
        kind: PayloadKind::TextUpdate {
            object_id: Text::new("").expect("an empty ID"),
            text_lineage: Id([9; 16]),
            load: 1,
            meta: Sorted::new(vec![path()]).expect("one entry"),
            content: content(yrs, in_blob),
        },
    }
    .to_bytes()
}

/// A `text_snapshot` holding `yrs`, inline or in a blob.
pub fn snapshot_payload(yrs: &[u8], in_blob: bool) -> Vec<u8> {
    Payload {
        rid: Id([7; 16]),
        kind: PayloadKind::TextSnapshot {
            object_id: Text::new("").expect("an empty ID"),
            revision: Digest([0; 32]),
            text_lineage: Id([9; 16]),
            meta: Sorted::new(vec![path()]).expect("one entry"),
            content: content(yrs, in_blob),
            origin: None,
        },
    }
    .to_bytes()
}

fn content(yrs: &[u8], in_blob: bool) -> Content {
    if in_blob {
        Content::Blob(BlobRef {
            blob: Id([3; 16]),
            ciphertext_length: yrs.len() as u64 + 16,
            ciphertext_digest: Digest([0; 32]),
            blob_key: SecretKey([1; 32]),
            plaintext_length: yrs.len() as u64,
        })
    } else {
        Content::Inline {
            bytes: Bytes(yrs.to_vec()),
        }
    }
}

/// `data` as the struct section of card 1's first load: one client's run
/// with a struct count from the first byte, the writer's client ID and
/// clock 0, then the rest of `data` as its structs. Random bytes this way
/// become updates yrs reads that far, which reach integration far more
/// often.
pub fn writer_yrs(data: &[u8]) -> Vec<u8> {
    let mut yrs = Vec::with_capacity(data.len() + 8);
    yrs.push(1);
    yrs.push(data.first().map_or(1, |count| count % 8 + 1));
    let mut value = WRITER;
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            yrs.push(byte);
            break;
        }
        yrs.push(byte | 0x80);
    }
    yrs.push(0);
    yrs.extend_from_slice(data.get(1..).unwrap_or_default());
    yrs
}
