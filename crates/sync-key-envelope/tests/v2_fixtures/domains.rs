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
    roots(&mut f, domain);
    f
}

/// Marker validity and domain roots (§9): which markers a device takes as
/// roots, where a new file goes, where an object's file sits, and what a
/// bootstrap holds when a marker isn't valid.
fn roots(f: &mut Fixture, domain: sync_protocol::types::Id) {
    let early = domain;
    let late = id("another restricted domain");
    let (early_rid, late_rid) = (id("the early marker"), id("the late marker"));
    let state = |path: &str| json!({"path": path, "revision": "00".repeat(32), "delete": false});
    let gone = json!({"path": "", "revision": "00".repeat(32), "delete": true});
    let marker = |domain, rid, created_at, listed, live: Vec<serde_json::Value>| {
        json!({
            "domain": sync_protocol::types::Id::to_hex(&domain),
            "rid": sync_protocol::types::Id::to_hex(&rid),
            "created_at": created_at,
            "listed": listed,
            "live": live,
        })
    };
    let root = |folder: &str, domain: sync_protocol::types::Id, rid: sync_protocol::types::Id| json!({"folder": folder, "domain": domain.to_hex(), "rid": rid.to_hex()});
    let team = state("Team/.noura-domain");
    let cases: Vec<(&str, Vec<serde_json::Value>, Vec<serde_json::Value>)> = vec![
        (
            "a_marker_with_one_live_state_roots_its_folder",
            vec![marker(early, early_rid, 3, true, vec![team.clone()])],
            vec![root("Team", early, early_rid)],
        ),
        (
            "a_moved_marker_roots_the_folder_it_moved_to",
            vec![marker(
                early,
                early_rid,
                3,
                true,
                vec![state("Archive/Team/.noura-domain")],
            )],
            vec![root("Archive/Team", early, early_rid)],
        ),
        (
            "a_marker_with_two_live_states_is_no_root",
            vec![marker(
                early,
                early_rid,
                3,
                true,
                vec![team.clone(), state("Other/.noura-domain")],
            )],
            vec![],
        ),
        (
            "a_deleted_marker_is_no_root",
            vec![marker(early, early_rid, 3, true, vec![gone.clone()])],
            vec![],
        ),
        (
            "a_state_and_a_delete_are_no_root",
            vec![marker(early, early_rid, 3, true, vec![team.clone(), gone])],
            vec![],
        ),
        (
            "a_marker_the_log_never_carried_is_no_root",
            vec![marker(early, early_rid, 3, true, vec![])],
            vec![],
        ),
        (
            "a_marker_at_another_name_is_no_root",
            vec![marker(
                early,
                early_rid,
                3,
                true,
                vec![state("Team/notes.md")],
            )],
            vec![],
        ),
        (
            "a_marker_at_the_workspace_root_is_no_root",
            vec![marker(
                early,
                early_rid,
                3,
                true,
                vec![state(".noura-domain")],
            )],
            vec![],
        ),
        (
            "a_domain_the_policy_no_longer_lists_is_no_root",
            vec![marker(early, early_rid, 3, false, vec![team.clone()])],
            vec![],
        ),
        (
            "an_earlier_root_evicts_a_later_marker_inside_it",
            vec![
                marker(early, early_rid, 3, true, vec![team.clone()]),
                marker(
                    late,
                    late_rid,
                    5,
                    true,
                    vec![state("Team/Pay/.noura-domain")],
                ),
            ],
            vec![root("Team", early, early_rid)],
        ),
        (
            "an_earlier_root_evicts_a_later_marker_at_its_path",
            vec![
                marker(late, late_rid, 5, true, vec![team.clone()]),
                marker(early, early_rid, 3, true, vec![team.clone()]),
            ],
            vec![root("Team", early, early_rid)],
        ),
        (
            "a_later_domains_root_may_hold_an_earlier_one",
            vec![
                marker(
                    early,
                    early_rid,
                    3,
                    true,
                    vec![state("Team/Pay/.noura-domain")],
                ),
                marker(late, late_rid, 5, true, vec![team.clone()]),
            ],
            vec![
                root("Team", late, late_rid),
                root("Team/Pay", early, early_rid),
            ],
        ),
    ];
    for (name, markers, expect) in cases {
        f.case(json!({"kind": "valid_roots", "name": name, "markers": markers, "expect": expect}));
    }

    let nested = vec![
        root("Team", late, late_rid),
        root("Team/Pay", early, early_rid),
    ];
    for (name, path, expect) in [
        (
            "a_file_outside_every_root_goes_to_the_workspace_domain",
            "Plan.md",
            None,
        ),
        ("a_new_file_goes_to_its_root", "Team/Plan.md", Some(late)),
        (
            "a_new_file_goes_to_the_innermost_root",
            "Team/Pay/Plan.md",
            Some(early),
        ),
        (
            "a_name_that_only_starts_like_the_root_is_outside",
            "Team 2/Plan.md",
            None,
        ),
    ] {
        f.case(json!({
            "kind": "new_file_domain",
            "name": name,
            "roots": nested,
            "path": path,
            "expect": expect.map(|d: sync_protocol::types::Id| d.to_hex()),
        }));
    }
    for (name, folder, expect) in [
        ("a_marker_outside_every_root_may_be_created", "Other", true),
        ("a_marker_inside_another_root_is_refused", "Team/Sub", false),
        ("a_marker_at_a_root_is_refused", "Team", false),
        ("the_workspace_root_takes_no_marker", "", false),
    ] {
        f.case(json!({
            "kind": "marker_allowed",
            "name": name,
            "roots": [root("Team", early, early_rid)],
            "folder": folder,
            "expect": expect,
        }));
    }
    let one = vec![root("Team", early, early_rid)];
    for (name, path, object_domain, expect) in [
        (
            "an_object_of_the_root_sits_at_its_path",
            "Team/Plan.md",
            Some(early),
            json!({"at": "Team/Plan.md"}),
        ),
        (
            "a_workspace_object_outside_every_root_sits_at_its_path",
            "Plan.md",
            None,
            json!({"at": "Plan.md"}),
        ),
        (
            "a_workspace_object_under_a_root_goes_under_shared",
            "Team/Plan.md",
            None,
            json!({"shared": "Team (shared)/Plan.md"}),
        ),
        (
            "another_domains_object_under_a_root_goes_under_shared",
            "Team/Sub/Plan.md",
            Some(late),
            json!({"shared": "Team (shared)/Sub/Plan.md"}),
        ),
        (
            "an_object_whose_path_leaves_its_root_stays_for_review",
            "Elsewhere/Plan.md",
            Some(early),
            json!({"outside": "Team"}),
        ),
    ] {
        f.case(json!({
            "kind": "placement",
            "name": name,
            "roots": one,
            "path": path,
            "object_domain": object_domain.map(|d: sync_protocol::types::Id| d.to_hex()),
            "expect": expect,
        }));
    }
    for (name, local, expect) in [
        (
            "an_edit_under_shared_uploads_at_its_synced_path",
            "Team (shared)/Sub/Plan.md",
            "Team/Sub/Plan.md",
        ),
        ("a_path_elsewhere_stays", "Team/Plan.md", "Team/Plan.md"),
    ] {
        f.case(json!({
            "kind": "synced_path",
            "name": name,
            "roots": one,
            "local": local,
            "expect": expect,
        }));
    }
    f.case(json!({
        "kind": "bootstrap_held_roots",
        "name": "a_bootstrap_without_a_valid_marker_holds_each_folder_that_directly_holds_an_object",
        "domain": early.to_hex(),
        "paths": ["Team/Plan.md", "Team/Notes.md", "Team/Sub/Deep/Plan.md"],
        "expect": [
            {"folder": "Team", "domain": early.to_hex()},
            {"folder": "Team/Sub/Deep", "domain": early.to_hex()},
        ],
    }));
    for (name, names, expect) in [
        ("the_marker_counts_as_present", vec![".noura-domain"], true),
        (
            "an_icloud_placeholder_counts_as_present",
            vec![".noura-domain.icloud"],
            true,
        ),
        (
            "icloud_s_own_placeholder_of_a_dot_file_counts_too",
            vec!["..noura-domain.icloud"],
            true,
        ),
        (
            "an_ordinary_file_doesnt",
            vec!["Plan.md", ".DS_Store"],
            false,
        ),
    ] {
        f.case(json!({
            "kind": "marker_present",
            "name": name,
            "names": names,
            "expect": expect,
        }));
    }
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
