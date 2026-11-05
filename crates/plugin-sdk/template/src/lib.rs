use std::sync::atomic::{AtomicU32, Ordering};

use plugin_sdk::{Event, Node, Page, Plugin, View, workspace};

static PRESSES: AtomicU32 = AtomicU32::new(0);

struct WordCount;

impl Plugin for WordCount {
    fn pages() -> Vec<Page> {
        vec![Page {
            id: "main".into(),
            title: "Word count".into(),
        }]
    }

    fn render(_page: String) -> Vec<Node> {
        let files = workspace::list_entries().map_or(0, |entries| entries.len());
        let presses = PRESSES.load(Ordering::Relaxed);
        let mut view = View::panel("root", Some("Word count"));
        view.text(View::ROOT, "files", &format!("Files: {files}"));
        view.text(View::ROOT, "presses", &format!("Presses: {presses}"));
        view.button(View::ROOT, "press", "Press me");
        view.into_nodes()
    }

    fn handle_event(_page: String, event: Event) {
        if matches!(event, Event::Pressed(node) if node == "press") {
            PRESSES.fetch_add(1, Ordering::Relaxed);
        }
    }
}

plugin_sdk::export!(WordCount);
