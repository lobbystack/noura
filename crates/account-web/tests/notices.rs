//! The release check of account-web's notices: the notice the module ships
//! with names each MPL-2.0 crate the module holds, at the version Cargo.lock
//! pins, and carries the license's text; the build script copies it beside
//! the module; and the repository's third-party notices name webauthn-rs
//! and webauthn-rs-proto under MPL-2.0.

const NOTICE: &str = include_str!("../THIRD_PARTY_NOTICES.txt");
const LOCK: &str = include_str!("../../../Cargo.lock");
const REPOSITORY_NOTICES: &str = include_str!("../../../THIRD_PARTY_NOTICES.md");
const BUILD_SCRIPT: &str = include_str!("../../../scripts/build-account-web.sh");

fn locked_version(name: &str) -> &'static str {
    let entry = format!("name = \"{name}\"\nversion = \"");
    let at = LOCK
        .find(&entry)
        .unwrap_or_else(|| panic!("Cargo.lock holds {name}"))
        + entry.len();
    let end = LOCK[at..].find('"').expect("a version") + at;
    &LOCK[at..end]
}

#[test]
fn the_module_ships_the_mpl_notice_of_each_mpl_crate_it_holds() {
    for name in ["webauthn-rs-proto", "base64urlsafedata"] {
        let version = locked_version(name);
        assert!(
            NOTICE.contains(&format!("- {name} {version},")),
            "the notice names {name} {version}"
        );
    }
    for section in [
        "Mozilla Public License Version 2.0",
        "3.2. Distribution of Executable Form",
        "Exhibit A - Source Code Form License Notice",
    ] {
        assert!(
            NOTICE.contains(section),
            "the notice carries the MPL-2.0 text: {section}"
        );
    }
    assert!(BUILD_SCRIPT.contains("crates/account-web/THIRD_PARTY_NOTICES.txt"));
    assert!(BUILD_SCRIPT.contains("$out/THIRD_PARTY_NOTICES.txt"));
}

#[test]
fn the_repository_notices_name_webauthn_rs_under_mpl_2_0() {
    for name in ["webauthn-rs", "webauthn-rs-proto"] {
        assert!(
            REPOSITORY_NOTICES.contains(&format!("`{name}`")),
            "THIRD_PARTY_NOTICES.md names {name}"
        );
    }
    let paragraph = REPOSITORY_NOTICES
        .split("\n\n")
        .find(|p| p.contains("`webauthn-rs-proto`"))
        .expect("a paragraph on webauthn-rs");
    assert!(paragraph.contains("MPL-2.0"), "{paragraph}");
}

/// The newest version of `name` that Cargo.lock pins, since the legacy v1
/// code still pins older dalek releases.
fn newest_locked_version(name: &str) -> String {
    let entry = format!("name = \"{name}\"\nversion = \"");
    LOCK.match_indices(&entry)
        .map(|(at, _)| {
            let start = at + entry.len();
            let end = LOCK[start..].find('"').expect("a version") + start;
            LOCK[start..end].to_owned()
        })
        .max_by_key(|v| {
            v.split('.')
                .map(|p| p.parse::<u64>().unwrap_or(0))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| panic!("Cargo.lock holds {name}"))
}

#[test]
fn the_notice_carries_the_bsd_text_of_the_share_viewers_signature_crates() {
    for name in ["curve25519-dalek", "ed25519-dalek", "subtle"] {
        let version = newest_locked_version(name);
        assert!(
            NOTICE.contains(&format!("{name} {version}")),
            "the notice names {name} {version}"
        );
    }
    assert!(NOTICE.contains("BSD 3-Clause License text"));
    assert!(NOTICE.contains("Redistributions in binary form must reproduce"));
    assert!(NOTICE.contains("- MIT: pulldown-cmark"));
}
