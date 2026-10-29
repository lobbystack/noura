//! `sync-v2-domains.json`: the `.noura-domain` marker's content (§9).
//! Ticket 50 adds the cases about marker validity and domain roots that
//! need restricted domains.

use serde_json::json;
use sync_protocol::records::marker_content;
use sync_protocol::types::to_hex;

use super::common::{Fixture, id};

pub fn fixture() -> Fixture {
    let mut f = Fixture::new(
        "sync-v2-domains.json",
        "Restricted domain markers (sync-v2.md §9): the exact content of a `.noura-domain` file, one line `domain: <base64url domain ID>` and a line feed, and content that isn't a marker's. `stray_marker` asks whether a client refuses an object at a marker path, `held_roots` which local markers hold their folders after a sync without a journal, and `hold_ends` whether a hold ends without your confirmation.",
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
    journal_less(&mut f, domain);
    f
}

/// Local `.noura-domain` files when a folder syncs without this device's
/// journal, and objects at a marker path (§9). Ticket 50 adds the cases
/// that need restricted domains in the policy.
fn journal_less(f: &mut Fixture, domain: sync_protocol::types::Id) {
    let other = id("another restricted domain");
    let marker = id("the restricted domain's marker");
    let stray = id("an object an editor placed at a marker path");
    for (name, rid, path, expect) in [
        (
            "an_object_at_a_marker_path_that_no_domain_names_is_refused",
            stray,
            "Team/.noura-domain",
            true,
        ),
        (
            "a_domains_own_marker_is_written",
            marker,
            "Team/.noura-domain",
            false,
        ),
        (
            "a_name_that_only_contains_the_marker_name_is_ordinary",
            stray,
            "Team/.noura-domain.md",
            false,
        ),
    ] {
        f.case(json!({
            "kind": "stray_marker",
            "name": name,
            "rid": rid.to_hex(),
            "path": path,
            "marker_rids": [marker.to_hex()],
            "expect": expect,
        }));
    }
    f.case(json!({
        "kind": "held_roots",
        "name": "local_markers_other_than_applied_ones_hold_their_folders",
        "locals": [
            {"folder": "Team", "domain": domain.to_hex()},
            {"folder": "Team/Pay", "domain": other.to_hex()},
            {"folder": "Scratch", "domain": null},
        ],
        "applied": [{"folder": "Team", "domain": domain.to_hex()}],
        "expect": [
            {"folder": "Scratch", "domain": null},
            {"folder": "Team/Pay", "domain": other.to_hex()},
        ],
    }));
    for (name, roots, expect) in [
        (
            "another_domains_valid_marker_never_ends_a_hold",
            vec![("Team", domain), ("Team/Pay", domain)],
            false,
        ),
        (
            "the_held_domains_innermost_root_ends_the_hold",
            vec![("Team", domain), ("Team/Pay", other)],
            true,
        ),
        (
            "a_grant_restored_after_a_reset_ends_the_hold_through_the_domains_marker",
            vec![("Team/Pay", other)],
            true,
        ),
        (
            "an_outer_root_of_the_held_domain_under_another_inner_one_doesnt",
            vec![("Team", other), ("Team/Pay", domain)],
            false,
        ),
    ] {
        f.case(json!({
            "kind": "hold_ends",
            "name": name,
            "held": {"folder": "Team/Pay", "domain": other.to_hex()},
            "roots": roots.iter().map(|(folder, d)| json!({"folder": folder, "domain": d.to_hex()})).collect::<Vec<_>>(),
            "expect": expect,
        }));
    }
}
