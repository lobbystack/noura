//! Trial S7: a hello plugin in Rust. It shows a count and a button that adds
//! one to it.
//!
//! A native build of this crate only type-checks the plugin, so it exports
//! nothing: names such as `handle-event` belong in a component, not in a
//! native library's export table.
#![cfg_attr(not(target_family = "wasm"), allow(dead_code))]

wit_bindgen::generate!({
    path: "../wit",
    world: "plugin",
});

use noura_trial::s7::ui::NodeKind;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNT: AtomicU32 = AtomicU32::new(0);

struct Hello;

impl Guest for Hello {
    fn render() -> Vec<Node> {
        let count = COUNT.load(Ordering::Relaxed);
        vec![
            Node {
                id: "root".into(),
                kind: NodeKind::Panel,
                children: vec![1, 2],
            },
            Node {
                id: "count".into(),
                kind: NodeKind::Text(format!("Count: {count}")),
                children: vec![],
            },
            Node {
                id: "add-one".into(),
                kind: NodeKind::Button("Add one".into()),
                children: vec![],
            },
        ]
    }

    fn handle_event(node_id: String) {
        if node_id == "add-one" {
            COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(target_family = "wasm")]
export!(Hello);
