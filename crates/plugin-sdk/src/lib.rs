//! The WIT interfaces plugins implement and the Rust library for plugin
//! authors.
//!
//! `wit/plugin.wit` defines `noura:plugin@0.1.0`. A Rust plugin is a
//! `cdylib` crate built for `wasm32-wasip2` that implements [`Plugin`] and
//! names its type once with [`export!`]:
//!
//! ```
//! use plugin_sdk::{Event, Node, Page, Plugin, View, VirtualRow};
//!
//! struct Hello;
//!
//! impl Plugin for Hello {
//!     fn pages() -> Vec<Page> {
//!         vec![Page { id: "hello".into(), title: "Hello".into() }]
//!     }
//!
//!     fn render(_page: String) -> Vec<Node> {
//!         let mut view = View::panel("root", Some("Hello"));
//!         view.text(View::ROOT, "greeting", "Hello from a plugin.");
//!         view.into_nodes()
//!     }
//!
//!     fn rows(_page: String, _node: String, _start: u32, _count: u32) -> Vec<VirtualRow> {
//!         Vec::new()
//!     }
//!
//!     fn handle_event(_page: String, _event: Event) {}
//! }
//!
//! plugin_sdk::export!(Hello);
//! ```
//!
//! Each capability interface sits in its own module, such as [`workspace`],
//! and works only when the plugin's manifest declares its capability.

// The generated bindings and their `export!` macro name this crate as
// `plugin_sdk`, so plugin crates need no dependency on `wit-bindgen`.
extern crate self as plugin_sdk;

wit_bindgen::generate!({
    path: "wit",
    world: "plugin",
    pub_export_macro: true,
    export_macro_name: "__export_plugin",
    default_bindings_module: "plugin_sdk",
    runtime_path: "::plugin_sdk::__private::rt",
});

#[doc(hidden)]
pub mod __private {
    pub use wit_bindgen::rt;
}

pub use exports::noura::plugin::view::{
    ColumnPress, Event, Field, Guest as Plugin, Page, Submission, Target,
};
pub use noura::plugin::types::Error;
pub use noura::plugin::ui::{
    Choice, Column, Form, Input, Item, MenuItem, Node, NodeKind, SortOrder, Table, Value,
    VirtualList, VirtualRow,
};

/// Reading the open workspace. Needs the `workspace-read` capability.
pub mod workspace {
    pub use crate::noura::plugin::workspace::{
        Entry, EntryKind, File, Object, list_entries, read_file, read_object,
    };
}

/// Exports a type that implements [`Plugin`] as the plugin's component.
///
/// A build for another target than WebAssembly exports nothing, so the
/// plugin crate still type-checks natively.
#[macro_export]
macro_rules! export {
    ($ty:ident) => {
        #[cfg(target_family = "wasm")]
        $crate::__export_plugin!($ty);
        #[cfg(not(target_family = "wasm"))]
        const _: fn() = || {
            let _ = <$ty as $crate::Plugin>::pages;
        };
    };
}

/// Builds a view's flat node list. Each node added names its parent by the
/// index an earlier call returned, starting from [`View::ROOT`].
///
/// ```
/// use plugin_sdk::{Column, Value, View};
///
/// let mut view = View::panel("root", Some("Garden"));
/// let beds = view.list(View::ROOT, "beds", "Beds");
/// let north = view.item(beds, "north", "North bed", Some("Tomatoes"));
/// let menu = view.menu(north, "north-menu", "Actions for North bed");
/// view.menu_item(menu, "water", "Water", true);
/// view.separator(menu, "north-gap");
/// view.menu_item(menu, "harvest", "Harvest", false);
///
/// let columns = vec![Column { id: "name".into(), title: "Plant".into(), sorted: None }];
/// let plants = view.table(View::ROOT, "plants", "Plants", columns);
/// view.row(plants, "basil", &["Basil"]);
///
/// let form = view.form(View::ROOT, "new-bed", "New bed", "Add bed");
/// view.input(form, "name", "Name", Value::Text(String::new()));
/// view.input(form, "sown", "Sown on", Value::Date(None));
/// view.choice_input(form, "sun", "Sun", &[("full", "Full sun"), ("shade", "Shade")], None);
///
/// view.virtual_list(View::ROOT, "seeds", "Seeds", 10_000);
/// assert_eq!(view.into_nodes().len(), 14);
/// ```
#[derive(Debug, Clone)]
pub struct View {
    nodes: Vec<Node>,
}

impl View {
    /// The index of the view's root panel.
    pub const ROOT: u32 = 0;

    /// A view whose root is a panel with this ID, labeled for screen
    /// readers when `label` is given.
    pub fn panel(id: &str, label: Option<&str>) -> Self {
        Self {
            nodes: vec![Node {
                id: id.to_owned(),
                kind: NodeKind::Panel(label.map(str::to_owned)),
                children: Vec::new(),
            }],
        }
    }

    /// Add a node of `kind` to the node at `parent`, and return its index.
    ///
    /// # Panics
    ///
    /// When `parent` isn't the index of a node already added.
    pub fn add(&mut self, parent: u32, id: &str, kind: NodeKind) -> u32 {
        let index = u32::try_from(self.nodes.len()).expect("fewer than 2^32 nodes");
        self.nodes[parent as usize].children.push(index);
        self.nodes.push(Node {
            id: id.to_owned(),
            kind,
            children: Vec::new(),
        });
        index
    }

    /// Add a panel inside the panel at `parent`.
    pub fn add_panel(&mut self, parent: u32, id: &str, label: Option<&str>) -> u32 {
        self.add(parent, id, NodeKind::Panel(label.map(str::to_owned)))
    }

    /// Add a line of plain text.
    pub fn text(&mut self, parent: u32, id: &str, text: &str) -> u32 {
        self.add(parent, id, NodeKind::Text(text.to_owned()))
    }

    /// Add Markdown, which Noura draws.
    pub fn markdown(&mut self, parent: u32, id: &str, markdown: &str) -> u32 {
        self.add(parent, id, NodeKind::Markdown(markdown.to_owned()))
    }

    /// Add a button. Its presses arrive as [`Event::Pressed`] with `id`,
    /// unless you give it a [`menu`](Self::menu), which it then opens.
    pub fn button(&mut self, parent: u32, id: &str, label: &str) -> u32 {
        self.add(parent, id, NodeKind::Button(label.to_owned()))
    }

    /// Add a list you select items in. Add its items with
    /// [`item`](Self::item); selecting one sends [`Event::Selected`], and
    /// opening one sends [`Event::Activated`].
    pub fn list(&mut self, parent: u32, id: &str, label: &str) -> u32 {
        self.add(parent, id, NodeKind::List(label.to_owned()))
    }

    /// Add an item to the list at `list`, with an optional quieter line
    /// under its label.
    pub fn item(&mut self, list: u32, id: &str, label: &str, detail: Option<&str>) -> u32 {
        let item = Item {
            label: label.to_owned(),
            detail: detail.map(str::to_owned),
        };
        self.add(list, id, NodeKind::Item(item))
    }

    /// Add a list of `rows` rows that Noura asks [`Plugin::rows`] for as
    /// they scroll into view.
    pub fn virtual_list(&mut self, parent: u32, id: &str, label: &str, rows: u32) -> u32 {
        let list = VirtualList {
            label: label.to_owned(),
            rows,
        };
        self.add(parent, id, NodeKind::VirtualList(list))
    }

    /// Add a table with these columns. Add its rows with
    /// [`row`](Self::row). A header press sends [`Event::Sorted`].
    pub fn table(&mut self, parent: u32, id: &str, label: &str, columns: Vec<Column>) -> u32 {
        let table = Table {
            label: label.to_owned(),
            columns,
        };
        self.add(parent, id, NodeKind::Table(table))
    }

    /// Add a row to the table at `table`, with one cell per column.
    pub fn row(&mut self, table: u32, id: &str, cells: &[&str]) -> u32 {
        let cells = cells.iter().map(|cell| (*cell).to_owned()).collect();
        self.add(table, id, NodeKind::Row(cells))
    }

    /// Add a form with a submit button labeled `submit`. Its inputs go
    /// inside it; submitting sends [`Event::Submitted`] with their values.
    pub fn form(&mut self, parent: u32, id: &str, label: &str, submit: &str) -> u32 {
        let form = Form {
            label: label.to_owned(),
            submit: submit.to_owned(),
        };
        self.add(parent, id, NodeKind::Form(form))
    }

    /// Add a labeled input holding `value`, whose case sets the kind of
    /// input. Each change sends [`Event::Changed`].
    pub fn input(&mut self, parent: u32, id: &str, label: &str, value: Value) -> u32 {
        let input = Input {
            label: label.to_owned(),
            value,
            choices: Vec::new(),
        };
        self.add(parent, id, NodeKind::Input(input))
    }

    /// Add a single-choice input. `choices` holds each option's ID and
    /// label, and `chosen` the ID of the chosen option.
    pub fn choice_input(
        &mut self,
        parent: u32,
        id: &str,
        label: &str,
        choices: &[(&str, &str)],
        chosen: Option<&str>,
    ) -> u32 {
        let input = Input {
            label: label.to_owned(),
            value: Value::Choice(chosen.map(str::to_owned)),
            choices: choices
                .iter()
                .map(|(id, label)| Choice {
                    id: (*id).to_owned(),
                    label: (*label).to_owned(),
                })
                .collect(),
        };
        self.add(parent, id, NodeKind::Input(input))
    }

    /// Add a menu to the item, row or button at `owner`.
    pub fn menu(&mut self, owner: u32, id: &str, label: &str) -> u32 {
        self.add(owner, id, NodeKind::Menu(label.to_owned()))
    }

    /// Add an item to the menu at `menu`. Choosing it sends
    /// [`Event::Chosen`] with `id`.
    pub fn menu_item(&mut self, menu: u32, id: &str, label: &str, enabled: bool) -> u32 {
        let item = MenuItem {
            label: label.to_owned(),
            enabled,
        };
        self.add(menu, id, NodeKind::MenuItem(item))
    }

    /// Add a line between the menu's items.
    pub fn separator(&mut self, menu: u32, id: &str) -> u32 {
        self.add(menu, id, NodeKind::Separator)
    }

    /// The nodes, root first, for [`Plugin::render`] to return.
    pub fn into_nodes(self) -> Vec<Node> {
        self.nodes
    }
}
