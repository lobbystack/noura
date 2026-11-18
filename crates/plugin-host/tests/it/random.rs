//! Plugins get a random seed from WASI's random interface, so a std
//! `HashMap` works, and the types Transcription and Dictation register
//! are held for them.

use crate::support;
use plugin_host::{Limits, ObjectTypes, PLUGIN_TYPES, Services};
use support::plugins;

#[test]
fn a_plugin_s_hash_map_seeds_itself() {
    let Some(component) = plugins::built("probe_plugin") else {
        return;
    };
    let (plugin, _folders) = plugins::start(
        "probe",
        &component,
        &["name = \"views\""],
        Services::default(),
        Limits::default(),
    );
    let view = plugin.render("random").expect("the probe renders");
    assert_eq!(plugins::texts(&view), ["hashmap 3 2"]);
}

#[test]
fn transcription_and_dictation_types_are_held_for_their_plugins() {
    for (object_type, owner) in [
        ("transcription", "transcription"),
        ("transcript", "transcription"),
        ("transcription-settings", "transcription"),
        ("dictation-settings", "dictation"),
    ] {
        assert!(
            PLUGIN_TYPES.contains(&(object_type, owner)),
            "{object_type}"
        );
    }
    let _ = ObjectTypes::new();
}
