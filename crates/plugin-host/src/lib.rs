//! Runs plugins in Wasmtime and enforces the capabilities each one declares.
//!
//! A plugin is a folder holding a manifest, `plugin.toml`, and a
//! WebAssembly component built against `noura:plugin`, whose WIT lives in
//! `plugin-sdk`. [`PluginHost`] reads such a folder, compiles the
//! component on this device and keeps it in its library, and starts each
//! plugin in its own store with the capabilities you approved. The
//! services behind the capability interfaces, such as [`WorkspaceReads`],
//! come from the app. Object types that plugins register go in one
//! [`ObjectTypes`] registry per workspace.

mod error;
mod host;
mod instance;
mod manifest;
mod object_types;
mod runtime;
mod services;
pub mod trial;
mod view;

pub use error::{InstallError, InstallErrorCode};
pub use host::{Candidate, CodeSource, LoadedPlugin, PluginHost};
pub use instance::{
    Event, Node, NodeKind, ObjectType, Page, PluginError, PluginInstance, Registration, Services,
};
pub use manifest::{
    Capability, INTERFACE_VERSIONS, MANIFEST_FILE, Platform, PluginManifest, read_manifest,
    valid_plugin_id,
};
pub use object_types::{KERNEL_TYPES, ObjectTypes, TypeError};
pub use runtime::{Backend, Limits};
pub use services::{
    Entry, EntryKind, FileRead, Listed, NewObject, ObjectChanges, ObjectRead, ObjectRecord,
    PropertyProblem, ServiceError, WorkspaceObjects, WorkspacePath, WorkspaceReads,
};
pub use view::{MAX_DEPTH, MAX_NODES, View, ViewError, ViewKind, ViewNode};
