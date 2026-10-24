//! Seam 3: the Calendar plugin's manifest in the real host. Calendar reads
//! the workspace, adds a page and an Inbox section, and declares nothing
//! that writes, so code running under its capabilities can't write an
//! object. The app-core suite runs the plugin against real files.

mod support;

use plugin_host::{Capability, Services, read_manifest};
use support::plugins;

/// The Calendar plugin's manifest, as its crate ships it.
const MANIFEST: &str = include_str!("../../plugins/calendar/plugin.toml");

#[test]
fn calendar_declares_workspace_reads_a_view_and_an_inbox_section_and_nothing_that_writes() {
    let Some(calendar) = plugins::built("calendar_plugin") else {
        return;
    };
    let folder = tempfile::tempdir().unwrap();
    plugins::write_folder(folder.path(), MANIFEST, "calendar_plugin.wasm", &calendar);

    let manifest = read_manifest(folder.path()).unwrap();

    assert_eq!(manifest.id, "calendar");
    let names: Vec<&str> = manifest.capabilities.iter().map(Capability::name).collect();
    assert_eq!(names, ["workspace-read", "views", "inbox-sections"]);
}

#[test]
fn code_running_under_the_calendar_capabilities_is_denied_a_write() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    // The probe's code under the Calendar manifest: the same ID and the
    // same capabilities, approved in full.
    let folder = tempfile::tempdir().unwrap();
    let library = tempfile::tempdir().unwrap();
    plugins::write_folder(folder.path(), MANIFEST, "calendar_plugin.wasm", &probe);
    let host = plugins::host(library.path());
    host.install(&host.inspect(folder.path()).unwrap()).unwrap();
    let loaded = host.load("calendar").unwrap();
    let plugin = host.start(
        &loaded,
        &loaded.manifest().capabilities,
        Services::default(),
    );

    let nodes = plugin.render("create-object:note:Plan").unwrap();

    assert_eq!(
        plugins::texts(&nodes),
        ["error capability-not-declared object-types"]
    );
}
