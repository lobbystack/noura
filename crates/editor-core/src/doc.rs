//! The text side of a Markdown note in the editor: its buffer, parse,
//! bounds, reveal state and edit commands. `editor-egui`'s renderer holds
//! one and adds layout, painting and input translation.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use comrak::nodes::AstNode;
use comrak::{Arena, Options};

use crate::ast::Ast;
use crate::background::{BackgroundParser, Wake};
use crate::bounds::Bounds;

/// Notes at least this long reparse edits in place and parse in full in
/// the background. A full parse of a note this size takes well under a
/// millisecond.
pub const LARGE_NOTE_BYTES: usize = 64 * 1024;

/// A run of top-level blocks an in-place reparse replaced, between two
/// versions of the bounds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockEdit {
    /// `bounds_seq` before the reparse, and after it.
    pub from_bounds_seq: u64,
    pub to_bounds_seq: u64,
    /// The blocks replaced, as indices among the root's children before.
    pub old_blocks: std::ops::Range<usize>,
    /// The blocks that replaced them, as indices after.
    pub new_blocks: std::ops::Range<usize>,
}

/// What parsing a note has cost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParseStats {
    /// Parses of the whole text on the thread that edits the note.
    pub full_parses: usize,
    /// Edits that reparsed only the blocks they reached.
    pub in_place: usize,
    /// Background parses taken in.
    pub installed: usize,
    /// Background parses dropped because the text had changed since.
    pub dropped: usize,
}
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
    /// Code highlights worked out for the current text.
    pub highlight_cache: RefCell<crate::highlight::HighlightCache>,

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
    /// The parse of the current text, between calls that use it.
    ast: Option<Ast>,
    /// Full parses of a large note's text, made on another thread.
    background: Option<BackgroundParser>,
    /// What a background parse calls when it finishes.
    wake: Option<Wake>,
    /// Notes at least this many bytes long reparse an edit in place and
    /// parse in full in the background.
    large_note_bytes: usize,
    /// What parsing has cost this thread so far.
    stats: ParseStats,
    /// The top-level blocks the last in-place reparse replaced.
    last_block_edit: Option<BlockEdit>,
    /// Marked text an input method is composing at the caret. It never
    /// reaches the buffer; see [`crate::ime`].
    pub(crate) composition: Option<crate::ime::Composition>,
    pub debug: bool,
}

/// A node and one of its source lines.
pub type LinePrefixKey = (NodeKey, (Grapheme, Grapheme));

/// A node's cache key; see [`MdDoc::pack_node_key`].
pub type NodeKey = u128;

/// The maps the caches keep: keyed by a node, and hashed fast.
pub type NodeMap<K, V> = HashMap<K, V, rustc_hash::FxBuildHasher>;
/// The prefix length the node owns on that line, and whether it's complete.
pub type LinePrefixValue = (Graphemes, bool);

/// Values derived from the text, cached per parse. Wiped whenever the text
/// changes ([`TextCache::ensure_text_consistent`]).
#[derive(Default)]
pub struct TextCache {
    pub line_prefix_len: RefCell<NodeMap<LinePrefixKey, LinePrefixValue>>,
    pub node_range: RefCell<NodeMap<NodeKey, (Grapheme, Grapheme)>>,
    /// Depends on `reveal_seq` too: `hidden_by_fold_deps` holds the
    /// `reveal_seq` it was last populated at, or `None` if never.
    pub hidden_by_fold: RefCell<NodeMap<NodeKey, bool>>,
    pub hidden_by_fold_deps: Cell<Option<u64>>,
    /// The `text_seq` this cache was last consistent with.
    pub text_seq: Cell<u64>,
    /// Whether a list item is selected, and the selection that answer is
    /// for.
    pub any_item_selected: Cell<Option<((Grapheme, Grapheme), bool)>>,
}

impl TextCache {
    pub fn clear(&self) {
        self.line_prefix_len.borrow_mut().clear();
        self.node_range.borrow_mut().clear();
        self.hidden_by_fold.borrow_mut().clear();
        self.hidden_by_fold_deps.set(None);
        self.any_item_selected.set(None);
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
            highlight_cache: Default::default(),
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
            ast: None,
            background: None,
            wake: None,
            large_note_bytes: LARGE_NOTE_BYTES,
            stats: ParseStats::default(),
            last_block_edit: None,
            composition: None,
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
        let mut ops = Vec::new();
        let mut response = buffer::Response::default();
        let mut undo_redo = false;
        self.with_ast(|doc, root| {
            for event in events {
                undo_redo |= matches!(event, Event::Undo | Event::Redo);
                response |= doc.calc_operations(root, event, &mut ops, layout);
            }
        });
        self.buffer.queue(ops);
        response |= self.buffer.update();
        if response.text_updated {
            self.bump_text_seq();
        }
        let edited = response.text_updated || response.selection_user_moved;
        response |= self.unfold_after_edit(edited, undo_redo);
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
    /// [`Arena`] — comrak's AST is arena-allocated. [`Self::take_ast`]
    /// reuses one parse per text version instead.
    pub fn reparse<'a>(&mut self, arena: &'a Arena<'a>) -> &'a AstNode<'a> {
        self.cache.ensure_text_consistent(self.text_seq);

        let root = self.parse(arena);
        self.ensure_bounds(root);
        root
    }

    /// The parse of the current text, with its bounds built: the one the
    /// last call handed back with [`Self::put_ast`], or a new one. Hand it
    /// back when done so the next call reuses it.
    ///
    /// In a large note, an edit since the last call reparses only the
    /// blocks it reaches, and a full parse follows in the background; see
    /// [`crate::background`].
    pub fn take_ast(&mut self) -> Ast {
        self.cache.ensure_text_consistent(self.text_seq);
        self.install_background_parse();
        let ast = match self.ast.take() {
            Some(ast) if ast.text_seq() == self.text_seq => ast,
            Some(mut ast) if self.reparses_in_place(&ast) => {
                let Some(reparsed) = crate::lookahead::update_ast(
                    &mut ast,
                    self.buffer.current.text_with("\n"),
                    &Self::comrak_options(),
                    self.text_seq,
                ) else {
                    // The edit reaches `$$` math past the lookahead, which
                    // only a full parse reads.
                    return self.take_full_ast();
                };
                let from_bounds_seq = self.bounds_seq;
                ast.with_root(|root| self.update_bounds(root, &reparsed));
                self.last_block_edit = Some(BlockEdit {
                    from_bounds_seq,
                    to_bounds_seq: self.bounds_seq,
                    old_blocks: reparsed.blocks.start..reparsed.blocks.start + reparsed.replaced,
                    new_blocks: reparsed.blocks.clone(),
                });
                self.stats.in_place += 1;
                let wake = self.wake.clone();
                let background = self
                    .background
                    .take()
                    .unwrap_or_else(|| BackgroundParser::new(wake));
                background.request(self);
                self.background = Some(background);
                ast
            }
            _ => return self.take_full_ast(),
        };
        ast.with_root(|root| self.ensure_bounds(root));
        ast
    }

    /// [`Self::take_ast`], parsing the whole text unless the cached parse
    /// is of the current text.
    pub(crate) fn take_full_ast(&mut self) -> Ast {
        self.cache.ensure_text_consistent(self.text_seq);
        let ast = match self.ast.take() {
            Some(ast) if ast.text_seq() == self.text_seq => ast,
            _ => {
                self.stats.full_parses += 1;
                crate::lookahead::parse_ast(
                    self.buffer.current.text_with("\n"),
                    &Self::comrak_options(),
                    self.text_seq,
                )
            }
        };
        ast.with_root(|root| self.ensure_bounds(root));
        ast
    }

    /// Whether an edit to the text `ast` parsed can reparse in place: the
    /// note is large and the bounds are `ast`'s. Nothing folds, so no edit
    /// hides or shows blocks far from it, whatever fold markers a file
    /// holds as text.
    fn reparses_in_place(&self, ast: &Ast) -> bool {
        self.buffer.current.len_bytes() >= self.large_note_bytes
            && self.bounds.text_seq == ast.text_seq()
    }

    /// Takes in a finished background parse of the current text, or drops
    /// one of text that has changed since.
    fn install_background_parse(&mut self) {
        let Some(background) = &mut self.background else {
            return;
        };
        let Some(parsed) = background.take() else {
            return;
        };
        if parsed.ast.text_seq() != self.text_seq || parsed.bounds.text_seq != self.text_seq {
            background.dropped += 1;
            return;
        }
        background.installed += 1;
        // The full parse's arena holds no nodes an in-place reparse left
        // behind.
        self.ast = Some(parsed.ast);
        if self.bounds.same_text_bounds(&parsed.bounds) {
            // The in-place reparse built what the full parse did, so what
            // was worked out from it still holds.
            return;
        }
        let wrap_lines = std::mem::take(&mut self.bounds.wrap_lines);
        self.bounds = Bounds {
            wrap_lines,
            ..parsed.bounds
        };
        self.bounds_seq = self.next_seq();
        self.cache.clear();
    }

    /// Calls `wake` from the background thread whenever a full parse
    /// finishes, such as to ask the window for a frame that takes it in.
    pub fn on_background_parse(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.wake = Some(std::sync::Arc::new(wake));
        self.background = None;
    }

    /// Treats notes of at least `bytes` bytes as large: an edit reparses
    /// in place and a full parse follows in the background.
    pub fn set_large_note_bytes(&mut self, bytes: usize) {
        self.large_note_bytes = bytes;
    }

    /// Holds background parses back while `hold` is set, as a test does to
    /// see typing go on without them.
    pub fn hold_background_parses(&mut self, hold: bool) {
        let wake = self.wake.clone();
        self.background
            .get_or_insert_with(|| BackgroundParser::new(wake))
            .hold(hold);
    }

    /// Waits up to `timeout` for a background parse to finish, without
    /// taking it in. The next [`Self::take_ast`] does.
    pub fn wait_for_background_parse(&mut self, timeout: std::time::Duration) -> bool {
        self.background
            .as_mut()
            .is_some_and(|background| background.wait(timeout))
    }

    /// The top-level blocks the last in-place reparse replaced, for caches
    /// of per-block values that can keep the rest across it.
    pub fn last_block_edit(&self) -> Option<&BlockEdit> {
        self.last_block_edit.as_ref()
    }

    /// What parsing has cost so far.
    pub fn parse_stats(&self) -> ParseStats {
        ParseStats {
            installed: self.background.as_ref().map_or(0, |b| b.installed),
            dropped: self.background.as_ref().map_or(0, |b| b.dropped),
            ..self.stats
        }
    }

    /// Keeps `ast` for the next [`Self::take_ast`], unless the text
    /// changed since it was parsed.
    pub fn put_ast(&mut self, ast: Ast) {
        if ast.text_seq() == self.text_seq {
            self.ast = Some(ast);
        }
    }

    /// Runs `work` with this document and the parse of its current text.
    pub fn with_ast<R>(&mut self, work: impl for<'a> FnOnce(&mut Self, &'a AstNode<'a>) -> R) -> R {
        let ast = self.take_ast();
        let result = ast.with_root(|root| work(self, root));
        self.put_ast(ast);
        result
    }

    /// Rebuilds the bounds `root`, a parse of the current text, gives when
    /// the text changed since they were built, and the fold state when the
    /// reveal state changed.
    fn ensure_bounds<'a>(&mut self, root: &'a AstNode<'a>) {
        // Bounds depend only on text. Skip rebuild + bump bounds_seq
        // when text hasn't changed.
        if self.bounds.text_seq != self.text_seq {
            self.bounds.inline_paragraphs.clear();
            self.calc_source_lines();
            self.calc_fold_bounds(root);
            self.bounds.has_folds = !self.bounds.folds.is_empty();
            self.calc_atom_bounds(root);
            self.calc_inline_spans(root);
            self.bounds.frontmatter = self.frontmatter_range();
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
    }

    /// How the editor parses notes. The extensions on are the syntax
    /// today's editor draws. Lockbook's others (spoilers, emoji
    /// shortcodes, superscript, subscript, `__` as underline, code math)
    /// stay off, so their text shows as written.
    pub fn comrak_options() -> Options<'static> {
        let mut options = Options::default();
        options.parse.smart = true;
        options.parse.ignore_setext = false;
        // Callouts of any type are quotes the editor reads itself; comrak's
        // alerts know only GitHub's five types.
        options.extension.alerts = false;
        options.extension.autolink = true;
        options.extension.description_lists = false;
        options.extension.footnotes = false;
        // The editor reads frontmatter itself; see `crate::lookahead`.
        options.extension.front_matter_delimiter = None;
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
