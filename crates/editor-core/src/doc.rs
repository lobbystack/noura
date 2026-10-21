//! The text side of a Markdown note in the editor: its buffer, parse,
//! bounds, reveal state and edit commands. `editor-egui`'s renderer holds
//! one and adds layout, painting and input translation.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use comrak::nodes::AstNode;
use comrak::{Arena, Options};

use crate::bounds::Bounds;
use crate::buffer::{self, Buffer};
use crate::input::cursor::CursorState;
use crate::input::{Event, TextLayout};
use crate::offset_types::{Grapheme, Graphemes};
use crate::resolvers::{LinkResolver, NotFound, OpenedLink};

pub struct MdDoc {
    // document
    pub buffer: Buffer,
    /// Bumped on every text change; text-derived caches key off it.
    pub text_seq: u64,
    pub bounds: Bounds,
    pub bounds_seq: u64,
    /// The counter `text_seq`, `bounds_seq` and `reveal_seq` draw from. A
    /// renderer shares it with its own caches.
    pub ws_seq: Arc<AtomicU64>,
    /// Caches of values derived from the text and reveal state.
    pub cache: TextCache,

    // reveal inputs
    /// The selection that reveals syntax, or `None` while the editor isn't
    /// focused.
    pub reveal_selection: Option<(Grapheme, Grapheme)>,
    /// A preview atom entered via `Event::EnterAtom` whose whole range is
    /// selected (a bare autolink — its URL *is* its full source, so no
    /// selection endpoint can land interior). Force-reveals the source;
    /// re-resolved each frame and cleared when the selection leaves it.
    pub entered_atom: Option<(Grapheme, Grapheme)>,
    pub find_current_match: Option<(Grapheme, Grapheme)>,
    /// Read-only search-preview highlight: the snippet range to reveal.
    pub preview_match: Option<(Grapheme, Grapheme)>,
    pub reveal_seq: u64, // includes selection and find matches
    /// Spoilers the user has revealed, by source range. Cleared whenever the
    /// text changes so a fresh edit re-hides every spoiler.
    pub revealed_spoilers: HashSet<(Grapheme, Grapheme)>,

    // modes
    pub readonly: bool,  // disables task checkboxes and saving
    pub plaintext: bool, // render as source text, not parsed markdown
    pub disable_images: bool,

    // editing
    pub cursor: CursorState,
    /// Answers link lookups. Defaults to [`NotFound`].
    pub link_resolver: Box<dyn LinkResolver>,
    /// Links the user opened, for the embedding app to act on.
    pub opened_links: Vec<OpenedLink>,
    /// Text a copy or cut put on the clipboard, for the UI to hand to the
    /// system clipboard.
    pub copied: Option<String>,
    /// Events an edit queued to apply after the current batch, such as the
    /// fold-tag edit a fold command makes.
    pub deferred_events: Vec<crate::input::Event>,
    pub debug: bool,
}

/// A node and one of its source lines.
pub type LinePrefixKey = (u64, (Grapheme, Grapheme));
/// The prefix length the node owns on that line, and whether it's complete.
pub type LinePrefixValue = (Graphemes, bool);

/// Values derived from the text, cached per parse. Wiped whenever the text
/// changes ([`TextCache::ensure_text_consistent`]).
#[derive(Default)]
pub struct TextCache {
    pub line_prefix_len: RefCell<HashMap<LinePrefixKey, LinePrefixValue>>,
    pub node_range: RefCell<HashMap<u64, (Grapheme, Grapheme)>>,
    /// Depends on `reveal_seq` too: `hidden_by_fold_deps` holds the
    /// `reveal_seq` it was last populated at, or `None` if never.
    pub hidden_by_fold: RefCell<HashMap<u64, bool>>,
    pub hidden_by_fold_deps: Cell<Option<u64>>,
    /// The `text_seq` this cache was last consistent with.
    pub text_seq: Cell<u64>,
}

impl TextCache {
    pub fn clear(&self) {
        self.line_prefix_len.borrow_mut().clear();
        self.node_range.borrow_mut().clear();
        self.hidden_by_fold.borrow_mut().clear();
        self.hidden_by_fold_deps.set(None);
    }

    /// Wipe and re-stamp if `current` differs from the recorded text_seq.
    pub fn ensure_text_consistent(&self, current: u64) {
        if self.text_seq.get() != current {
            self.clear();
            self.text_seq.set(current);
        }
    }
}

impl MdDoc {
    /// A document holding `text`, with its own sequence counter.
    pub fn new(text: &str) -> Self {
        Self::with_seq(text, Arc::new(AtomicU64::new(1)))
    }

    /// A document holding `text` that draws sequence numbers from `ws_seq`.
    pub fn with_seq(text: &str, ws_seq: Arc<AtomicU64>) -> Self {
        Self {
            buffer: text.into(),
            text_seq: 0,
            bounds: Default::default(),
            bounds_seq: 0,
            ws_seq,
            cache: Default::default(),
            reveal_selection: None,
            entered_atom: None,
            find_current_match: None,
            preview_match: None,
            reveal_seq: 0,
            revealed_spoilers: Default::default(),
            readonly: false,
            plaintext: false,
            disable_images: false,
            cursor: Default::default(),
            link_resolver: Box::new(NotFound),
            opened_links: Vec::new(),
            copied: None,
            deferred_events: Vec::new(),
            debug: false,
        }
    }

    /// The note's text.
    pub fn text(&self) -> &str {
        self.buffer.current.text()
    }

    /// The selection as `(anchor, caret)`.
    pub fn selection(&self) -> (Grapheme, Grapheme) {
        self.buffer.current.selection
    }

    /// The changes made to the text since the last call, in order: one per
    /// replacement the buffer applied. Replaying them on the text the note
    /// opened with gives its text now.
    pub fn take_changes(&mut self) -> Vec<crate::buffer::TextChange> {
        self.buffer.take_changes()
    }

    /// Applies a batch of events the way one frame of input applies them:
    /// the edits they make undo together. `layout` answers the questions an
    /// edit asks about where text was drawn; [`NoLayout`] answers for a
    /// document with no UI. Events an edit defers, such as the tag a fold
    /// command writes, apply in further batches.
    ///
    /// [`NoLayout`]: crate::input::NoLayout
    pub fn apply(
        &mut self,
        events: impl IntoIterator<Item = Event>,
        layout: &dyn TextLayout,
    ) -> buffer::Response {
        let mut response = self.apply_batch(events, layout);
        // A deferred edit can't defer another forever; stop after a few.
        for _ in 0..8 {
            if self.deferred_events.is_empty() {
                break;
            }
            let deferred = std::mem::take(&mut self.deferred_events);
            response |= self.apply_batch(deferred, layout);
        }
        response
    }

    fn apply_batch(
        &mut self,
        events: impl IntoIterator<Item = Event>,
        layout: &dyn TextLayout,
    ) -> buffer::Response {
        let arena = Arena::new();
        let root = self.reparse(&arena);
        let mut ops = Vec::new();
        let mut response = buffer::Response::default();
        let mut undo_redo = false;
        for event in events {
            undo_redo |= matches!(event, Event::Undo | Event::Redo);
            response |= self.calc_operations(root, event, &mut ops, layout);
        }
        self.buffer.queue(ops);
        response |= self.buffer.update();
        if response.text_updated {
            self.bump_text_seq();
        }
        let edited = response.text_updated || response.selection_user_moved;
        response |= self.unfold_after_edit(&arena, edited, undo_redo);
        response
    }

    /// The next value of the shared sequence counter.
    pub fn next_seq(&self) -> u64 {
        self.ws_seq.fetch_add(1, Ordering::Relaxed)
    }

    /// Bump `text_seq`. Call after any buffer mutation that may change
    /// text — caches and find-match invalidation key off this counter.
    pub fn bump_text_seq(&mut self) {
        self.text_seq = self.next_seq();
        self.revealed_spoilers.clear();
    }

    /// Bump `reveal_seq` after the reveal inputs change.
    pub fn bump_reveal_seq(&mut self) {
        self.reveal_seq = self.next_seq();
    }

    /// Re-parse the buffer into a fresh AST and rebuild all text-derived
    /// bounds (source lines, words, inline paragraphs). The caller owns the
    /// [`Arena`] — comrak's AST is arena-allocated.
    pub fn reparse<'a>(&mut self, arena: &'a Arena<'a>) -> &'a AstNode<'a> {
        self.cache.ensure_text_consistent(self.text_seq);

        let options = Self::comrak_options();
        let text_with_newline = self.buffer.current.text_with("\n");
        let root = comrak::parse_document(arena, &text_with_newline, &options);

        // Bounds depend only on text. Skip rebuild + bump bounds_seq
        // when text hasn't changed.
        if self.bounds.text_seq != self.text_seq {
            self.bounds.inline_paragraphs.clear();
            self.calc_source_lines();
            self.calc_fold_bounds(root);
            self.calc_atom_bounds(root);
            // Populate before compute_bounds: pre_spacing_lines (called
            // from compute_bounds_block_pre_spacing) queries
            // hidden_by_fold, which expects every node already cached.
            self.populate_hidden_by_fold(root);
            self.compute_bounds(root);
            self.bounds.inline_paragraphs.sort();
            self.calc_words();
            self.calc_syntax_words(root);
            self.bounds.text_seq = self.text_seq;
            self.bounds_seq = self.next_seq();
        } else {
            // Still call populate — its own deps check skips when
            // text+reveal haven't moved, but this catches reveal-only
            // changes (text seq same, reveal_seq advanced).
            self.populate_hidden_by_fold(root);
        }

        root
    }

    /// How the editor parses notes. The extensions on are the syntax
    /// today's editor draws. Lockbook's others (spoilers, emoji
    /// shortcodes, superscript, subscript, `__` as underline, code math)
    /// stay off, so their text shows as written.
    pub fn comrak_options() -> Options<'static> {
        let mut options = Options::default();
        options.parse.smart = true;
        options.parse.ignore_setext = false;
        options.extension.alerts = true;
        options.extension.autolink = true;
        options.extension.description_lists = false;
        options.extension.footnotes = false;
        options.extension.front_matter_delimiter = Some("---".to_string());
        options.extension.greentext = false;
        options.extension.header_ids = None; // intended for HTML renderers
        options.extension.highlight = true;
        options.extension.math_code = false;
        options.extension.math_dollars = true;
        options.extension.multiline_block_quotes = false;
        options.extension.shortcodes = false;
        options.extension.spoiler = false;
        options.extension.strikethrough = true;
        options.extension.subscript = false;
        options.extension.superscript = false;
        options.extension.table = true;
        options.extension.tagfilter = false; // intended for HTML renderers
        options.extension.tasklist = true;
        options.extension.underline = false; // `__` is strong, as CommonMark reads it
        options.extension.wikilinks_title_after_pipe = true; // matches obsidian
        options.extension.wikilinks_title_before_pipe = false; // would not match obsidian
        options.render.escaped_char_spans = true;
        options
    }
}
