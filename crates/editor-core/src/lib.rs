//! The Markdown editor's text: its buffer, offsets, parsing, reveal rules
//! and edit commands.
//!
//! This crate has no UI dependency; `editor-egui` draws and drives it.
//! The editor comes from Lockbook's editor; see
//! `docs/provenance/lockbook-editor.md`.
//!
//! [`doc::MdDoc`] is the entry point: it holds a note's [`buffer::Buffer`],
//! parses it with comrak, and turns [`input::Event`]s into edits.

pub mod blocks;
pub mod bounds;
pub mod buffer;
pub mod constructs;
pub mod diff;
pub mod doc;
pub mod fold;
pub mod images;
pub mod input;
pub mod links;
pub mod nodes;
pub mod offset_types;
pub mod operation_types;
pub mod reorder;
pub mod resolvers;
pub mod reveal;
pub mod text_units;
pub mod unicode_segs;

pub use comrak;
pub use doc::MdDoc;
