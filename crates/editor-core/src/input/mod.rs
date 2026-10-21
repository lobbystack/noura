pub mod advance;
pub mod cursor;
pub mod mutation;

use comrak::nodes::NodeValue;

use crate::offset_types::Grapheme;

/*
 * This module processes input events, with the following major concerns:
 * * Plumbing: combining programmatic and UI input, delegating to appropriate handlers
 * * Enrichment: did the user click on a link, or select a word, or drag a selection?
 * * Buffer manipulation: text replacements & cursor movements, operational transformation, merging concurrent edits
 */

/// text location
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Location {
    CurrentCursor,
    Grapheme(Grapheme),
    Pos(Pos),
}

/// text unit that has a start and end location
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Bound {
    Word,
    Line,
    Paragraph,
    Doc,
}

/// text unit you can increment or decrement a location by
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Increment {
    Char,
    Lines(usize),
}

/// text location relative to some absolute text location
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Advance {
    /// text location at a bound; if you're already there, this leaves you there
    /// (e.g. cmd+left/right)
    To(Bound),

    /// text location at the next bound; if you're already there, this goes to
    /// the next one (e.g. option+left/right)
    Next(Bound),

    /// text location some increment away; if you're in the middle of one of
    /// these, this goes somewhere in the middle of the next one (e.g. up/down)
    By(Increment),
}

/// text region specified in some manner
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Region {
    /// 0-length region starting and ending at location
    Location(Location),

    /// Text from secondary cursor to location. Preserves selection.
    ToLocation(Location),

    /// Text from one location to another
    BetweenLocations { start: Location, end: Location },

    /// Currently selected text
    Selection,

    /// Currently selected text, or if the selection is empty, text from the
    /// primary cursor to one char/line before/after or to start/end of
    /// word/line/doc
    SelectionOrAdvance { advance: Advance, backwards: bool },

    /// Text from primary cursor to one char/line before/after or to start/end
    /// of word/line/paragraph/doc. In some situations this instead represents
    /// the start of selection (if `backwards`) or end of selection, based on
    /// what feels intuitive when using arrow keys to navigate a document.
    ToAdvance {
        advance: Advance,
        backwards: bool,
        extend_selection: bool,
    },

    /// Current word/line/paragraph/doc, preferring previous word if `backwards`
    Bound { bound: Bound, backwards: bool },

    /// Word/line/paragraph/doc at a location, preferring previous word if `backwards`
    BoundAt {
        bound: Bound,
        location: Location,
        backwards: bool,
    },
}

/// Standardized edits to any editor state e.g. buffer, clipboard, debug state.
/// Interpretation may depend on render state e.g. galley positions, line wrap.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Select {
        region: Region,
    },
    Replace {
        region: Region,
        text: String,
        advance_cursor: bool,
    }, // replace region with text and optionally advance cursor to end of new text
    ToggleStyle {
        region: Region,
        style: NodeValue,
    }, // supports toolbar and inline tyle keyboard shortcuts
    Newline {
        shift: bool,
    }, // distinct from replace because it triggers auto-bullet, etc
    Delete {
        region: Region,
    }, // distinct from replace because it triggers numbered list renumber, etc
    Indent {
        deindent: bool,
    }, // distinct from replace because it's a no-op for first list item, etc
    Undo,
    Redo,
    Cut,
    Copy,
    ToggleDebug,
    IncrementBaseFontSize,
    DecrementBaseFontSize,
    ToggleFold {
        node: Option<(Grapheme, Grapheme)>,
    }, // the block whose range this is, else the selection's blocks
    EnterAtom, // select the URL inside the selected atom (e.g. image), revealing its source; the touch path in, since mobile has no arrow keys
    ToggleSpoiler {
        node: (Grapheme, Grapheme),
    }, // reveal or hide the spoiler whose range this is
    OpenLink {
        url: String,
        wikilink: bool,
    }, // navigate, on the platform
    Paste {
        text: String,
    }, // replace the selection with pasted text, or link the selection to a pasted URL
    ToggleTask {
        node: (Grapheme, Grapheme),
    }, // check or uncheck the task item whose range this is
    Unfold {
        tag: (Grapheme, Grapheme),
    }, // remove the fold tag at this range, showing the section it hid
}

impl From<(Grapheme, Grapheme)> for Region {
    fn from((start, end): (Grapheme, Grapheme)) -> Self {
        Region::BetweenLocations {
            start: Location::Grapheme(start),
            end: Location::Grapheme(end),
        }
    }
}

/// A point in the editor's layout, in the coordinates the layout reports.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pos {
    pub x: f32,
    pub y: f32,
}

// Positions compare by value; NaN never reaches here from a pointer.
impl Eq for Pos {}

/// Where the layout drew the text, for edits that start from a point or move
/// by visual rows. `editor-egui` answers from what it painted last frame.
pub trait TextLayout {
    /// The range a click at `pos` selects: a cursor position, or a whole
    /// atomic span such as a list marker.
    fn pos_to_range(&self, pos: Pos) -> (Grapheme, Grapheme);

    /// The cursor position nearest `pos`.
    fn pos_to_offset(&self, pos: Pos) -> Grapheme;

    /// The x coordinate of a caret at `offset`, or `None` if the layout
    /// hasn't placed it.
    fn offset_x(&self, offset: Grapheme) -> Option<f32>;

    /// The cursor position one visual row above (`backwards`) or below
    /// `offset`, nearest `x_target`.
    fn offset_by_row(&self, offset: Grapheme, x_target: f32, backwards: bool) -> Grapheme;

    /// Whether this layout placed no rows, so vertical moves step over
    /// source lines and hold a column counted in graphemes instead.
    fn rows_are_source_lines(&self) -> bool {
        false
    }
}

/// A layout that placed nothing. Points resolve to the start of the text,
/// and vertical moves step over source lines instead of visual rows.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoLayout;

impl TextLayout for NoLayout {
    fn pos_to_range(&self, _pos: Pos) -> (Grapheme, Grapheme) {
        (Grapheme(0), Grapheme(0))
    }

    fn pos_to_offset(&self, _pos: Pos) -> Grapheme {
        Grapheme(0)
    }

    fn offset_x(&self, _offset: Grapheme) -> Option<f32> {
        None
    }

    fn offset_by_row(&self, offset: Grapheme, _x_target: f32, _backwards: bool) -> Grapheme {
        offset
    }

    fn rows_are_source_lines(&self) -> bool {
        true
    }
}
