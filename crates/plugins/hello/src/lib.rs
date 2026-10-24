//! A hello plugin: a rail page whose panel shows how many files the
//! workspace holds and a button that adds one to a count.
//!
//! Build it with `crates/plugin-sdk/build-plugins.sh`, then install the
//! folder holding `plugin.toml` and the built `hello_plugin.wasm`.

use std::sync::atomic::{AtomicU32, Ordering};

use plugin_sdk::{Event, Node, Page, Plugin, View, workspace};

static COUNT: AtomicU32 = AtomicU32::new(0);

struct Hello;

impl Plugin for Hello {
    fn pages() -> Vec<Page> {
        vec![Page {
            id: "hello".into(),
            title: "Hello".into(),
        }]
    }

    fn render(_page: String) -> Vec<Node> {
        let mut view = View::panel("root", Some("Hello"));
        view.text(View::ROOT, "files", &files());
        let count = COUNT.load(Ordering::Relaxed);
        view.text(View::ROOT, "count", &format!("Count: {count}"));
        view.button(View::ROOT, "add-one", "Add one");
        view.into_nodes()
    }

    fn handle_event(_page: String, event: Event) {
        if matches!(event, Event::Pressed(node) if node == "add-one") {
            COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// How many files the workspace holds, in words.
fn files() -> String {
    match workspace::list_entries() {
        Ok(entries) => {
            let files = entries
                .iter()
                .filter(|entry| {
                    entry
                        .as_ref()
                        .is_ok_and(|entry| entry.kind == workspace::EntryKind::File)
                })
                .count();
            match files {
                1 => "This workspace has 1 file.".to_owned(),
                files => format!("This workspace has {files} files."),
            }
        }
        Err(error) => format!("Noura didn't list the files: {error:?}"),
    }
}

plugin_sdk::export!(Hello);
