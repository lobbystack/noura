//! The notice the server's image ships names every crate under the Mozilla
//! Public License at the version Cargo.lock pins, and carries the license.

use std::collections::BTreeMap;

const NOTICE: &str = include_str!("../THIRD_PARTY_NOTICES.txt");
const LOCK: &str = include_str!("../../../Cargo.lock");
const DOCKERFILE: &str = include_str!("../Dockerfile");

/// Every locked package's versions.
fn locked() -> BTreeMap<&'static str, Vec<&'static str>> {
    let mut versions: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for package in LOCK.split("[[package]]\n").skip(1) {
        let mut lines = package.lines();
        let name = lines.next().and_then(|l| l.strip_prefix("name = \""));
        let version = lines.next().and_then(|l| l.strip_prefix("version = \""));
        if let (Some(name), Some(version)) = (name, version) {
            versions
                .entry(name.trim_end_matches('"'))
                .or_default()
                .push(version.trim_end_matches('"'));
        }
    }
    versions
}

#[test]
fn the_notice_names_the_locked_mpl_crates_and_ships_in_the_image() {
    let locked = locked();
    for name in [
        "webauthn-rs",
        "webauthn-rs-core",
        "webauthn-rs-proto",
        "webauthn-attestation-ca",
        "base64urlsafedata",
    ] {
        for version in &locked[name] {
            let line = format!("- {name} {version},");
            assert!(NOTICE.contains(&line), "the notice lacks {line}");
        }
    }
    assert!(NOTICE.contains("Mozilla Public License Version 2.0"));
    assert!(NOTICE.contains("Exhibit B"));
    assert!(DOCKERFILE.contains("COPY crates/sync-server/THIRD_PARTY_NOTICES.txt"));
}
