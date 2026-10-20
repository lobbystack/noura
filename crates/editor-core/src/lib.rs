//! The Markdown editor's text: its buffer, offsets, parsing, reveal rules
//! and edit commands.
//!
//! This crate has no UI dependency; `editor-egui` draws and drives it.
//! Its design comes from Lockbook's editor; see
//! `docs/provenance/lockbook-editor.md`.

pub mod buffer;
pub mod diff;
pub mod offset_types;
pub mod operation_types;
pub mod resolvers;
pub mod unicode_segs;
