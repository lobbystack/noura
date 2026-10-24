//! Runs plugins in Wasmtime and enforces the capabilities each one declares.
//!
//! A plugin is a folder holding a manifest, `plugin.toml`, and a
//! WebAssembly component built against `noura:plugin`, whose WIT lives in
//! `plugin-sdk`. [`PluginHost`] reads such a folder, compiles the
//! component on this device and keeps it in its library, and starts each
//! plugin as a [`RunningPlugin`] with the capabilities you approved. Each
//! call runs in the plugin's own store, under a memory cap and a deadline.
//! The services behind the capability interfaces, such as
//! [`WorkspaceReads`] and `local-core`'s web access boundary, come from the
//! app, and a [`Router`] carries one plugin's command calls to another.
//! Object types that plugins register go in one [`ObjectTypes`] registry
//! per workspace.

mod error;
mod host;
mod instance;
mod manifest;
mod object_types;
mod running;
mod runtime;
mod services;
pub mod trial;
mod view;

pub use error::{InstallError, InstallErrorCode};
pub use host::{Candidate, CodeSource, LoadedPlugin, PluginHost};
pub use instance::{
    Calendar, CalendarEntry, CalendarMode, Choice, Column, ColumnPress, Command, Event, Field,
    Form, InboxSection, Input, Item, ItemList, MenuItem, Move, Node, NodeKind, ObjectType, Page,
    PluginError, Registration, Services, SortOrder, Submission, Table, Target, Tool, Value,
    VirtualList, VirtualRow,
};
pub use manifest::{
    Capability, Grant, INTERFACE_VERSIONS, MANIFEST_FILE, Platform, PluginManifest, read_manifest,
    valid_command_id, valid_plugin_id,
};
pub use object_types::{KERNEL_TYPES, ObjectTypes, TypeError};
pub use running::{
    CalledListener, FAILURE_WINDOW, Health, MAX_FAILURES, Router, RunningPlugin, Surface,
};
pub use runtime::{Backend, Limits};
pub use services::{
    Entry, EntryKind, FileRead, Listed, NewObject, ObjectChanges, ObjectRead, ObjectRecord,
    PropertyProblem, ServiceError, WorkspaceObjects, WorkspacePath, WorkspaceReads,
};
pub use view::{MAX_DEPTH, MAX_NODES, MAX_ROWS, View, ViewError, ViewKind, ViewNode, check_rows};
