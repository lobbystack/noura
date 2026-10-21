//! Shared helpers for the sandbox's tests: one compiled sandbox per backend,
//! the fixture file, and builders for records.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::OnceLock;

use collab_sandbox::{Backend, Note, Sandbox};
use serde_json::Value;
use sync_protocol::codec::{Bytes, Encode, Sorted, Text};
use sync_protocol::records::{Content, Meta, Payload, PayloadKind};
use sync_protocol::types::{Id, from_hex};

/// The sandbox on Cranelift, compiled once per test binary.
pub fn sandbox() -> &'static Sandbox {
    static SANDBOX: OnceLock<Sandbox> = OnceLock::new();
    SANDBOX.get_or_init(|| Sandbox::new(Backend::Cranelift).expect("the sandbox starts"))
}

/// The sandbox on Pulley, compiled once per test binary.
#[cfg(feature = "pulley")]
pub fn pulley() -> &'static Sandbox {
    static SANDBOX: OnceLock<Sandbox> = OnceLock::new();
    SANDBOX.get_or_init(|| Sandbox::new(Backend::Pulley).expect("the sandbox starts"))
}

/// Every backend this build runs, by name.
pub fn backends() -> Vec<(&'static str, &'static Sandbox)> {
    #[allow(unused_mut)]
    let mut all = vec![("cranelift", sandbox())];
    #[cfg(feature = "pulley")]
    all.push(("pulley", pulley()));
    all
}

/// The client ID of `card`'s `load`.
pub fn client(card: u64, load: u64) -> u64 {
    (card << 29) + load
}

/// Client IDs for a note's loads: card 3's loads from 1 up.
pub fn client_ids() -> Box<dyn FnMut() -> Option<u64> + Send> {
    client_ids_of(3)
}

/// Client IDs for `card`'s loads, from 1 up.
pub fn client_ids_of(card: u64) -> Box<dyn FnMut() -> Option<u64> + Send> {
    let mut load = 0;
    Box::new(move || {
        load += 1;
        Some(client(card, load))
    })
}

/// A note loaded from `journal`.
pub fn note(journal: Vec<Vec<u8>>) -> Note {
    Note::open(sandbox(), journal, client_ids()).expect("the note opens")
}

pub fn fixture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/workspace-format/fixtures/sync-v2-collaboration.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("the fixture exists"))
        .expect("the fixture is JSON")
}

pub fn hex(value: &Value) -> Vec<u8> {
    from_hex(value.as_str().expect("a hex string")).expect("hexadecimal")
}

pub fn cases(kind: &str) -> Vec<Value> {
    fixture()["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .filter(|case| case["kind"] == kind)
        .cloned()
        .collect()
}

pub fn path_meta(path: &str) -> Meta {
    Meta {
        key: Text::new("path").unwrap(),
        seq: 0,
        value: Some(Bytes(path.as_bytes().to_vec())),
    }
}

/// A `text_update` payload of an unmanaged note at `Notes/Plan.md`.
pub fn update_payload(load: u32, yrs: &[u8]) -> Vec<u8> {
    Payload {
        rid: Id([7; 16]),
        kind: PayloadKind::TextUpdate {
            object_id: Text::new("").unwrap(),
            text_lineage: Id([9; 16]),
            load,
            meta: Sorted::new(vec![path_meta("Notes/Plan.md")]).unwrap(),
            content: Content::Inline {
                bytes: Bytes(yrs.to_vec()),
            },
        },
    }
    .to_bytes()
}

/// A `text_snapshot` payload of an unmanaged note at `Notes/Plan.md`.
pub fn snapshot_payload(yrs: &[u8]) -> Vec<u8> {
    Payload {
        rid: Id([7; 16]),
        kind: PayloadKind::TextSnapshot {
            object_id: Text::new("").unwrap(),
            revision: sync_protocol::crypto::hash(yrs),
            text_lineage: Id([9; 16]),
            meta: Sorted::new(vec![path_meta("Notes/Plan.md")]).unwrap(),
            content: Content::Inline {
                bytes: Bytes(yrs.to_vec()),
            },
            origin: None,
        },
    }
    .to_bytes()
}

/// A lib0 v1 writer for crafted yrs bytes.
#[derive(Default, Clone)]
pub struct Lib0(pub Vec<u8>);

impl Lib0 {
    pub fn var(mut self, mut value: u64) -> Self {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                self.0.push(byte);
                return self;
            }
            self.0.push(byte | 0x80);
        }
    }

    pub fn raw(mut self, bytes: &[u8]) -> Self {
        self.0.extend_from_slice(bytes);
        self
    }

    pub fn string(self, text: &str) -> Self {
        self.var(text.len() as u64).raw(text.as_bytes())
    }
}

/// `client` inserting `text` at the start of `content`, at `clock`.
pub fn insert(client: u64, clock: u64, text: &str) -> Vec<u8> {
    Lib0::default()
        .var(1)
        .var(1)
        .var(client)
        .var(clock)
        .raw(&[4])
        .var(1)
        .string("content")
        .string(text)
        .var(0)
        .0
}

/// `client` inserting `text` right after the element `(origin_client,
/// origin_clock)`, at `clock`.
pub fn insert_after(client: u64, clock: u64, origin: (u64, u64), text: &str) -> Vec<u8> {
    Lib0::default()
        .var(1)
        .var(1)
        .var(client)
        .var(clock)
        .raw(&[0x84])
        .var(origin.0)
        .var(origin.1)
        .string(text)
        .var(0)
        .0
}
