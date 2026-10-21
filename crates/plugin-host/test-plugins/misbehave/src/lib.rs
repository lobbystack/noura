//! A test plugin for the host's limits. Its page, `misbehave`, draws a
//! line of text and three buttons; pressing `loop` loops forever,
//! `allocate` allocates until the memory cap stops it, and `panic`
//! panics. Its command `loop` and its Inbox section `slow` loop forever
//! too, and its Inbox section `broken` returns a view whose root lists a
//! child that doesn't exist. The page `fine` always draws.

use plugin_sdk::{Command, Event, InboxSection, Node, NodeKind, Page, Plugin, View};

struct Misbehave;

impl Plugin for Misbehave {
    fn pages() -> Vec<Page> {
        vec![Page {
            id: "misbehave".into(),
            title: "Misbehave".into(),
        }]
    }

    fn render(page: String) -> Vec<Node> {
        match page.as_str() {
            "loop" => spin(),
            "allocate" => allocate(),
            "panic" => panic!("the misbehave plugin panics on purpose"),
            _ => {
                let mut view = View::panel("root", Some("Misbehave"));
                view.text(View::ROOT, "line", "Still fine.");
                view.button(View::ROOT, "loop", "Loop forever");
                view.button(View::ROOT, "allocate", "Allocate too much");
                view.button(View::ROOT, "panic", "Panic");
                view.into_nodes()
            }
        }
    }

    fn handle_event(_page: String, event: Event) {
        let Event::Pressed(node) = event;
        match node.as_str() {
            "loop" => {
                spin();
            }
            "allocate" => {
                allocate();
            }
            "panic" => panic!("the misbehave plugin panics on purpose"),
            _ => {}
        }
    }

    fn commands() -> Vec<Command> {
        vec![Command {
            id: "loop".into(),
            title: "Loop forever".into(),
            palette: true,
            plugins: true,
        }]
    }

    fn run_command(_id: String, _input: String) -> Result<String, String> {
        spin()
    }

    fn inbox_sections() -> Vec<InboxSection> {
        vec![
            InboxSection {
                id: "slow".into(),
                title: "Looping section".into(),
            },
            InboxSection {
                id: "broken".into(),
                title: "Broken section".into(),
            },
        ]
    }

    fn render_inbox_section(section: String) -> Vec<Node> {
        if section == "broken" {
            return vec![Node {
                id: "root".into(),
                kind: NodeKind::Panel(None),
                children: vec![7],
            }];
        }
        spin()
    }
}

/// Never returns.
fn spin() -> ! {
    let mut count: u64 = 0;
    loop {
        count = std::hint::black_box(count.wrapping_add(1));
    }
}

/// Allocates a mebibyte at a time, touching each, until the host's memory
/// cap refuses more and the allocation fails.
fn allocate() -> ! {
    let mut held: Vec<Vec<u8>> = Vec::new();
    loop {
        held.push(vec![1; 1 << 20]);
        std::hint::black_box(&held);
    }
}

plugin_sdk::export!(Misbehave);
