//! Widgets the editor draws with: the virtualized scroll area, the caret and
//! the glyphon text pipeline.

pub mod affine_scroll;
pub mod caret;
pub mod glyphon_cache;
pub mod glyphon_render;

pub use caret::{paint_caret, with_blinking_caret};
