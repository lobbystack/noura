//! One test binary for the crate's integration tests. Each module was
//! a test binary of its own, so `--test it <module>::` selects its tests.

// `editor-core`'s syntax corpus, which `caret_walk` walks through.
#[path = "../../../editor-core/tests/it/corpus/mod.rs"]
mod corpus;
mod support;

mod accesskit;
mod byte_identity;
mod caret_walk;
mod drawn_colors;
mod editing_commands;
mod editor_kittest;
mod editor_paint;
mod images;
mod ime;
mod large_note;
mod math;
mod obsidian_drawing;
mod preview_parity;
mod spelling;
mod tables;
mod text_size;
