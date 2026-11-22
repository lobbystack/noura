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
//! plugin holds its capability. [`clock`] needs none: it gives the instant
//! now and this device's time zone, the plugin's only clock.

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
        commands::Guest as Commands, inbox::Guest as Inbox, kernel_events::Guest as KernelEvents,
        object_types::Guest as ObjectTypes, settings::Guest as Settings, tools::Guest as Tools,
        view::Guest as View,
    };
    pub use wit_bindgen::rt;
}

pub use exports::noura::plugin::commands::Command;
pub use exports::noura::plugin::inbox::Section as InboxSection;
pub use exports::noura::plugin::kernel_events::{
    KernelEvent, MicrophoneUse, NotificationChoice, ShortcutEvent,
};
pub use exports::noura::plugin::object_types::ObjectType;
pub use exports::noura::plugin::tools::Tool;
pub use exports::noura::plugin::view::Page;
pub use noura::plugin::types::{Error, PropertyError};
pub use noura::plugin::ui::{
    Activity, ActivityState, Avatar, Calendar, CalendarEntry, CalendarMode, Check, Choice, Color,
    Column, ColumnPress, Composer, DatePicker, Editable, EditableStyle, EmptyState, Event, Field,
    Form, Heading, HeadingLevel, Icon, Input, Item, ItemList, MenuItem, Message, MessageAuthor,
    Move, NameField, Naming, Node, NodeKind, Picker, PickerOption, Pill, Popover, SidePanel,
    SortOrder, Status, Submission, Table, Target, Value, VirtualList, VirtualRow,
};

/// Reading the open workspace. Needs the `workspace-read` capability.
pub mod workspace {
    pub use crate::noura::plugin::workspace::{
        Entry, EntryKind, File, Object, link_to, list_entries, read_file, read_object,
        read_object_at, resolve_link,
    };
}

/// Creating, reading, changing, moving and deleting objects of the types
/// the plugin registered. Needs the `object-types` capability.
pub mod objects {
    pub use crate::noura::plugin::objects::{
        Changes, NewObject, Object, create, delete, get, list, move_folder, move_to, update,
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

/// The instant now and this device's time zone. Needs no capability, and
/// is the plugin's only clock: WASI's clocks aren't linked.
pub mod clock {
    pub use crate::noura::plugin::clock::{Reading, now};
}

/// Opening an object in the note editor, one of the plugin's own pages or
/// a section of Settings in the window you acted in, and moving an object
/// of the plugin's own types to the trash there. Needs the `views` capability,
/// and works only in answer to something you did.
pub mod navigation {
    pub use crate::noura::plugin::navigation::{
        Placement, SettingsSection, open_object, open_page, open_settings, trash_object,
    };
}

/// Chats with the AI providers you set up in Settings, which Noura keeps
/// as Markdown files in the workspace, and one-off prompts with
/// `complete`. Needs the `ai` capability. Choosing a provider is your
/// say-so, so Noura asks nothing before a request goes out, and no call
/// returns a provider's key. Once you add a Brave Search key, `web_search`
/// and `web_fetch` search the web and read pages for the model, and
/// `web_ready` says whether they work. Each one answers a call the model
/// made: a run of the plugin's completed with a `web_search` call for
/// that query, or a `web_fetch` call for that address.
///
/// ```no_run
/// use plugin_sdk::ai::{self, Request, RunStatus};
///
/// let provider = ai::providers().unwrap().remove(0);
/// let chat = ai::create_chat().unwrap();
/// let run = ai::send(&Request {
///     chat: chat.id,
///     provider: provider.id,
///     message: "What should I plant in October?".into(),
///     // The tools of the plugins on in the workspace.
///     tools: ai::workspace_tools().unwrap(),
/// })
/// .unwrap();
/// // Noura draws the plugin's views again as the answer streams in.
/// let now = ai::read_run(&run.id).unwrap();
/// if now.status == RunStatus::Streaming {
///     ai::stop(&run.id).unwrap();
/// } else if now.status == RunStatus::Completed && !now.tool_calls.is_empty() {
///     // Noura asks you about each call, runs the ones you allow, and
///     // streams the next answer into the same run.
///     ai::run_tools(&run.id).unwrap();
/// }
/// ```
///
/// Orbs are named agents in the workspace, each with one ongoing thread.
/// An Orb runs the tools its file lists without asking:
///
/// ```no_run
/// use plugin_sdk::ai;
///
/// let orbs = ai::orbs().unwrap();
/// let meetings = orbs.iter().find(|orb| orb.title == "Meetings").unwrap();
/// let run = ai::send_to_orb(&meetings.id, "What did we decide today?").unwrap();
/// // Later: the run streams, runs its tool calls and answers again.
/// let _ = ai::read_run(&run.id).unwrap();
/// let (_orb, _thread, _messages) = ai::read_orb(&meetings.id).unwrap();
/// let _history = ai::orb_runs(&meetings.id, 10).unwrap();
/// ```
pub mod ai {
    pub use crate::noura::plugin::ai::{
        Chat, Completion, CompletionError, CompletionRequest, Effort, Failure, Message,
        MessageKind, MessageStatus, Model, Orb, OrbChanges, OrbRun, OrbRunStatus, OrbStart,
        OrbState, Provider, Request, Retention, Run, RunStatus, ToolCall, ToolDefinition,
        ToolResult, WebError, WebPage, WebResult, answer_tools, chats, complete, create_chat,
        create_orb, delete_orb, keep_orbs_running, models, orb_runs, orbs, orbs_held, providers,
        read_chat, read_orb, read_run, rename_chat, resume, run_orb, run_tools, send, send_to_orb,
        set_orb_paused, set_retention, stop, update_orb, web_fetch, web_ready, web_search,
        workspace_tools,
    };
}

/// Recording the microphone and the sound other apps play, and turning
/// speech into text with models on this device. Needs the `speech`
/// capability. Every call returns at once: Noura records and transcribes
/// on its own threads, and the plugin reads the text as it comes.
///
/// ```no_run
/// use plugin_sdk::speech::{self, RecordingOptions, RecordingState, Source};
///
/// let id = speech::start(&RecordingOptions {
///     sources: vec![Source::Microphone, Source::SystemAudio],
///     model: None,
///     microphone: None,
///     language: None,
/// })
/// .unwrap();
/// // Later, as the plugin draws its views again or answers an event:
/// let now = speech::read_recording(&id).unwrap();
/// for segment in &now.segments {
///     let _ = (&segment.channel, segment.start_ms, &segment.text);
/// }
/// speech::stop(&id).unwrap();
/// // Once a `recording-changed` event says it finished:
/// if speech::read_recording(&id).unwrap().state == RecordingState::Finished {
///     speech::forget(&id).unwrap();
/// }
/// ```
pub mod speech {
    pub use crate::noura::plugin::speech::{
        Channel, Microphone, Model, Preferences, Recording, RecordingOptions, RecordingState,
        Segment, Source, cancel, forget, keep_audio, last_dictation, microphones, models,
        read_preferences, read_recording, remember_dictation, save_audio, set_preferences, start,
        stop,
    };
}

/// The keyboard shortcuts the manifest declares under `global-shortcuts`,
/// with the keys you set in Settings. Their presses and releases come to
/// [`Plugin::handle_kernel_event`].
pub mod global_shortcuts {
    pub use crate::noura::plugin::global_shortcuts::{Shortcut, shortcuts};
}

/// Pasting text into the app you're typing in. Needs the `paste`
/// capability, and works only in answer to something you did.
pub mod paste {
    pub use crate::noura::plugin::paste::{Method, copy, paste};
}

/// Entries in Noura's menu bar or tray menu. Needs the `menu-bar-items`
/// capability. A chosen entry comes to [`Plugin::handle_kernel_event`].
pub mod menu_bar {
    pub use crate::noura::plugin::menu_bar::{Entry, EntryKind, set_entries};
}

/// Notifications in the system's notification center. Needs the
/// `notifications` capability, and works only in answer to something you
/// did. A chosen notification comes to [`Plugin::handle_kernel_event`].
pub mod notifications {
    pub use crate::noura::plugin::notifications::{Action, Notification, notify, withdraw};
}

/// Events the plugin tells Noura about, which start the triggers of Orbs
/// that name them, as `<plugin>.<event>`. Needs the `events` capability,
/// whose manifest entry declares each event.
///
/// ```no_run
/// use plugin_sdk::events;
///
/// // A recording ended: Orbs whose triggers say
/// // `event transcription.finished` start.
/// events::emit("finished", Some("transcription_01jd8x3k9q2m7v4c6b1n0p5r8a")).unwrap();
/// ```
pub mod events {
    pub use crate::noura::plugin::events::emit;
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

    /// The nodes of the section a page adds to the sidebar above the file
    /// tree while it shows, or none for no section. Its node IDs don't
    /// repeat the page view's, and its events reach
    /// [`handle_event`](Self::handle_event). Needs `views`.
    fn sidebar(_page: String) -> Vec<Node> {
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

    /// Answer a kernel event: a shortcut press or release, a menu bar
    /// entry, the microphone or a recording. Each needs its capability.
    fn handle_kernel_event(_event: KernelEvent) {}
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
                fn sidebar(page: ::std::string::String) -> ::std::vec::Vec<$crate::Node> {
                    <$ty as $crate::Plugin>::sidebar(page)
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

            impl $crate::__private::KernelEvents for __NouraExports {
                fn handle_kernel_event(event: $crate::KernelEvent) {
                    <$ty as $crate::Plugin>::handle_kernel_event(event)
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
/// // Add bed waits until the bed has a name.
/// view.require(form, "name");
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
///
/// A calendar grid shows the entries it holds in a month, a week or a
/// day. Your own buttons change its mode and period:
///
/// ```
/// use plugin_sdk::{CalendarMode, View};
///
/// let mut view = View::panel("root", Some("Garden diary"));
/// view.button(View::ROOT, "next-week", "Next week");
/// let diary = view.calendar(View::ROOT, "diary", "Diary", CalendarMode::Week, "2026-10-21");
/// view.entry(diary, "frost", "First frost", "2026-10-22", None);
/// view.entry(
///     diary,
///     "market",
///     "Seed market",
///     "2026-10-24T09:00:00-04:00",
///     Some("2026-10-24T12:00:00-04:00"),
/// );
/// assert_eq!(view.into_nodes().len(), 5);
/// ```
///
/// A list can show which item is current, its items can carry a checkbox
/// and a warning, and a name field names something new, or renames an
/// item in its place. A list can open its items on one click, as links:
///
/// ```
/// use plugin_sdk::View;
///
/// let mut view = View::panel("root", Some("Chores"));
/// let views = view.list(View::ROOT, "views", "Views");
/// view.item(views, "today", "Today", None);
/// view.item(views, "all", "All", None);
/// view.select(views, "today");
/// let chores = view.list(View::ROOT, "chores", "Chores");
/// view.open_on_click(chores);
/// let weed = view.checkbox_item(chores, "weed", "Weed the beds", Some("Yesterday"), false);
/// view.warn(weed, "Overdue");
/// let mulch = view.checkbox_item(chores, "mulch", "Mulch", None, false);
/// view.name_field(mulch, "rename", "Chore name", "Mulch");
/// view.name_field(View::ROOT, "new-chore", "New chore name", "");
/// assert_eq!(view.into_nodes().len(), 9);
/// ```
///
/// A conversation holds messages and the steps taken along the way, with
/// a box to write the next message in at the bottom:
///
/// ```
/// use plugin_sdk::{ActivityState, MessageAuthor, View};
///
/// let mut view = View::panel("root", Some("Garden helper"));
/// let talk = view.conversation(View::ROOT, "talk", "Garden helper");
/// view.message(talk, "asked", MessageAuthor::You, "You", "When do I sow beans?");
/// let looked = view.activity(talk, "looked", "Read the sowing calendar", ActivityState::Done);
/// view.markdown(looked, "looked-found", "Beans: after the last frost.");
/// let answer = view.message(talk, "answer", MessageAuthor::Other, "Helper", "After the **last frost**.");
/// view.note(answer, "Stopped.");
/// let composer = view.composer(talk, "next", "Message", "", "Send");
/// view.choice_input(composer, "helper", "Helper", &[("sun", "Sun"), ("rain", "Rain")], Some("sun"));
/// view.stop(composer, "Stop");
/// assert_eq!(view.into_nodes().len(), 8);
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

    /// Add a page's title, with `detail` under it, such as the period
    /// the page shows. Buttons you add to it line up at its end, and the
    /// first one stands out.
    pub fn page_heading(
        &mut self,
        parent: u32,
        id: &str,
        text: &str,
        detail: Option<&str>,
        icon: Option<Icon>,
    ) -> u32 {
        self.heading(parent, id, text, HeadingLevel::Page, detail, icon)
    }

    /// Add the title of a page that shows a period, in the page title's
    /// place: large, `text` in bold and `rest` after it in regular weight,
    /// such as `November` then `2026`. Buttons you add to it line up at
    /// its end, as on a page's title. A calendar grid under it adds no
    /// title of its own.
    pub fn period_heading(&mut self, parent: u32, id: &str, text: &str, rest: Option<&str>) -> u32 {
        self.heading(parent, id, text, HeadingLevel::Period, rest, None)
    }

    /// Add a section's title, with `detail` beside it. Buttons you add to
    /// it line up at its end.
    pub fn section_heading(
        &mut self,
        parent: u32,
        id: &str,
        text: &str,
        icon: Option<Icon>,
    ) -> u32 {
        self.heading(parent, id, text, HeadingLevel::Section, None, icon)
    }

    fn heading(
        &mut self,
        parent: u32,
        id: &str,
        text: &str,
        level: HeadingLevel,
        detail: Option<&str>,
        icon: Option<Icon>,
    ) -> u32 {
        let heading = Heading {
            text: text.to_owned(),
            level,
            detail: detail.map(str::to_owned),
            icon,
        };
        self.add(parent, id, NodeKind::Heading(heading))
    }

    /// Add what an empty page or section shows: its title, a line that
    /// says what to do, and an icon. Buttons you add to it show under
    /// its text.
    pub fn empty_state(
        &mut self,
        parent: u32,
        id: &str,
        title: &str,
        detail: Option<&str>,
        icon: Option<Icon>,
    ) -> u32 {
        let empty = EmptyState {
            title: title.to_owned(),
            detail: detail.map(str::to_owned),
            icon,
        };
        self.add(parent, id, NodeKind::Empty(empty))
    }

    /// Add two panes side by side, the first `first_width` points wide
    /// (160 to 640) with a line on its right, the second taking the rest.
    /// Returns the two panels, to add each pane's nodes to.
    pub fn split(&mut self, parent: u32, id: &str, first_width: u32) -> (u32, u32) {
        let split = self.add(parent, id, NodeKind::Split(first_width));
        let first = self.add(split, &format!("{id}-first"), NodeKind::Panel(None));
        let second = self.add(split, &format!("{id}-second"), NodeKind::Panel(None));
        (first, second)
    }

    /// Add a conversation: a column in the middle of the page that takes
    /// the rest of it and scrolls on its own, staying at its end as it
    /// grows. Add its messages and steps to it, then, last, a
    /// [`composer`](Self::composer), which stays at the bottom of the page.
    pub fn conversation(&mut self, parent: u32, id: &str, label: &str) -> u32 {
        self.add(parent, id, NodeKind::Conversation(label.to_owned()))
    }

    /// Add a message from `author`, named `label` for screen readers.
    /// Yours shows `text` as you wrote it, in a bubble; any other shows it
    /// as Markdown. Give it a quiet line with [`note`](Self::note) and a
    /// problem with [`warn`](Self::warn).
    pub fn message(
        &mut self,
        parent: u32,
        id: &str,
        author: MessageAuthor,
        label: &str,
        text: &str,
    ) -> u32 {
        let message = Message {
            author,
            label: label.to_owned(),
            text: text.to_owned(),
            detail: None,
            warning: None,
        };
        self.add(parent, id, NodeKind::Message(message))
    }

    /// Show `note`, such as `Stopped.`, as a quiet line under the message
    /// at `message`.
    ///
    /// # Panics
    ///
    /// When `message` isn't the index of a message already added.
    pub fn note(&mut self, message: u32, note: &str) {
        match &mut self.nodes[message as usize].kind {
            NodeKind::Message(message) => message.detail = Some(note.to_owned()),
            _ => panic!("only a message shows a note"),
        }
    }

    /// Add a step taken along the way, such as a tool call, in `state`.
    /// The nodes you add to it show while you open it.
    pub fn activity(&mut self, parent: u32, id: &str, title: &str, state: ActivityState) -> u32 {
        let activity = Activity {
            title: title.to_owned(),
            state,
        };
        self.add(parent, id, NodeKind::Activity(activity))
    }

    /// Add a box to write a message in, holding `value`, with a send button
    /// that screen readers read as `send`. Choice inputs and buttons you
    /// add to it line up under the text. Sending sends
    /// [`Event::Submitted`] naming the box, with its text under the box's
    /// own ID, then its choices, and empties the box.
    pub fn composer(&mut self, parent: u32, id: &str, label: &str, value: &str, send: &str) -> u32 {
        let composer = Composer {
            label: label.to_owned(),
            value: value.to_owned(),
            send: send.to_owned(),
            stop: None,
        };
        self.add(parent, id, NodeKind::Composer(composer))
    }

    /// Put a stop button labeled `label` in the send button's place in the
    /// composer at `composer`, while something is under way. A press sends
    /// [`Event::Pressed`] naming the composer.
    ///
    /// # Panics
    ///
    /// When `composer` isn't the index of a composer already added.
    pub fn stop(&mut self, composer: u32, label: &str) {
        match &mut self.nodes[composer as usize].kind {
            NodeKind::Composer(composer) => composer.stop = Some(label.to_owned()),
            _ => panic!("only a composer shows a stop button"),
        }
    }

    /// Add a list you select items in. Add its items with
    /// [`item`](Self::item); selecting one sends [`Event::Selected`], and
    /// opening one sends [`Event::Activated`].
    pub fn list(&mut self, parent: u32, id: &str, label: &str) -> u32 {
        let list = ItemList {
            label: label.to_owned(),
            reorder: false,
            selected: None,
            opens_on_click: false,
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
            selected: None,
            opens_on_click: false,
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
            checked: None,
            warning: None,
            color: None,
            avatar: None,
            status: None,
        };
        self.add(column, id, NodeKind::Card(card))
    }

    /// Add an item to the list at `list`, with an optional quieter detail
    /// after its label, on the same line.
    pub fn item(&mut self, list: u32, id: &str, label: &str, detail: Option<&str>) -> u32 {
        let item = Item {
            label: label.to_owned(),
            detail: detail.map(str::to_owned),
            checked: None,
            warning: None,
            color: None,
            avatar: None,
            status: None,
        };
        self.add(list, id, NodeKind::Item(item))
    }

    /// Add an item like [`item`](Self::item) with a checkbox before its
    /// label. Checking or unchecking it sends [`Event::Checked`]; the box
    /// shows what you draw next.
    pub fn checkbox_item(
        &mut self,
        list: u32,
        id: &str,
        label: &str,
        detail: Option<&str>,
        checked: bool,
    ) -> u32 {
        let item = Item {
            label: label.to_owned(),
            detail: detail.map(str::to_owned),
            checked: Some(checked),
            warning: None,
            color: None,
            avatar: None,
            status: None,
        };
        self.add(list, id, NodeKind::Item(item))
    }

    /// Show `warning`, such as `Overdue`, after the label of the list item
    /// at `item`, or under the message at `item`, in the color Noura shows
    /// problems in. Screen readers read it with the label.
    ///
    /// # Panics
    ///
    /// When `item` isn't the index of a list item or a message already
    /// added.
    pub fn warn(&mut self, item: u32, warning: &str) {
        match &mut self.nodes[item as usize].kind {
            NodeKind::Item(item) => item.warning = Some(warning.to_owned()),
            NodeKind::Message(message) => message.warning = Some(warning.to_owned()),
            _ => panic!("only a list item or a message shows a warning"),
        }
    }

    /// Show the item with the node ID `item` selected in the list at
    /// `list`, such as the view you're in. The list's selection moves
    /// there when you draw another item selected than you drew before.
    ///
    /// # Panics
    ///
    /// When `list` isn't the index of a list already added.
    pub fn select(&mut self, list: u32, item: &str) {
        match &mut self.nodes[list as usize].kind {
            NodeKind::List(list) => list.selected = Some(item.to_owned()),
            _ => panic!("only a list selects an item"),
        }
    }

    /// Let one click open an item of the list at `list`, as a link does:
    /// the click sends [`Event::Activated`] after any
    /// [`Event::Selected`]. The arrow keys still only select.
    ///
    /// # Panics
    ///
    /// When `list` isn't the index of a list already added.
    pub fn open_on_click(&mut self, list: u32) {
        match &mut self.nodes[list as usize].kind {
            NodeKind::List(list) => list.opens_on_click = true,
            _ => panic!("only a list opens its items on a click"),
        }
    }

    /// Add a field that names something, holding `value`: empty for
    /// something new, the current name for a rename. Its parent is a
    /// panel or a form, or a list item it shows in place of, as for a
    /// rename. It takes the keyboard when it appears. Enter or leaving it sends
    /// [`Event::Named`] with a new name, and Escape, or leaving it empty
    /// or unchanged, sends [`Event::Cancelled`].
    pub fn name_field(&mut self, parent: u32, id: &str, label: &str, value: &str) -> u32 {
        let field = NameField {
            label: label.to_owned(),
            value: value.to_owned(),
        };
        self.add(parent, id, NodeKind::NameField(field))
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
    /// inside it; submitting, by the button or Enter in a single-line text
    /// input, sends [`Event::Submitted`] with their values.
    pub fn form(&mut self, parent: u32, id: &str, label: &str, submit: &str) -> u32 {
        let form = Form {
            label: label.to_owned(),
            submit: submit.to_owned(),
            required: Vec::new(),
        };
        self.add(parent, id, NodeKind::Form(form))
    }

    /// Keep the form at `form` from being submitted until its text input
    /// with the node ID `input` holds more than spaces. Its submit button
    /// shows disabled meanwhile.
    ///
    /// # Panics
    ///
    /// When `form` isn't the index of a form already added.
    pub fn require(&mut self, form: u32, input: &str) {
        match &mut self.nodes[form as usize].kind {
            NodeKind::Form(form) => form.required.push(input.to_owned()),
            _ => panic!("only a form requires an input"),
        }
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

    /// Add a calendar grid in `mode` that shows the period around `date`,
    /// a `YYYY-MM-DD` date. Add its entries with [`entry`](Self::entry).
    pub fn calendar(
        &mut self,
        parent: u32,
        id: &str,
        label: &str,
        mode: CalendarMode,
        date: &str,
    ) -> u32 {
        let calendar = Calendar {
            label: label.to_owned(),
            mode,
            date: date.to_owned(),
        };
        self.add(parent, id, NodeKind::Calendar(calendar))
    }

    /// Add an entry to the calendar grid at `calendar`. An all-day entry
    /// starts on a `YYYY-MM-DD` date and ends before its `end` date; a
    /// timed one starts and ends at RFC 3339 dates and times with an
    /// offset. Selecting it sends [`Event::Selected`] and opening it
    /// [`Event::Activated`], naming the calendar.
    pub fn entry(
        &mut self,
        calendar: u32,
        id: &str,
        title: &str,
        start: &str,
        end: Option<&str>,
    ) -> u32 {
        let entry = CalendarEntry {
            title: title.to_owned(),
            start: start.to_owned(),
            end: end.map(str::to_owned),
            color: None,
        };
        self.add(calendar, id, NodeKind::Entry(entry))
    }

    /// Add text you edit in place, holding `value`. A
    /// [`title`](EditableStyle::Title), a [`line`](EditableStyle::Line) or
    /// a [`body`](EditableStyle::Body) sends [`Event::Changed`] with its
    /// new text when you leave it, or press Return in a title or a line; a
    /// line sends it trimmed, and may send it empty. An
    /// [`add`](EditableStyle::Add) field sends it on Return, then empties
    /// itself and keeps the keyboard, so you can add one thing after
    /// another. With `focus`, it takes the keyboard when it appears, or
    /// when you draw it with `focus` after drawing it without.
    pub fn editable(
        &mut self,
        parent: u32,
        id: &str,
        label: &str,
        value: &str,
        style: EditableStyle,
        focus: bool,
    ) -> u32 {
        let editable = Editable {
            label: label.to_owned(),
            value: value.to_owned(),
            style,
            focus,
        };
        self.add(parent, id, NodeKind::Editable(editable))
    }

    /// Add a button that shows `value`, a `YYYY-MM-DD` date, or `label`
    /// while it holds none, and opens a date picker with Today, Tomorrow,
    /// Next week, a month to pick a day from and Clear. A pick sends
    /// [`Event::Changed`] with [`Value::Date`].
    pub fn date_picker(&mut self, parent: u32, id: &str, label: &str, value: Option<&str>) -> u32 {
        let picker = DatePicker {
            label: label.to_owned(),
            value: value.map(str::to_owned),
        };
        self.add(parent, id, NodeKind::DatePicker(picker))
    }

    /// Add a button that shows the chosen option and opens a menu of
    /// `options`, each an ID, a label and an optional color for a mark
    /// before the label. A pick sends [`Event::Changed`] with
    /// [`Value::Choice`].
    pub fn picker(
        &mut self,
        parent: u32,
        id: &str,
        label: &str,
        options: &[(&str, &str, Option<Color>)],
        chosen: Option<&str>,
    ) -> u32 {
        let picker = Picker {
            label: label.to_owned(),
            options: options
                .iter()
                .map(|(id, label, color)| PickerOption {
                    id: (*id).to_owned(),
                    label: (*label).to_owned(),
                    color: *color,
                })
                .collect(),
            chosen: chosen.map(str::to_owned),
        };
        self.add(parent, id, NodeKind::Picker(picker))
    }

    /// Add a popover that floats beside the item, card, entry or button
    /// with the node ID `anchor`, with an arrow toward it. Its children go
    /// where a panel's go. Escape or a click outside it sends
    /// [`Event::Cancelled`] with the popover's ID.
    pub fn popover(&mut self, parent: u32, id: &str, label: &str, anchor: &str) -> u32 {
        let popover = Popover {
            label: label.to_owned(),
            anchor: anchor.to_owned(),
        };
        self.add(parent, id, NodeKind::Popover(popover))
    }

    /// Draw the list item or the calendar entry at `node` in `color`: an
    /// item's checkbox, or a dot before its label when it has none, and
    /// an entry's fill.
    ///
    /// # Panics
    ///
    /// When `node` isn't the index of a list item or a calendar entry
    /// already added.
    pub fn color(&mut self, node: u32, color: Color) {
        match &mut self.nodes[node as usize].kind {
            NodeKind::Item(item) => item.color = Some(color),
            NodeKind::Entry(entry) => entry.color = Some(color),
            _ => panic!("only a list item or a calendar entry takes a color"),
        }
    }

    /// Show `avatar` before the label of the list item at `item`, in place
    /// of a color's dot.
    ///
    /// # Panics
    ///
    /// When `item` isn't the index of a list item already added.
    pub fn avatar(&mut self, item: u32, avatar: Avatar) {
        match &mut self.nodes[item as usize].kind {
            NodeKind::Item(item) => item.avatar = Some(avatar),
            _ => panic!("only a list item shows an avatar"),
        }
    }

    /// Show a status dot in `color` at the end of the list item at
    /// `item`, which screen readers read as `label`, such as `Working`.
    ///
    /// # Panics
    ///
    /// When `item` isn't the index of a list item already added.
    pub fn status(&mut self, item: u32, label: &str, color: Color) {
        match &mut self.nodes[item as usize].kind {
            NodeKind::Item(item) => {
                item.status = Some(Status {
                    label: label.to_owned(),
                    color,
                });
            }
            _ => panic!("only a list item shows a status"),
        }
    }

    /// Add a large avatar, centered, as at the top of a profile.
    pub fn large_avatar(&mut self, parent: u32, id: &str, avatar: Avatar) -> u32 {
        self.add(parent, id, NodeKind::Avatar(avatar))
    }

    /// Add a pill button with `avatar` and `label`, which shows pressed
    /// while what it opens is `open`. A press sends [`Event::Pressed`].
    /// The page's first node, when a pill, shows centered at the top of
    /// the page in place of its title.
    pub fn pill(
        &mut self,
        parent: u32,
        id: &str,
        label: &str,
        avatar: Option<Avatar>,
        open: bool,
    ) -> u32 {
        let pill = Pill {
            label: label.to_owned(),
            avatar,
            open,
        };
        self.add(parent, id, NodeKind::Pill(pill))
    }

    /// Add a side panel, `width` points wide (240 to 480), down the
    /// page's right edge, named `label` for screen readers. Add it to the
    /// view's own panel, last. Returns it, to add its nodes to.
    pub fn side_panel(&mut self, id: &str, label: &str, width: u32) -> u32 {
        let panel = SidePanel {
            label: label.to_owned(),
            width,
        };
        self.add(Self::ROOT, id, NodeKind::SidePanel(panel))
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
