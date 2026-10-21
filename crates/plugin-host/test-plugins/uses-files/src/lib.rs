//! A test plugin that lists a folder with `std::fs`, which makes the
//! component import `wasi:filesystem`. The host must refuse to load it.

use plugin_sdk::{Event, Node, Page, Plugin, View};

struct UsesFiles;

impl Plugin for UsesFiles {
    fn pages() -> Vec<Page> {
        Vec::new()
    }

    fn render(_page: String) -> Vec<Node> {
        let count = std::fs::read_dir(".").map_or(0, Iterator::count);
        let mut view = View::panel("root", None);
        view.text(View::ROOT, "count", &format!("{count} entries"));
        view.into_nodes()
    }

    fn handle_event(_page: String, _event: Event) {}
}

plugin_sdk::export!(UsesFiles);
