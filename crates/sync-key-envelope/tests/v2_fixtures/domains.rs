//! `sync-v2-domains.json`: the `.noura-domain` marker's content (§9).
//! Ticket 50 adds the cases about marker validity and domain roots.

use serde_json::json;
use sync_protocol::records::marker_content;
use sync_protocol::types::to_hex;

use super::common::{Fixture, id};

pub fn fixture() -> Fixture {
    let mut f = Fixture::new(
        "sync-v2-domains.json",
        "Restricted domain markers (sync-v2.md §9): the exact content of a `.noura-domain` file, one line `domain: <base64url domain ID>` and a line feed, and content that isn't a marker's.",
    );
    let domain = id("restricted domain");
    let content = marker_content(&domain);
    f.case(json!({
        "kind": "marker",
        "name": "marker_content",
        "content": to_hex(content.as_bytes()),
        "expect": domain.to_hex(),
    }));
    for (name, text) in [
        ("missing_line_feed", content.trim_end().to_owned()),
        ("crlf", content.replace('\n', "\r\n")),
        ("extra_line", format!("{content}more\n")),
        ("padded_base64", content.replace('\n', "==\n")),
        ("other_key", content.replace("domain:", "Domain:")),
        ("no_space", content.replace("domain: ", "domain:")),
        (
            "zero_id",
            format!(
                "domain: {}\n",
                sync_protocol::types::Id::ZERO.to_base64url()
            ),
        ),
    ] {
        f.case(json!({"kind": "marker", "name": name, "content": to_hex(text.as_bytes()), "expect": null}));
    }
    f.set("marker_file_name", json!(".noura-domain"));
    f
}
