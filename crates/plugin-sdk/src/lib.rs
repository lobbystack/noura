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
//! as [`workspace`], [`objects`] and [`network`], and works only when the
//! plugin holds its capability.

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
        commands::Guest as Commands, inbox::Guest as Inbox, object_types::Guest as ObjectTypes,
        settings::Guest as Settings, tools::Guest as Tools, view::Guest as View,
    };
    pub use wit_bindgen::rt;
}

pub use exports::noura::plugin::commands::Command;
pub use exports::noura::plugin::inbox::Section as InboxSection;
pub use exports::noura::plugin::object_types::ObjectType;
pub use exports::noura::plugin::tools::Tool;
pub use exports::noura::plugin::view::Page;
pub use noura::plugin::types::{Error, PropertyError};
pub use noura::plugin::ui::{Event, Node, NodeKind};

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

/// Requests to the HTTPS hosts the manifest's `network` capability lists.
pub mod network {
    pub use crate::noura::plugin::network::{Header, Method, Request, Response, fetch};
}

/// Running commands that other plugins offer. Needs the `commands`
/// capability with each command, or its plugin, among its `targets`.
pub mod command_calls {
    pub use crate::noura::plugin::command_calls::call;
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

    /// Something happened in a page's view. Needs `views`.
    fn handle_event(_page: String, _event: Event) {}

    /// The object types the plugin registers when it starts, each with
    /// its schema. Needs `object-types`.
    fn object_types() -> Vec<ObjectType> {
        Vec::new()
    }

    /// The nodes of the properties panel Noura shows beside the body of
    /// the object `object`, one of the plugin's types. Needs
    /// `object-types`.
    fn render_panel(_object: String) -> Vec<Node> {
        Vec::new()
    }

    /// Something happened in the properties panel of the object `object`.
    /// Needs `object-types`.
    fn handle_panel_event(_object: String, _event: Event) {}

    /// The commands the plugin offers to the command palette and to other
    /// plugins. Needs `commands`.
    fn commands() -> Vec<Command> {
        Vec::new()
    }

    /// Run the command `id` with `input`, which is empty from the command
    /// palette. Needs `commands`.
    fn run_command(id: String, _input: String) -> Result<String, String> {
        Err(format!("This plugin has no command {id}."))
    }

    /// The sections the plugin adds to the Inbox. Needs `inbox-sections`.
    fn inbox_sections() -> Vec<InboxSection> {
        Vec::new()
    }

    /// The nodes of an Inbox section. Needs `inbox-sections`.
    fn render_inbox_section(_section: String) -> Vec<Node> {
        Vec::new()
    }

    /// Something happened in an Inbox section. Needs `inbox-sections`.
    fn handle_inbox_event(_section: String, _event: Event) {}

    /// The title of the plugin's page in Settings, if it has one. Needs
    /// `settings`.
    fn settings_page() -> Option<String> {
        None
    }

    /// The nodes of the plugin's page in Settings. Needs `settings`.
    fn render_settings() -> Vec<Node> {
        Vec::new()
    }

    /// Something happened on the plugin's page in Settings. Needs
    /// `settings`.
    fn handle_settings_event(_event: Event) {}

    /// The tools the plugin offers to AI assistants. Needs `tools`.
    fn tools() -> Vec<Tool> {
        Vec::new()
    }

    /// Run the tool `name` with JSON `arguments` that matched its schema.
    /// Needs `tools`.
    fn call_tool(name: String, _arguments: String) -> Result<String, String> {
        Err(format!("This plugin has no tool {name}."))
    }
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
                fn handle_event(page: ::std::string::String, event: $crate::Event) {
                    <$ty as $crate::Plugin>::handle_event(page, event)
                }
            }

            impl $crate::__private::Commands for __NouraExports {
                fn commands() -> ::std::vec::Vec<$crate::Command> {
                    <$ty as $crate::Plugin>::commands()
                }
                fn run(
                    id: ::std::string::String,
                    input: ::std::string::String,
                ) -> ::std::result::Result<::std::string::String, ::std::string::String> {
                    <$ty as $crate::Plugin>::run_command(id, input)
                }
            }

            impl $crate::__private::Inbox for __NouraExports {
                fn sections() -> ::std::vec::Vec<$crate::InboxSection> {
                    <$ty as $crate::Plugin>::inbox_sections()
                }
                fn render_section(section: ::std::string::String) -> ::std::vec::Vec<$crate::Node> {
                    <$ty as $crate::Plugin>::render_inbox_section(section)
                }
                fn handle_section_event(section: ::std::string::String, event: $crate::Event) {
                    <$ty as $crate::Plugin>::handle_inbox_event(section, event)
                }
            }

            impl $crate::__private::Settings for __NouraExports {
                fn page() -> ::std::option::Option<::std::string::String> {
                    <$ty as $crate::Plugin>::settings_page()
                }
                fn render_settings() -> ::std::vec::Vec<$crate::Node> {
                    <$ty as $crate::Plugin>::render_settings()
                }
                fn handle_settings_event(event: $crate::Event) {
                    <$ty as $crate::Plugin>::handle_settings_event(event)
                }
            }

            impl $crate::__private::Tools for __NouraExports {
                fn tools() -> ::std::vec::Vec<$crate::Tool> {
                    <$ty as $crate::Plugin>::tools()
                }
                fn call_tool(
                    name: ::std::string::String,
                    arguments: ::std::string::String,
                ) -> ::std::result::Result<::std::string::String, ::std::string::String> {
                    <$ty as $crate::Plugin>::call_tool(name, arguments)
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

/// Builds a view's flat node list. Each node added names its parent panel
/// by the index an earlier call returned, starting from [`View::ROOT`].
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

    /// Add a node of `kind` to the panel at `parent`, and return its
    /// index.
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

    /// Add a button. Its presses arrive as [`Event::Pressed`] with `id`.
    pub fn button(&mut self, parent: u32, id: &str, label: &str) -> u32 {
        self.add(parent, id, NodeKind::Button(label.to_owned()))
    }

    /// The nodes, root first, for [`Plugin::render`] to return.
    pub fn into_nodes(self) -> Vec<Node> {
        self.nodes
    }
}
