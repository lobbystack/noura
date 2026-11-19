//! One test binary for the crate's integration tests. Each module was
//! a test binary of its own, so `--test it <module>::` selects its tests.

mod corpus;

mod buffer;
mod callouts;
mod caret_walk;
mod changes;
mod code;
mod commands;
mod constructs;
mod embeds;
mod find;
mod formatting;
mod history;
mod images;
mod ime;
mod incremental_parse;
mod large_notes;
mod link_completion;
mod links;
mod lockbook_behaviors;
mod lookahead;
mod math;
mod obsidian_inline;
mod outside_edits;
mod plain_text;
mod reveal;
mod shortcuts;
mod tables;
mod typing;
mod wikilinks;
