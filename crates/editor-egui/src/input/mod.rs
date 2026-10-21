//! Input. `editor-core` owns the events and the edit commands they make;
//! this module translates egui input into events, answers the layout
//! questions edits ask, and draws the caret and selection.

pub use editor_core::input::{
    Advance, Bound, Event, Increment, Location, NoLayout, Pos, Region, TextLayout, advance,
    mutation,
};

pub mod canonical;
pub mod cursor;
pub mod layout;

/// A point in egui's coordinates as `editor-core` sees it.
pub fn pos(pos: egui::Pos2) -> Pos {
    Pos { x: pos.x, y: pos.y }
}
