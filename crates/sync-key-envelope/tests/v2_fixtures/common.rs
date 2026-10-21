//! Shared machinery for the v2 fixture generator: deterministic actors,
//! the fixture document, and the rejected variants each accepted record
//! gets.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Value, json};
use sync_key_envelope::v2::DeviceKeys;
use sync_protocol::codec::Encode;
use sync_protocol::crypto::derive_key;
use sync_protocol::records::{Card, Signed};
use sync_protocol::tag::Tag;
use sync_protocol::types::{Id, SecretKey, to_hex};

/// A fixture's actor: a device whose keys derive from its name, and its
/// account.
#[derive(Clone, Debug)]
pub struct Actor {
    pub name: String,
    pub keys: DeviceKeys,
    pub account: Id,
}

/// A deterministic account ID from a name.
pub fn account_id(name: &str) -> Id {
    let mut id = [0u8; 16];
    id.copy_from_slice(&derive_key("noura.sync.v2 fixture account", name.as_bytes()).0[..16]);
    Id(id)
}

/// A deterministic 16-byte ID from a label.
pub fn id(label: &str) -> Id {
    let mut id = [0u8; 16];
    id.copy_from_slice(&derive_key("noura.sync.v2 fixture id", label.as_bytes()).0[..16]);
    Id(id)
}

/// A deterministic 32-byte secret from a label.
pub fn secret(label: &str) -> SecretKey {
    derive_key("noura.sync.v2 fixture secret", label.as_bytes())
}

/// A deterministic nonce from a label.
pub fn nonce(label: &str) -> sync_protocol::types::Nonce {
    let mut n = [0u8; 24];
    n.copy_from_slice(&derive_key("noura.sync.v2 fixture nonce", label.as_bytes()).0[..24]);
    sync_protocol::types::Nonce(n)
}

impl Actor {
    /// The device `name` of the account `account_name`.
    pub fn new(account_name: &str, name: &str) -> Self {
        let keys = DeviceKeys::from_seeds(
            &derive_key("noura.sync.v2 fixture signing seed", name.as_bytes()),
            derive_key("noura.sync.v2 fixture hpke seed", name.as_bytes()),
        );
        Self {
            name: name.to_owned(),
            keys,
            account: account_id(account_name),
        }
    }

    pub fn device(&self) -> Id {
        self.keys.device()
    }

    pub fn card(&self) -> Card {
        self.keys.card(self.account)
    }

    /// The public description a fixture lists for this actor. Fixture keys
    /// are public test values.
    pub fn describe(&self) -> Value {
        json!({
            "account": self.account.to_hex(),
            "device": self.device().to_hex(),
            "signing_seed": to_hex(self.keys.signing().seed().expose()),
            "signing_public": self.keys.signing().public().to_hex(),
            "hpke_seed": to_hex(self.keys.hpke_seed().expose()),
            "card_digest": self.card().fingerprint().to_hex(),
        })
    }
}

/// One fixture document.
pub struct Fixture {
    pub file: &'static str,
    description: String,
    keys: BTreeMap<String, Value>,
    records: Vec<Value>,
    cases: Vec<Value>,
    extra: BTreeMap<String, Value>,
}

/// Rewrites a JSON value with every map's keys sorted, so the file never
/// depends on serde_json's map order.
pub fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, Value> =
                map.iter().map(|(k, v)| (k, canonical(v))).collect();
            let mut out = serde_json::Map::new();
            for (k, v) in sorted {
                out.insert(k.clone(), v);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// Which variants a record type gets besides its accepted encoding.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// A tagged, signed record: wrong tag, wrong version, truncated field,
    /// bad signature.
    TaggedSigned,
    /// A tagged, unsigned record: wrong tag, wrong version, truncated field.
    Tagged,
    /// An untagged body: truncated field and trailing bytes.
    Body,
}

impl Fixture {
    pub fn new(file: &'static str, description: &str) -> Self {
        Self {
            file,
            description: description.to_owned(),
            keys: BTreeMap::new(),
            records: Vec::new(),
            cases: Vec::new(),
            extra: BTreeMap::new(),
        }
    }

    pub fn actor(&mut self, actor: &Actor) {
        self.keys.insert(actor.name.clone(), actor.describe());
    }

    pub fn set(&mut self, key: &str, value: Value) {
        self.extra.insert(key.to_owned(), value);
    }

    pub fn case(&mut self, case: Value) {
        self.cases.push(case);
    }

    fn push_record(&mut self, record: Value) {
        self.records.push(record);
    }

    /// Records an accepted value of `ty` and its rejected variants. A
    /// signed record names its signer's actor.
    pub fn accept<T: Encode + Serialize>(
        &mut self,
        name: &str,
        ty: &str,
        value: &T,
        signer: Option<&Actor>,
        shape: Shape,
    ) -> Vec<u8> {
        let bytes = value.to_bytes();
        let mut record = json!({
            "name": name,
            "type": ty,
            "hex": to_hex(&bytes),
            "digest": sync_protocol::crypto::hash(&bytes).to_hex(),
            "expect": "ok",
        });
        // Large bodies carry their bytes only; the fields would double them.
        if bytes.len() <= 16 * 1024 {
            record["value"] = serde_json::to_value(value).unwrap();
        }
        if let Some(signer) = signer {
            self.actor(signer);
            record["signer"] = json!(signer.name);
        }
        self.push_record(record);
        self.variants(name, ty, &bytes, signer, shape);
        bytes
    }

    /// Records the standard rejected variants of the accepted record
    /// `name`, as mutations of its bytes: `truncate` drops that many final
    /// bytes, `append` adds bytes, `retag` replaces the leading tag, `version`
    /// rewrites the leading tag's version, and `flip_last_byte` flips the
    /// lowest bit of the last byte, which lies in a signature.
    pub fn variants(
        &mut self,
        name: &str,
        ty: &str,
        bytes: &[u8],
        signer: Option<&Actor>,
        shape: Shape,
    ) {
        let mut variant = |suffix: &str, mutation: Value, expect: Value| {
            let mut record = json!({
                "name": format!("{name}_{suffix}"),
                "type": ty,
                "from": name,
                "mutation": mutation,
                "expect": expect,
            });
            if let Some(signer) = signer {
                record["signer"] = json!(signer.name);
            }
            self.records.push(record);
        };
        variant(
            "truncated",
            json!({"truncate": 1}),
            json!({"decode": "truncated"}),
        );
        match shape {
            Shape::Body => {
                variant(
                    "trailing",
                    json!({"append": "00"}),
                    json!({"decode": "trailing_bytes"}),
                );
            }
            Shape::Tagged | Shape::TaggedSigned => {
                let tag_len = usize::from(bytes[0]) + 3;
                let own = std::str::from_utf8(&bytes[1..tag_len - 2]).unwrap();
                let other = if own == Tag::Share.domain() {
                    Tag::Op
                } else {
                    Tag::Share
                };
                variant(
                    "wrong_tag",
                    json!({"retag": to_hex(&other.bytes())}),
                    json!({"decode": "wrong_tag"}),
                );
                variant(
                    "wrong_version",
                    json!({"version": 3}),
                    json!({"decode": "unsupported_version"}),
                );
            }
        }
        if shape == Shape::TaggedSigned {
            variant(
                "bad_signature",
                json!({"flip_last_byte": true}),
                json!({"verify": "signature"}),
            );
        }
    }

    /// Records bytes a decoder must refuse.
    pub fn reject(&mut self, name: &str, ty: &str, bytes: &[u8], expect: Value) {
        self.push_record(json!({
            "name": name,
            "type": ty,
            "hex": to_hex(bytes),
            "expect": expect,
        }));
    }

    /// Records a signed record's accepted encoding and variants.
    pub fn accept_signed<B: Encode + Serialize>(
        &mut self,
        name: &str,
        ty: &str,
        record: &Signed<B>,
        signer: &Actor,
    ) -> Vec<u8> {
        self.accept(name, ty, record, Some(signer), Shape::TaggedSigned)
    }

    pub fn to_json(&self) -> String {
        let doc = json!({
            "description": self.description,
            "fixture_keys_are_public_test_values": true,
            "keys": self.keys,
            "records": self.records,
            "cases": self.cases,
        });
        let mut doc = doc.as_object().unwrap().clone();
        for (k, v) in &self.extra {
            doc.insert(k.clone(), v.clone());
        }
        let value = canonical(&Value::Object(doc));
        let mut out = Vec::new();
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
        let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
        value.serialize(&mut serializer).unwrap();
        out.push(b'\n');
        String::from_utf8(out).unwrap()
    }
}

/// Hexadecimal of an encodable value.
pub fn hex_of<T: Encode>(value: &T) -> String {
    to_hex(&value.to_bytes())
}
