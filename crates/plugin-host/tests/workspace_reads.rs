//! Seam 3: workspace reads. Entries and files cross by workspace-relative
//! path, objects by stable ID, each with its content revision. Paths from
//! a plugin are checked before any service sees them.

mod support;

use std::sync::Arc;

use plugin_host::{Entry, EntryKind, FileRead, Listed, ObjectRead, WorkspacePath};
use support::plugins::{self, FakeWorkspace};

const READS: [&str; 2] = ["name = \"workspace-read\"", "name = \"views\""];

fn path(path: &str) -> WorkspacePath {
    WorkspacePath::new(path).unwrap()
}

fn workspace() -> FakeWorkspace {
    FakeWorkspace {
        listing: vec![
            Listed::Entry(Entry {
                path: path("Notes"),
                kind: EntryKind::Folder,
                object_id: None,
                object_type: None,
                revision: None,
            }),
            Listed::Entry(Entry {
                path: path("Notes/Plan.md"),
                kind: EntryKind::File,
                object_id: Some("note_01".into()),
                object_type: Some("note".into()),
                revision: Some("rev-plan".into()),
            }),
            Listed::Unsupported,
        ],
        files: vec![FileRead {
            path: path("Notes/Plan.md"),
            bytes: b"Grow tomatoes".to_vec(),
            revision: "rev-plan".into(),
        }],
        objects: vec![ObjectRead {
            id: "note_01".into(),
            object_type: "note".into(),
            title: "Plan".into(),
            path: path("Notes/Plan.md"),
            body: "Grow tomatoes".into(),
            properties: "{}".into(),
            revision: "rev-plan".into(),
        }],
        ..FakeWorkspace::default()
    }
}

#[test]
fn listing_returns_entries_with_ids_and_revisions_and_an_error_for_a_name_it_cant_hold() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&probe, &READS, Some(Arc::new(workspace())));

    let nodes = instance.render("list").unwrap();

    assert_eq!(
        plugins::texts(&nodes),
        [
            "folder Notes object - type - revision -",
            "file Notes/Plan.md object note_01 type note revision rev-plan",
            "error unsupported-path",
        ]
    );
}

#[test]
fn a_file_reads_by_path_with_its_revision() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&probe, &READS, Some(Arc::new(workspace())));

    let nodes = instance.render("read-file:Notes/Plan.md").unwrap();
    assert_eq!(
        plugins::texts(&nodes),
        ["file Notes/Plan.md revision rev-plan text Grow tomatoes"]
    );
    let nodes = instance.render("read-file:Notes/Gone.md").unwrap();
    assert_eq!(plugins::texts(&nodes), ["error not-found Notes/Gone.md"]);
}

#[test]
fn an_object_reads_by_its_stable_id_with_its_revision() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&probe, &READS, Some(Arc::new(workspace())));

    let nodes = instance.render("read-object:note_01").unwrap();

    assert_eq!(
        plugins::texts(&nodes),
        [
            "object note_01 type note title Plan path Notes/Plan.md revision rev-plan body Grow \
             tomatoes"
        ]
    );
}

#[test]
fn an_object_reads_by_the_path_of_its_file_with_its_revision() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let workspace = Arc::new(workspace());
    let (instance, _folders) = plugins::running(&probe, &READS, Some(workspace.clone()));

    let nodes = instance.render("read-object-at:Notes/Plan.md").unwrap();
    assert_eq!(
        plugins::texts(&nodes),
        [
            "object note_01 type note title Plan path Notes/Plan.md revision rev-plan body Grow \
             tomatoes"
        ]
    );
    let nodes = instance.render("read-object-at:Notes/Gone.md").unwrap();
    assert_eq!(plugins::texts(&nodes), ["error not-found Notes/Gone.md"]);
    let nodes = instance.render("read-object-at:../Plan.md").unwrap();
    assert_eq!(plugins::texts(&nodes), ["error invalid-path ../Plan.md"]);
    assert_eq!(
        workspace.calls(),
        [
            "read-object-at Notes/Plan.md",
            "read-object-at Notes/Gone.md"
        ]
    );
}

#[test]
fn a_path_that_isnt_workspace_relative_never_reaches_the_workspace() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let workspace = Arc::new(workspace());
    let (instance, _folders) = plugins::running(&probe, &READS, Some(workspace.clone()));

    for bad in [
        "../secret.md",
        "/etc/hosts",
        "Notes\\Plan.md",
        "Notes//Plan.md",
        "./Notes/Plan.md",
        "Notes/../../x",
        "",
    ] {
        let nodes = instance.render(&format!("read-file:{bad}")).unwrap();
        assert_eq!(
            plugins::texts(&nodes),
            [format!("error invalid-path {bad}")],
            "{bad:?}"
        );
    }
    assert!(workspace.calls().is_empty(), "{:?}", workspace.calls());
}
