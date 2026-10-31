//! Generates the protocol v2 fixtures of sync-v2.md §32 and checks that
//! the files in docs/workspace-format/fixtures match what the code
//! produces. Run with `NOURA_WRITE_FIXTURES=1` to write them.
//!
//! Each case states its expected outcome where it is built, from the rule
//! it exercises; `sync-protocol`'s and this crate's fixture tests then run
//! the verifiers on the files and compare.

mod account;
mod approval;
mod blob;
mod chain;
mod collaboration;
mod common;
mod device;
mod domains;
mod encoding;
mod ids;
mod invitation;
mod keys;
mod live;
mod manifest;
mod operation;
mod padding;
mod policy;
mod presence;
mod profile;
mod recovery;
mod share;
mod world;

use std::path::PathBuf;

fn fixtures() -> Vec<common::Fixture> {
    vec![
        encoding::fixture(),
        padding::fixture(),
        ids::fixture(),
        device::fixture(),
        account::fixture(),
        approval::fixture(),
        policy::fixture(),
        keys::fixture(),
        operation::fixture(),
        chain::fixture(),
        live::fixture(),
        domains::fixture(),
        manifest::fixture(),
        blob::fixture(),
        collaboration::fixture(),
        presence::fixture(),
        profile::fixture(),
        invitation::fixture(),
        share::fixture(),
        recovery::fixture(),
    ]
    .into_iter()
    .map(with_restore_cases)
    .collect()
}

/// Adds the decision tables of a restore (§14), which the client's own
/// tests read, to the fixtures that name them.
fn with_restore_cases(mut fixture: common::Fixture) -> common::Fixture {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("restore_cases.json")).expect("the cases parse");
    let key = match fixture.file {
        "sync-v2-live.json" => "live",
        "sync-v2-chain.json" => "chain",
        "sync-v2-profile.json" => "profile",
        _ => return fixture,
    };
    fixture.set("restore_cases", cases[key].clone());
    fixture
}

fn directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/workspace-format/fixtures")
}

#[test]
fn v2_fixtures_are_current() {
    let write = std::env::var_os("NOURA_WRITE_FIXTURES").is_some();
    let mut stale = Vec::new();
    for fixture in fixtures() {
        let path = directory().join(fixture.file);
        let json = fixture.to_json();
        if write {
            std::fs::write(&path, json).unwrap();
        } else if std::fs::read_to_string(&path).ok().as_deref() != Some(json.as_str()) {
            stale.push(fixture.file);
        }
    }
    assert!(
        stale.is_empty(),
        "stale fixtures {stale:?}: regenerate with NOURA_WRITE_FIXTURES=1"
    );
}
