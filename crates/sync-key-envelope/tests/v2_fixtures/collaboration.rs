//! `sync-v2-collaboration.json`: the formats of text mode (§22): the text
//! lineage ID and yrs client IDs. Tickets 54 and 55 add the sandbox verdict
//! cases, the verdict module's hash and the merge cases.

use serde_json::json;
use sync_protocol::crypto::hash;
use sync_protocol::ids::{text_lineage_id, yrs_client_id};
use sync_protocol::tag::Tag;
use sync_protocol::types::to_hex;

use super::common::{Fixture, id};

pub fn fixture() -> Fixture {
    let mut f = Fixture::new(
        "sync-v2-collaboration.json",
        "Text mode (sync-v2.md §22): the text lineage ID derived from the rid, the base operation's digest and the first document's bytes, and yrs client IDs from a card index and a load number.",
    );
    let rid = id("note rid");
    let base = hash(b"base operation");
    let document = vec![
        0x01, 0x01, 0x00, 0x00, 0x04, 0x01, 0x07, 0x63, 0x6f, 0x6e, 0x74, 0x65, 0x6e, 0x74,
    ];
    let mut input = Tag::TextLineage.bytes();
    input.extend_from_slice(&rid.0);
    input.extend_from_slice(&base.0);
    input.extend_from_slice(&document);
    f.case(json!({
        "kind": "text_lineage",
        "name": "lineage_of_a_first_document",
        "rid": rid.to_hex(),
        "base": base.to_hex(),
        "document": to_hex(&document),
        "input": to_hex(&input),
        "expect": text_lineage_id(&rid, &base, &document).to_hex(),
    }));
    let mut other = document.clone();
    other.push(0);
    f.case(json!({
        "kind": "text_lineage",
        "name": "another_first_document_another_lineage",
        "rid": rid.to_hex(),
        "base": base.to_hex(),
        "document": to_hex(&other),
        "input": null,
        "expect": text_lineage_id(&rid, &base, &other).to_hex(),
    }));
    for (index, load, expect) in [
        (1u32, 1u32, Some(536_870_913u64)),
        (2, 7, Some(2 * 536_870_912 + 7)),
        (16_777_215, 536_870_911, Some((1u64 << 53) - 1)),
        (8, 1, Some(4_294_967_297)),
        (0, 1, None),
        (1, 0, None),
        (1, 536_870_912, None),
        (16_777_216, 1, None),
    ] {
        f.case(json!({
            "kind": "client_id",
            "name": format!("card_{index}_load_{load}"),
            "card_index": index,
            "load": load,
            "expect": expect,
        }));
        assert_eq!(yrs_client_id(index, load), expect);
    }
    f
}
