//! The WIT interfaces plugins implement and the Rust library for plugin
//! authors.
//!
//! `wit/plugin.wit` defines `noura:plugin@0.1.0`. A Rust plugin is a
//! `cdylib` crate built for `wasm32-wasip2` that implements [`Plugin`] and
//! names its type once with [`export!`]. Each method of [`Plugin`] has a
//! default, so a plugin writes only the ones for the capabilities its
//! manifest declares:
//!
//! ```
//! use plugin_sdk::{Node, Page, Plugin, View};
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
//! }
//!
//! plugin_sdk::export!(Hello);
//! ```
//!
//! Each capability interface the plugin calls sits in its own module, such
//! as [`workspace`] and [`objects`], and works only when the plugin holds
//! its capability.

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
    pub use crate::exports::noura::plugin::{
        object_types::Guest as ObjectTypes, view::Guest as View,
    };
    pub use wit_bindgen::rt;
}

pub use exports::noura::plugin::object_types::ObjectType;
pub use exports::noura::plugin::view::Page;
pub use noura::plugin::types::{Error, PropertyError};
pub use noura::plugin::ui::{
    Choice, Column, ColumnPress, Event, Field, Form, Input, Item, ItemList, MenuItem, Move, Node,
    NodeKind, SortOrder, Submission, Table, Target, Value, VirtualList, VirtualRow,
};

/// Reading the open workspace. Needs the `workspace-read` capability.
pub mod workspace {
    pub use crate::noura::plugin::workspace::{
        Entry, EntryKind, File, Object, list_entries, read_file, read_object,
    };
}

/// Creating, reading, changing, moving and deleting objects of the types
/// the plugin registered. Needs the `object-types` capability.
pub mod objects {
    pub use crate::noura::plugin::objects::{
        Changes, NewObject, Object, create, delete, get, list, move_to, update,
    };
}

/// What a plugin adds to Noura. Noura calls each method only when the
/// plugin holds the capability it needs; the defaults add nothing.
pub trait Plugin {
    /// The pages the plugin adds to the rail. Needs `views`.
    fn pages() -> Vec<Page> {
        Vec::new()
    }

    /// The nodes of a page's view. Needs `views`.
    fn render(_page: String) -> Vec<Node> {
        Vec::new()
    }

    /// Rows `start` up to `start + count` of the virtual list `node` in a
    /// page's view, as they scroll into view. Return fewer when the list
    /// ends sooner. Needs `views`.
    fn rows(_page: String, _node: String, _start: u32, _count: u32) -> Vec<VirtualRow> {
        Vec::new()
    }

    /// Something happened in a page's view. Needs `views`.
    fn handle_event(_page: String, _event: Event) {}

    /// The object types the plugin registers when it starts, each with
    /// its schema. Needs `object-types`.
    fn object_types() -> Vec<ObjectType> {
        Vec::new()
    }

    /// The nodes of the properties panel Noura shows beside the body of
    /// the object `object`, one of the plugin's types. A panel can't hold
    /// a virtual list. Needs `object-types`.
    fn render_panel(_object: String) -> Vec<Node> {
        Vec::new()
    }

    /// Something happened in the properties panel of the object `object`.
    /// Needs `object-types`.
    fn handle_panel_event(_object: String, _event: Event) {}
}

/// Exports a type that implements [`Plugin`] as the plugin's component.
///
/// A build for another target than WebAssembly exports nothing, so the
/// plugin crate still type-checks natively.
#[macro_export]
macro_rules! export {
    ($ty:ident) => {
        #[cfg(target_family = "wasm")]
        const _: () = {
            struct __NouraExports;

            impl $crate::__private::View for __NouraExports {
                fn pages() -> ::std::vec::Vec<$crate::Page> {
                    <$ty as $crate::Plugin>::pages()
                }
                fn render(page: ::std::string::String) -> ::std::vec::Vec<$crate::Node> {
                    <$ty as $crate::Plugin>::render(page)
                }
                fn rows(
                    page: ::std::string::String,
                    node: ::std::string::String,
                    start: u32,
                    count: u32,
                ) -> ::std::vec::Vec<$crate::VirtualRow> {
                    <$ty as $crate::Plugin>::rows(page, node, start, count)
                }
                fn handle_event(page: ::std::string::String, event: $crate::Event) {
                    <$ty as $crate::Plugin>::handle_event(page, event)
                }
            }

            impl $crate::__private::ObjectTypes for __NouraExports {
                fn types() -> ::std::vec::Vec<$crate::ObjectType> {
                    <$ty as $crate::Plugin>::object_types()
                }
                fn render_panel(object: ::std::string::String) -> ::std::vec::Vec<$crate::Node> {
                    <$ty as $crate::Plugin>::render_panel(object)
                }
                fn handle_panel_event(object: ::std::string::String, event: $crate::Event) {
                    <$ty as $crate::Plugin>::handle_panel_event(object, event)
                }
            }

            $crate::__export_plugin!(__NouraExports);
        };
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
///
/// A board holds columns of cards you drag between columns, and a list
/// can let you drag its items into a new order. A drop sends
/// [`Event::Moved`]:
///
/// ```
/// use plugin_sdk::View;
///
/// let mut view = View::panel("root", Some("Chores"));
/// let board = view.board(View::ROOT, "chores", "Chores");
/// let todo = view.board_column(board, "todo", "To do");
/// let doing = view.board_column(board, "doing", "Doing");
/// let weed = view.card(todo, "weed", "Weed the beds", Some("Saturday"));
/// let menu = view.menu(weed, "weed-menu", "Move Weed the beds");
/// view.menu_item(menu, "weed-to-doing", "Doing", true);
/// view.card(doing, "mulch", "Mulch", None);
///
/// let order = view.reorderable_list(View::ROOT, "order", "Sowing order");
/// view.item(order, "beans", "Beans", None);
/// view.item(order, "peas", "Peas", None);
/// assert_eq!(view.into_nodes().len(), 11);
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
        let list = ItemList {
            label: label.to_owned(),
            reorder: false,
        };
        self.add(parent, id, NodeKind::List(list))
    }

    /// Add a list like [`list`](Self::list) whose items you can also drag
    /// to new positions. A drop sends [`Event::Moved`], naming the list as
    /// both `node` and `to`; the list shows the order you draw next.
    pub fn reorderable_list(&mut self, parent: u32, id: &str, label: &str) -> u32 {
        let list = ItemList {
            label: label.to_owned(),
            reorder: true,
        };
        self.add(parent, id, NodeKind::List(list))
    }

    /// Add a board. Add its columns with
    /// [`board_column`](Self::board_column), at least one. Dropping a card
    /// sends [`Event::Moved`]; the board shows the cards where you draw
    /// them next, so a move you refuse puts the card back.
    pub fn board(&mut self, parent: u32, id: &str, label: &str) -> u32 {
        self.add(parent, id, NodeKind::Board(label.to_owned()))
    }

    /// Add a column headed `label` to the board at `board`.
    pub fn board_column(&mut self, board: u32, id: &str, label: &str) -> u32 {
        self.add(board, id, NodeKind::BoardColumn(label.to_owned()))
    }

    /// Add a card to the board column at `column`, with an optional
    /// quieter detail under its label. Selecting it sends
    /// [`Event::Selected`] and opening it [`Event::Activated`], naming the
    /// board.
    pub fn card(&mut self, column: u32, id: &str, label: &str, detail: Option<&str>) -> u32 {
        let card = Item {
            label: label.to_owned(),
            detail: detail.map(str::to_owned),
        };
        self.add(column, id, NodeKind::Card(card))
    }

    /// Add an item to the list at `list`, with an optional quieter detail
    /// after its label, on the same line.
    pub fn item(&mut self, list: u32, id: &str, label: &str, detail: Option<&str>) -> u32 {
        let item = Item {
            label: label.to_owned(),
            detail: detail.map(str::to_owned),
        };
        self.add(list, id, NodeKind::Item(item))
    }

    /// Add a list of `rows` rows, at most 1,000,000, that Noura asks
    /// [`Plugin::rows`] for as they scroll into view.
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

    /// Add a row to the table at `table`, with one cell per column. At
    /// least one cell needs text: screen readers name the row by the first
    /// cell that has some.
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

    /// Add a menu to the item, row, card or button at `owner`.
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
