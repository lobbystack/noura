//! Draws the `editor-core` Markdown editor with egui and translates its input.
//!
//! The editor comes from Lockbook's live-preview Markdown editor; see
//! `docs/provenance/lockbook-editor.md`. Text is shaped with glyphon and
//! painted through eframe's wgpu renderer with a paint callback, so an app
//! must call [`register_font_system`] and
//! [`register_render_callback_resources`] before showing an [`Editor`].

use std::mem;
use std::time::{Duration, Instant};

use comrak::Arena;
use comrak::nodes::AstNode;
use editor_core::doc::MdDoc;
use editor_core::offset_types::{Byte, Grapheme};
use egui::os::OperatingSystem;
use egui::{Context, EventFilter, Frame, Id, Margin, Pos2, Rect, Stroke, Ui, UiBuilder, Vec2};
use input::mutation::EventState;
use widget::block::LayoutCache;

pub use editor_core::bounds;
pub mod fold;
pub mod fonts;
pub mod input;
pub mod palette;
pub mod resolvers;
mod scroll_content;
pub mod seq;
pub mod show;
pub mod style;
mod theme;
pub(crate) mod widget;
pub mod widgets;

#[cfg(test)]
mod tests;

pub use input::Event;
pub use widgets::glyphon_render::{
    GlyphonRenderCallbackResources, GlyphonRendererCallback, TextBufferArea, install_font_system,
    register_font_system, register_render_callback_resources,
};

use crate::palette::ThemeExt as _;
use crate::resolvers::{EmbedResolver, LinkResolver};
use crate::scroll_content::DocRowId;
use crate::widgets::affine_scroll::{AffineScrollArea, Align, Offset, Reveal};

#[derive(Debug, Default)]
pub struct Response {
    // state changes
    pub text_updated: bool,
    pub selection_updated: bool,
    pub scroll_updated: bool,

    /// A link the user opened this frame, resolved through the
    /// [`LinkResolver`]. The editor never opens it itself.
    pub opened_link: Option<resolvers::OpenedLink>,
}

/// A span of the note's source and where the last frame drew it.
#[derive(Clone, Debug, PartialEq)]
pub struct PaintedSpan {
    pub source_range: (Grapheme, Grapheme),
    pub rect: Rect,
    /// Whether the span drew text, as opposed to spacing or an image.
    pub glyphs: bool,
}

/// A region painted this frame that a platform deciding touches itself
/// must not treat as text.
#[derive(Clone, Debug)]
pub enum TouchTarget {
    /// A tap applies this event.
    Tap(input::Event),
    /// The scrollbar: a drag on its thumb.
    Scrollbar,
    /// A popup egui handles itself.
    Popup,
}

pub struct MdRender {
    // context
    pub ctx: Context,
    pub layout: MdLayout,
    pub dark_mode: bool,
    pub ext: String,
    pub touch_mode: bool,
    /// Set only while painting the floating drag-reorder card, so chrome
    /// that shouldn't travel with it (the fold button) can opt out.
    pub painting_drag_float: bool,

    /// The text side: buffer, parse, bounds, reveal state and edit
    /// commands. The renderer derefs to it.
    pub doc: MdDoc,

    // render output
    pub fragments: Vec<widget::utils::wrap_layout::Fragment>,
    pub text_areas: Vec<TextBufferArea>,
    /// Strikethroughs and underlines painted on top of text
    pub deco_lines: Vec<widget::utils::wrap_layout::DecoLine>,
    pub render_events: Vec<input::Event>,
    /// Touch targets painted this frame.
    pub touch_targets: Vec<(Rect, TouchTarget)>,
    /// Per-frame geometry index of every list item, populated during the
    /// render DFS. Cleared each frame (mirrors `fragments`); read for
    /// pointer hit-testing and drop-gap math by drag-to-reorder.
    pub block_boxes: Vec<widget::block::drag::BlockBox>,
    /// Drag action emitted by a list marker this frame, drained by
    /// `MdEdit::show` to update / commit the in-progress block drag
    /// (mirrors how `render_events` carries buffer edits out of render).
    pub block_drag_action: Option<widget::block::drag::BlockDragAction>,
    /// Per-frame, keyed by `ui.scope_id().with(salt)`; populated by
    /// `interact_fragments`, consumed by handlers in each node type.
    pub interaction_responses: std::collections::HashMap<egui::Id, egui::Response>,
    /// Per-scope interact rects behind `interaction_responses`, whose merged
    /// rect is only a bounding box; read by `touch_consume_interaction`.
    pub interaction_rects: std::collections::HashMap<egui::Id, Vec<Rect>>,
    // render input
    pub in_progress_selection: Option<(Grapheme, Grapheme)>,
    /// Active list-item drag, mirrored from `MdEdit` so the render pass
    /// can dim the source and draw the drop indicator.
    pub in_progress_block_drag: Option<widget::block::drag::BlockDrag>,
    /// The platform text system's marked text (dictation in progress, an
    /// IME composition), painted so provisional text reads as such.
    pub platform_marked: Option<(Grapheme, Grapheme)>,
    pub interactive: bool, // enables fold buttons
    /// Password mode: render every source byte as `*` while the buffer keeps
    /// the real text. Only meaningful with `plaintext` — masking a parsed
    /// markdown tree isn't defined. Byte-length preserving, so
    /// cursor/selection mapping is unchanged.
    pub mask: bool,
    /// Single-line field mode: lay out on one unbounded line instead of
    /// wrapping at the shown rect's width. [`MdEdit::show`] pairs this with a
    /// cursor-following horizontal scroll; the caller's rect clips the
    /// overflow.
    pub single_line: bool,

    pub search_range: Option<(Grapheme, Grapheme)>, // drawn in accent color for completions

    // capabilities
    pub embeds: Box<dyn EmbedResolver>,

    // caches
    pub layout_cache: LayoutCache,

    // viewport
    pub width: f32,
    pub width_seq: u64,

    pub viewport_height: f32,

    // debug
    pub frame_times: [Instant; 100],
    pub frame_times_idx: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollTarget {
    Cursor,
    FindMatch,
    /// Read-only search-preview snippet (see [`MdRender::preview_match`]).
    Preview,
}

/// Editing primitive — an [`MdRender`] plus the interactive state needed to
/// mutate it. Self-contained so it can be used standalone without needing an
/// [`Editor`]'s surrounding widgets.
pub struct MdEdit {
    pub renderer: MdRender,

    pub event: EventState,

    /// Phone-sized layout (phone or iPad compact mode).
    pub phone_mode: bool,

    /// Transient drag selection — `Some` while a drag is in progress; the
    /// rendered cursor/selection falls back to the buffer's own selection
    /// when `None`.
    pub in_progress_selection: Option<(Grapheme, Grapheme)>,

    /// Offset of the selection handle being dragged (touch), so auto-scroll
    /// follows the moving handle rather than always the selection end. `None`
    /// outside a handle drag — scroll then falls back to the selection end.
    pub in_progress_handle: Option<Grapheme>,

    /// Active list-item drag-to-reorder — `Some` from grab until release.
    /// Mirrored onto the renderer each frame for the dim/indicator paint.
    pub in_progress_block_drag: Option<widget::block::drag::BlockDrag>,

    /// Touch long-press → reorder gesture state (touch only; desktop drags
    /// the marker handle directly). See [`MdEdit::detect_touch_reorder`].
    pub touch_reorder: widget::block::drag::TouchReorder,

    /// A committed reorder `(section_range, insert_offset)`, applied at the
    /// start of the next `handle_input` so the move lands pre-render. Set on
    /// drag release (which runs post-render).
    pub pending_block_move: Option<((Grapheme, Grapheme), Grapheme)>,

    /// Frame-scoped single-target scroll intent, consumed at the end of the
    /// scroll area callback.
    pub pending_scroll: Option<ScrollTarget>,

    /// Horizontal scroll (px) of a [`MdRender::single_line`] field — how far
    /// the one-line layout is shifted left so the cursor stays inside the
    /// shown rect. Maintained by [`MdEdit::show`]; 0 outside single-line mode.
    pub single_line_scroll: f32,

    /// Vertical scroll (px) of a multi-line field whose content outgrew its
    /// rect. Maintained by [`MdEdit::show`]; 0 while the content fits.
    pub overflow_scroll: f32,

    /// Buffer (seq, selection) and rect at the last cursor-follow of
    /// `overflow_scroll` — the follow runs when these change, leaving a
    /// wheel scroll free to move the cursor out of view.
    overflow_follow: (usize, (Grapheme, Grapheme), Rect),

    /// Momentum from the last scroll-area frame.
    pub scroll_area_velocity: Vec2,

    /// Owns the per-row scroll state (offset, momentum) and renders
    /// the scrollbar.
    pub scroll_area: AffineScrollArea<DocRowId>,

    cursor_last_interact: Option<f64>,
}

impl MdEdit {
    /// Minimal editor for standalone use. Wraps `MdRender::empty` (no-op
    /// resolvers) and sets `readonly = false` on the renderer so the editor
    /// can mutate.
    pub fn empty(ctx: Context) -> Self {
        let mut renderer = MdRender::empty(ctx);
        renderer.readonly = false;
        Self::new(renderer, false, Id::NULL)
    }

    fn new(renderer: MdRender, phone_mode: bool, id_salt: Id) -> Self {
        Self {
            renderer,
            event: Default::default(),
            phone_mode,
            in_progress_selection: None,
            in_progress_handle: None,
            in_progress_block_drag: None,
            touch_reorder: Default::default(),
            pending_block_move: None,
            pending_scroll: None,
            single_line_scroll: 0.0,
            overflow_scroll: 0.0,
            overflow_follow: (0, Default::default(), Rect::ZERO),
            scroll_area_velocity: Default::default(),
            scroll_area: AffineScrollArea::new(id_salt),
            cursor_last_interact: None,
        }
    }
}

/// How an [`Editor`] treats its note.
pub struct MdConfig {
    /// Refuses edits; the note can still be selected and copied.
    pub readonly: bool,
    /// The file extension. Anything other than `md` shows as plain text.
    pub ext: String,
}

impl Default for MdConfig {
    fn default() -> Self {
        Self {
            readonly: false,
            ext: "md".into(),
        }
    }
}

/// The full editor widget: an [`MdEdit`] in a scroll area, with the
/// bookkeeping an app needs to load and save the note.
pub struct Editor {
    pub edit: MdEdit,

    // document identity
    pub id_salt: Id,
    pub initialized: bool,

    // change detection
    pub unprocessed_scroll: Option<Instant>,
    prev_dimensions: Option<Vec2>,
    embeds_seq: u64,

    // outputs from drawing a frame need an additional frame to process before reporting
    next_resp: Response,
}

static PRINT: bool = false;

pub struct MdLayout {
    pub margin: f32,
    pub max_width: f32,
    pub inline_padding: f32,
    pub annotation_font_size: f32,
    pub row_height: f32,
    pub block_padding: f32,
    pub indent: f32,
    pub bullet_radius: f32,
    pub row_spacing: f32,
    pub block_spacing: f32,
    pub completion_font_size: f32,
    pub completion_line_height: f32,
    pub completion_row_height: f32,
    pub completion_corner_radius: u8,
}

impl MdLayout {
    pub fn mobile() -> Self {
        Self {
            margin: 32.0,
            max_width: 1000.0,
            inline_padding: 3.0,
            annotation_font_size: 12.0,
            row_height: 16.0,
            block_padding: 10.0,
            indent: 26.0,
            bullet_radius: 2.0,
            row_spacing: 6.0,
            block_spacing: 14.0,
            completion_font_size: 14.0,
            completion_line_height: 16.0,
            completion_row_height: 24.0,
            completion_corner_radius: 4,
        }
    }

    pub fn desktop() -> Self {
        Self {
            margin: 45.0,
            max_width: 1000.0,
            inline_padding: 3.0,
            annotation_font_size: 12.0,
            row_height: 16.0,
            block_padding: 10.0,
            indent: 26.0,
            bullet_radius: 2.0,
            row_spacing: 6.0,
            block_spacing: 12.0,
            completion_font_size: 14.0,
            completion_line_height: 16.0,
            completion_row_height: 24.0,
            completion_corner_radius: 4,
        }
    }
}

impl MdRender {
    /// Renderer for `md` with no resolvers, the `md` extension, and the
    /// given read-only setting. Touch-mode and layout are derived from `ctx`.
    pub fn new(ctx: Context, md: &str, ext: &str, readonly: bool) -> Self {
        let touch_mode = matches!(ctx.os(), OperatingSystem::Android | OperatingSystem::IOS);
        let layout = if touch_mode {
            MdLayout::mobile()
        } else {
            MdLayout::desktop()
        };
        let dark_mode = ctx.global_style().visuals.dark_mode;
        let mut doc = MdDoc::with_seq(md, crate::seq::ws_seq(&ctx));
        doc.readonly = readonly;
        doc.plaintext = !ext.eq_ignore_ascii_case("md");
        Self {
            ctx,
            layout,
            dark_mode,
            ext: ext.into(),
            touch_mode,
            painting_drag_float: false,
            doc,
            fragments: Vec::new(),
            block_boxes: Vec::new(),
            block_drag_action: None,
            text_areas: Default::default(),
            deco_lines: Default::default(),
            render_events: Vec::new(),
            touch_targets: Default::default(),
            interaction_responses: Default::default(),
            interaction_rects: Default::default(),
            in_progress_selection: None,
            in_progress_block_drag: None,
            platform_marked: None,
            interactive: false,
            mask: false,
            single_line: false,
            search_range: None,
            embeds: Box::new(()),
            layout_cache: Default::default(),
            width: Default::default(),
            width_seq: 0,
            viewport_height: Default::default(),
            frame_times: [Instant::now(); 100],
            frame_times_idx: 0,
        }
    }

    /// Minimal renderer with no resolvers and an empty, read-only buffer.
    pub fn empty(ctx: Context) -> Self {
        Self::new(ctx, "", "md", true)
    }

    #[cfg(test)]
    pub(crate) fn test(md: &str) -> Self {
        let mut renderer = Self::new(test_egui_ctx(), md, "md", true);
        renderer.layout = MdLayout::desktop();
        renderer.touch_mode = false;
        renderer
    }

    /// Setter for `width` that bumps `width_seq` only when the value
    /// actually changes — frame-scoped writers reassign every frame even
    /// when the layout hasn't moved, and an unconditional bump would
    /// invalidate every cached height.
    pub fn set_width(&mut self, width: f32) {
        if self.width.to_bits() != width.to_bits() {
            self.width = width;
            self.width_seq = self
                .ws_seq
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Test setter for the viewport height — used by property tests
    /// that want to drive layout at a known viewport size without
    /// running a full egui frame.
    pub fn set_viewport_height(&mut self, viewport_height: f32) {
        // Images fit the viewport, so its height is a layout dimension as
        // the width is: a change invalidates the cached heights.
        if self.viewport_height.to_bits() != viewport_height.to_bits() {
            self.viewport_height = viewport_height;
            self.width_seq = self
                .ws_seq
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Re-parse the buffer into a fresh AST and rebuild all text-derived
    /// bounds. Wipes the layout cache first if the text changed. The
    /// caller owns the [`Arena`] — comrak's AST is arena-allocated.
    pub fn reparse<'a>(&mut self, arena: &'a Arena<'a>) -> &'a AstNode<'a> {
        // Width / link / embed / reveal invalidation flows through
        // dep-stamp mismatch in the height + spacing caches. Text changes
        // can affect any entry, so the layout cache wipes wholesale here
        // before any reads.
        self.layout_cache.ensure_text_consistent(self.doc.text_seq);
        self.doc.reparse(arena)
    }

    /// The fragments painted last frame, as `editor-core`'s layout.
    pub fn text_layout(&self) -> input::layout::FragmentLayout<'_> {
        input::layout::FragmentLayout {
            fragments: widget::utils::wrap_layout::Fragments(&self.fragments),
            last: self.doc.buffer.current.segs.last_cursor_position(),
        }
    }
}

impl std::ops::Deref for MdRender {
    type Target = MdDoc;

    fn deref(&self) -> &MdDoc {
        &self.doc
    }
}

impl std::ops::DerefMut for MdRender {
    fn deref_mut(&mut self) -> &mut MdDoc {
        &mut self.doc
    }
}

impl Editor {
    pub fn new(ctx: &Context, md: &str, config: MdConfig) -> Self {
        let MdConfig { readonly, ext } = config;
        let mut renderer = MdRender::new(ctx.clone(), md, &ext, readonly);
        renderer.interactive = true;
        let phone_mode = false;
        let id_salt = Id::unique("noura_editor");

        Self {
            edit: MdEdit::new(renderer, phone_mode, id_salt),
            id_salt,
            initialized: Default::default(),
            embeds_seq: 0,
            unprocessed_scroll: Default::default(),
            prev_dimensions: None,
            next_resp: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn test(md: &str) -> Self {
        Self::new(
            &test_egui_ctx(),
            md,
            MdConfig {
                readonly: false,
                ext: "md".into(),
            },
        )
    }

    /// Replaces the link resolver, which answers link and wikilink lookups.
    pub fn set_link_resolver(&mut self, resolver: Box<dyn LinkResolver>) {
        self.edit.renderer.doc.link_resolver = resolver;
    }

    /// Replaces the embed resolver, which loads and paints images.
    pub fn set_embed_resolver(&mut self, resolver: Box<dyn EmbedResolver>) {
        self.edit.renderer.embeds = resolver;
    }

    pub fn id(&self) -> Id {
        Id::unique("md_editor").with(self.id_salt)
    }

    /// The note's current text.
    pub fn text(&self) -> &str {
        self.edit.renderer.buffer.current.text()
    }

    /// The selection as `(anchor, caret)`.
    pub fn selection(&self) -> (Grapheme, Grapheme) {
        self.edit.renderer.buffer.current.selection
    }

    /// What the last frame drew, one span of source at a time, in source
    /// order. Hidden syntax draws nothing, so it has no glyph span.
    pub fn painted_spans(&self) -> Vec<PaintedSpan> {
        use widget::utils::wrap_layout::FragmentContent;
        self.edit
            .renderer
            .fragments
            .iter()
            .map(|fragment| PaintedSpan {
                source_range: fragment.source_range,
                rect: fragment.rect,
                glyphs: matches!(fragment.content, FragmentContent::Glyphs { .. }),
            })
            .collect()
    }

    /// Queues an event for the next frame.
    pub fn push_event(&mut self, event: Event) {
        self.edit.event.internal_events.push(event);
    }

    fn apply_pending_buffer(&mut self) {
        let resp = self.edit.renderer.buffer.update();
        if resp.text_updated {
            self.edit.renderer.bump_text_seq();
        }
    }

    /// Merges new file contents into the live buffer: edits made outside
    /// the editor since the last load or save merge with the edits made in
    /// it.
    pub fn apply_disk_version(&mut self, text: String) {
        self.edit.renderer.buffer.reload(text);
        self.apply_pending_buffer();
    }

    /// Applies queued buffer operations, then returns the buffer version and
    /// text to save. Pass both to [`Self::record_disk_save`] once the save
    /// succeeds.
    pub fn flush_for_save(&mut self) -> (usize, String) {
        self.apply_pending_buffer();
        let current = &self.edit.renderer.buffer.current;
        (current.seq, current.text().to_string())
    }

    /// Records that the file now holds `content`, saved at buffer version
    /// `seq`. Later outside edits merge against it.
    pub fn record_disk_save(&mut self, seq: usize, content: String) {
        self.edit.renderer.buffer.saved(seq, content);
    }

    pub fn focus(&self, ctx: &Context) {
        ctx.memory_mut(|m| {
            m.request_focus(self.id());
        });
    }

    pub fn focus_lock(&mut self, ctx: &Context) {
        ctx.memory_mut(|m| {
            m.set_focus_lock_filter(
                self.id(),
                EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            );
        });
    }

    pub fn focused(&self, ctx: &Context) -> bool {
        ctx.memory(|m| m.has_focus(self.id()))
    }

    pub fn surrender_focus(&self, ctx: &Context) {
        ctx.memory_mut(|m| {
            m.surrender_focus(self.id());
        });
    }

    pub fn plaintext_mode(&self) -> bool {
        self.edit.renderer.plaintext
    }

    pub fn show(&mut self, ui: &mut Ui) -> Response {
        let mut resp: Response = mem::take(&mut self.next_resp);

        let height = ui.available_size().y.round();
        let width = ui
            .max_rect()
            .width()
            .min(self.edit.renderer.layout.max_width)
            .round();
        let dimensions = Vec2::new(width, height);
        let (height_updated, width_updated) = match self.prev_dimensions {
            Some(prev) => (prev.y != dimensions.y, prev.x != dimensions.x),
            None => (true, true),
        };
        self.prev_dimensions = Some(dimensions);

        let dark_mode = ui.style().visuals.dark_mode;
        if dark_mode != self.edit.renderer.dark_mode {
            self.edit.renderer.dark_mode = dark_mode;
        }

        let start = Instant::now();

        if height_updated {
            self.edit.renderer.layout_cache.clear();
        }

        let embeds_seq = self.edit.renderer.embeds.seq();
        let embeds_updated = embeds_seq != self.embeds_seq;
        self.embeds_seq = embeds_seq;

        // --- input phase ------------------------------------------------------
        let prior_selection = self.edit.renderer.buffer.current.selection;
        let prior_entered_atom = self.edit.renderer.entered_atom;
        let buf_resp = self.handle_input(ui);

        if !self.initialized || buf_resp.text_updated {
            resp.text_updated = true;
            ui.ctx().request_repaint();
        }
        resp.selection_updated = prior_selection
            != self
                .edit
                .in_progress_selection
                .unwrap_or(self.edit.renderer.buffer.current.selection);
        // Loads completing (embeds) and atom enter/exit reflow layout, moving
        // the selection's rects without moving the selection.
        resp.scroll_updated |=
            embeds_updated || prior_entered_atom != self.edit.renderer.entered_atom;

        let all_selected = self.edit.renderer.buffer.current.selection
            == (0.into(), self.edit.renderer.last_cursor_position());
        if self.initialized
            && buf_resp.selection_user_moved
            && !all_selected
            && self.edit.in_progress_block_drag.is_none()
        {
            self.edit.pending_scroll = Some(ScrollTarget::Cursor);
        }

        let ast_elapsed = start.elapsed();
        let start = Instant::now();

        // --- draw phase (back to front) ---------------------------------------
        // Re-parse for render. handle_input parsed its own; that arena has
        // been dropped. The parse is assumed cheap (~1 ms).
        let arena = Arena::new();
        let root = {
            let _parse = tracing::trace_span!("MarkdownEditor::parse").entered();
            let options = MdDoc::comrak_options();
            let text_with_newline = self.edit.renderer.buffer.current.text().to_string() + "\n";
            comrak::parse_document(&arena, &text_with_newline, &options)
        };

        if PRINT {
            println!("{}", "=".repeat(80));
            print_ast(root);
        }

        ui.painter().rect_filled(
            ui.max_rect(),
            0.,
            self.edit.renderer.ctx.get_lb_theme().neutral_bg(),
        );
        self.edit.renderer.apply_theme(ui);
        ui.spacing_mut().item_spacing.x = 0.;

        let prev_offset = self.edit.scroll_area.stored_offset();

        ui.vertical(|ui| {
            // galleys / wrap_lines are cleared and repopulated inside
            // MdEdit::show — don't clear here or input handling (which reads
            // last-frame galleys) sees nothing.
            self.edit.renderer.touch_targets.clear();
            self.show_scrollable_editor(ui, root);
        });

        let new_offset = self.edit.scroll_area.stored_offset();
        self.next_resp.scroll_updated = new_offset != prev_offset;
        self.edit.scroll_area_velocity = self.edit.scroll_area.velocity();

        // Selection can change both in `handle_input` and later during
        // `MdEdit::show` pointer handling. Compute this after render so the
        // response sees both paths.
        resp.selection_updated |= prior_selection
            != self
                .edit
                .in_progress_selection
                .unwrap_or(self.edit.renderer.buffer.current.selection);

        let render_elapsed = start.elapsed();

        if self.edit.renderer.debug {
            self.edit.renderer.show_debug_fps(ui);
        }

        if PRINT {
            println!("{}", "-".repeat(80));
            println!("document: {:?}", self.edit.renderer.buffer.current.text());
            println!("{}", "-".repeat(80));
            println!("ast: {ast_elapsed:?}");
            println!("render: {render_elapsed:?}");
        }

        // post-frame bookkeeping
        if embeds_updated || height_updated || width_updated {
            if embeds_updated {
                self.unprocessed_scroll = Some(Instant::now());
            }
            ui.ctx().request_repaint();
        } else if resp.selection_updated {
            ui.ctx().request_repaint();
        }
        if self.next_resp.scroll_updated {
            self.unprocessed_scroll = Some(Instant::now());
            ui.ctx().request_repaint();
        }
        self.edit
            .event
            .internal_events
            .append(&mut self.edit.renderer.render_events);
        self.edit
            .event
            .internal_events
            .append(&mut self.edit.renderer.doc.deferred_events);

        if !self.edit.event.internal_events.is_empty() {
            ui.ctx().request_repaint();
        }

        if let Some(unprocessed_scroll) = self.unprocessed_scroll
            && unprocessed_scroll.elapsed() > Duration::from_millis(100)
        {
            self.unprocessed_scroll = None;
        }

        resp.opened_link = self.edit.renderer.doc.opened_links.pop();

        // focus editor when first shown or when nothing else has focus
        if !self.initialized || ui.memory(|m| m.focused().is_none()) {
            self.focus(ui.ctx());
        }
        if self.focused(ui.ctx()) {
            self.focus_lock(ui.ctx());
        }

        self.initialized = true;

        resp
    }

    pub fn will_consume_touch(&self, pos: Pos2) -> bool {
        self.touches_interactive_element(pos)
            || self.edit.scroll_area_velocity.abs().max_elem() > 0. // velocity zeroed at touch down
            || self.edit.scroll_area.momentum_cancel_press() // platform can check at touch up
            || self.reorder_armed() // a committed reorder isn't a tap
    }

    /// Whether a reorder is committed and dragging (`Armed`, not the
    /// still-`Pending` hold). Gates keyboard/cursor suppression — a `Pending`
    /// hold may still resolve to a tap, which must focus normally.
    pub fn reorder_armed(&self) -> bool {
        use widget::block::drag::TouchReorder;
        matches!(self.edit.touch_reorder, TouchReorder::Armed { .. })
    }

    /// Whether `pos` is over an interactive element (checkbox, fold button,
    /// link, spoiler, scrollbar, popup).
    pub fn touches_interactive_element(&self, pos: Pos2) -> bool {
        self.edit
            .renderer
            .touch_targets
            .iter()
            .any(|(rect, _)| rect.contains(pos))
    }

    #[tracing::instrument(name = "MarkdownEditor::input", level = "trace", skip_all)]
    fn handle_input(&mut self, ui: &Ui) -> editor_core::buffer::Response {
        self.edit.handle_input(ui.ctx(), self.id())
    }

    #[tracing::instrument(name = "MarkdownEditor::paint", level = "trace", skip_all)]
    fn show_scrollable_editor<'a>(&mut self, ui: &mut Ui, _root: &'a AstNode<'a>) {
        let scroll_id = self.id();

        // Captured by the Frame::canvas closure so post-render code can
        // position galleys against the canvas origin without re-deriving it.
        let mut captured_canvas_rect: Option<Rect> = None;

        Frame::canvas(ui.style())
            .inner_margin(Margin::ZERO)
            .stroke(Stroke::NONE)
            // Frame's own fill sizes to the content min_rect, which the
            // off-screen neighbor rows (and their interactive allocations)
            // inflate above the viewport — painting the background over
            // sibling chrome during scroll. Paint it ourselves, clipped to
            // the viewport, below.
            .fill(egui::Color32::TRANSPARENT)
            .show(ui, |ui| {
                // Claim full available width with 0 vertical space so the
                // Frame stretches horizontally regardless of how narrow the
                // visible content turns out to be.
                let avail_w = ui.available_width();
                ui.allocate_space(Vec2::new(avail_w, 0.0));

                let canvas_rect = ui.max_rect();
                captured_canvas_rect = Some(canvas_rect);
                let canvas_width = canvas_rect.width();
                let layout_margin = self.edit.renderer.layout.margin;

                // Content column width caps at `max_width`; the affine
                // widget spans the full canvas so its scrollbar lands
                // at the canvas right edge.
                let max_width = self.edit.renderer.layout.max_width;
                let col_width = (canvas_width - 2.0 * layout_margin).min(max_width).max(0.0);
                self.edit.renderer.set_width(col_width);

                let arena = Arena::new();
                let root = self.edit.renderer.reparse(&arena);

                // affine render: widget body spans the full canvas
                // (scrollbar at canvas right); content centers within
                // that body via `content_x`.
                let touch_scroll = self.edit.renderer.touch_mode;
                let content_x = canvas_rect.min.x
                    + ((canvas_rect.width() - self.edit.renderer.width) / 2.0).max(0.0);
                let pre = ui
                    .scope_builder(UiBuilder::new().max_rect(canvas_rect), |ui| {
                        ui.set_clip_rect(canvas_rect);
                        // Editor background, bounded to the viewport (see the
                        // `fill(TRANSPARENT)` note on the Frame). First shape in
                        // the scope, so it sits behind all content.
                        ui.painter().rect_filled(
                            canvas_rect,
                            0.0,
                            self.edit.renderer.ctx.get_lb_theme().neutral_bg(),
                        );
                        self.edit.scroll_area.touch_scroll = touch_scroll;
                        // An armed touch reorder owns the gesture — don't
                        // scroll the body under the dragged item.
                        self.edit.scroll_area.suppress_body_drag = matches!(
                            self.edit.touch_reorder,
                            widget::block::drag::TouchReorder::Armed { .. }
                        );

                        // Body alloc → pre_render → scrollbar/content
                        // ordering puts pre_render's click rect above
                        // the body's drag (so taps reach it) but below
                        // the scrollbar (so taps on the bar don't).
                        let begun = self.edit.scroll_area.begin(ui);

                        let pre = self.edit.pre_render(ui, canvas_rect, scroll_id, root);

                        self.edit.renderer.fragments.clear();
                        self.edit.renderer.block_boxes.clear();
                        self.edit.renderer.in_progress_block_drag =
                            self.edit.in_progress_block_drag;
                        self.edit.renderer.bounds.wrap_lines.clear();
                        self.edit.renderer.text_areas.clear();
                        self.edit.renderer.deco_lines.clear();

                        // Phase 1: drive scroll math + scrollbar, and
                        // pick up to one viewport's worth of rows above
                        // and below the visible window so navigation
                        // (advance_by_line) finds fragments on off-
                        // screen rows. Without this buffer, holding
                        // up/down arrow at a viewport edge sticks
                        // because `fragments` only contains visible-row
                        // entries. The immutable `DocScrollContent`
                        // borrow is released here before phase 2's
                        // mutable renderer paint.
                        let (visible, neighbors, far, scrollbar_grab) = {
                            use crate::widgets::affine_scroll::{Rows as _, VisibleRow};

                            let content = scroll_content::DocScrollContent::for_frame(
                                &self.edit.renderer,
                                root,
                                canvas_rect.height(),
                            );
                            let resp = self.edit.scroll_area.finish(ui, begun, &content);

                            let viewport_height = canvas_rect.height();
                            let mut neighbors: Vec<VisibleRow<scroll_content::DocRowId>> =
                                Vec::new();
                            if let Some(first) = resp.visible.first() {
                                let mut id = first.id;
                                let mut top = first.top;
                                let mut walked = 0.0f32;
                                while walked < viewport_height {
                                    let Some(prev_id) = content.prev(&id) else {
                                        break;
                                    };
                                    let height = content.precise(&prev_id);
                                    top -= height;
                                    neighbors.push(VisibleRow {
                                        id: prev_id,
                                        top,
                                        height,
                                    });
                                    walked += height;
                                    id = prev_id;
                                }
                            }
                            if let Some(last) = resp.visible.last() {
                                let mut id = last.id;
                                let mut top = last.top + last.height;
                                let mut walked = 0.0f32;
                                while walked < viewport_height {
                                    let Some(next_id) = content.next(&id) else {
                                        break;
                                    };
                                    let height = content.precise(&next_id);
                                    neighbors.push(VisibleRow {
                                        id: next_id,
                                        top,
                                        height,
                                    });
                                    top += height;
                                    walked += height;
                                    id = next_id;
                                }
                            }

                            // Rows holding the selection's ends beyond
                            // that band, at the scroll model's estimate
                            // of their distance, kept outside the band.
                            // Their layout is exact within the row, so a
                            // platform text system gets real geometry
                            // for a far end without the rows between.
                            let band_top = resp
                                .visible
                                .iter()
                                .chain(neighbors.iter())
                                .map(|r| r.top)
                                .fold(f32::INFINITY, f32::min);
                            let band_bottom = resp
                                .visible
                                .iter()
                                .chain(neighbors.iter())
                                .map(|r| r.top + r.height)
                                .fold(f32::NEG_INFINITY, f32::max);
                            let index = |id: &scroll_content::DocRowId| match id {
                                scroll_content::DocRowId::Block(i)
                                | scroll_content::DocRowId::Line(i) => Some(*i),
                                _ => None,
                            };
                            let band_first = resp
                                .visible
                                .iter()
                                .chain(neighbors.iter())
                                .filter_map(|r| index(&r.id))
                                .min();
                            let selection = self
                                .edit
                                .in_progress_selection
                                .unwrap_or(self.edit.renderer.buffer.current.selection);
                            let mut far: Vec<VisibleRow<scroll_content::DocRowId>> = Vec::new();
                            for end in [selection.0, selection.1] {
                                let Some(row) = content.find_text_row(end) else {
                                    continue;
                                };
                                let (Some(i), Some(first)) = (index(&row), band_first) else {
                                    continue;
                                };
                                let laid_out = resp
                                    .visible
                                    .iter()
                                    .chain(neighbors.iter())
                                    .chain(far.iter())
                                    .any(|r| r.id == row);
                                if laid_out {
                                    continue;
                                }
                                let height = content.precise(&row);
                                if height <= 0.0 {
                                    continue;
                                }
                                let estimate = self
                                    .edit
                                    .scroll_area
                                    .state
                                    .viewport_y_of(&content, &Offset::at_top_of(row));
                                let top = if i < first {
                                    estimate.min(band_top - height)
                                } else {
                                    estimate.max(band_bottom)
                                };
                                far.push(VisibleRow {
                                    id: row,
                                    top,
                                    height,
                                });
                            }
                            (resp.visible, neighbors, far, resp.scrollbar_grab)
                        };
                        // Register the scrollbar's grab area so taps on it
                        // don't fall through to cursor placement.
                        self.edit
                            .renderer
                            .touch_targets
                            .extend(scrollbar_grab.map(|rect| (rect, TouchTarget::Scrollbar)));

                        // Phase 2: paint each visible row with a mutable
                        // renderer borrow. Block list re-collected
                        // because the immutable borrow that held
                        // `DocScrollContent`'s copy was just dropped.
                        // Neighbor rows paint off-screen — clipped by
                        // the canvas — only to register fragments and
                        // wrap-line bounds for navigation.
                        let blocks: Vec<_> = root.children().collect();
                        for vrow in visible.iter().chain(neighbors.iter()).chain(far.iter()) {
                            let top_left =
                                Pos2::new(canvas_rect.min.x, canvas_rect.min.y + vrow.top);
                            let from = self.edit.renderer.fragments.len();
                            scroll_content::paint_row(
                                ui,
                                &mut self.edit.renderer,
                                root,
                                &blocks,
                                &vrow.id,
                                top_left,
                                content_x,
                            );
                            if far.contains(vrow) {
                                for f in &mut self.edit.renderer.fragments[from..] {
                                    f.far = true;
                                }
                            }
                        }

                        // read-only search-preview snippet highlight
                        if let Some(range) = self.edit.renderer.preview_match {
                            let theme = self.edit.renderer.ctx.get_lb_theme();
                            let color = theme.fg().yellow.lerp_to_gamma(theme.neutral_bg(), 0.5);
                            self.edit.show_range(ui, range, color);
                        }

                        if let Some(range) = self.edit.renderer.platform_marked {
                            let theme = self.edit.renderer.ctx.get_lb_theme();
                            let color = theme.accent().lerp_to_gamma(theme.neutral_bg(), 0.75);
                            self.edit.show_range(ui, range, color);
                        }

                        // List-item drag-to-reorder: consume the handle
                        // action and commit on release (visuals are drawn
                        // after post_render so they composite on top).
                        self.edit.detect_touch_reorder(ui, false);
                        self.edit.handle_block_drag(ui);
                        pre
                    })
                    .inner;
                self.edit.renderer.fragments.sort_by_key(|f| f.source_range);
                // Neighbor rows above the visible window paint after
                // visible rows, so wrap-lines lose their natural source
                // order. `Bound::Line` lookup is a linear `range_before`
                // / `range_after` walk that assumes sorted ranges.
                self.edit.renderer.bounds.wrap_lines.sort_by_key(|r| r.0);

                self.edit.post_render(ui, canvas_rect, scroll_id, pre);
                self.edit.draw_dragged_overlay(ui, root);
                ui.advance_cursor_after_rect(canvas_rect);
            });

        if matches!(self.edit.pending_scroll, Some(ScrollTarget::Preview)) {
            self.edit.pending_scroll = None;
            if let Some(canvas_rect) = captured_canvas_rect {
                self.scroll_to_preview(canvas_rect);
            }
        }
    }

    pub fn open_navigate(&mut self, byte_range: std::ops::Range<usize>) {
        let segs = &self.edit.renderer.buffer.current.segs;
        let last = segs.last_cursor_position();
        let start = segs
            .byte_to_char_floor(Byte(byte_range.start))
            .clamp(0.into(), last);
        let end = segs
            .byte_to_char_ceil(Byte(byte_range.end))
            .clamp(0.into(), last);

        self.edit
            .renderer
            .buffer
            .queue(vec![editor_core::operation_types::Operation::Select((
                start, end,
            ))]);
        self.edit.renderer.buffer.update();
        self.edit.renderer.reveal_seq = self
            .edit
            .renderer
            .ws_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.edit.pending_scroll = Some(ScrollTarget::Cursor);
    }

    pub fn preview_navigate(&mut self, byte_range: Option<std::ops::Range<usize>>) {
        let segs = &self.edit.renderer.buffer.current.segs;
        let new_match = byte_range.map(|r| {
            (
                segs.byte_to_char_floor(Byte(r.start)),
                segs.byte_to_char_ceil(Byte(r.end)),
            )
        });

        if self.edit.renderer.preview_match != new_match {
            self.edit.renderer.preview_match = new_match;
            // reveal state feeds cached block heights; invalidate like find does
            self.edit.renderer.reveal_seq = self
                .edit
                .renderer
                .ws_seq
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if new_match.is_some() {
                self.edit.pending_scroll = Some(ScrollTarget::Preview);
            }
        }
    }

    fn scroll_to_preview(&mut self, canvas_rect: Rect) {
        let Some(match_range) = self.edit.renderer.preview_match else {
            return;
        };

        let arena = Arena::new();
        let root = self.edit.renderer.reparse(&arena);
        let content = scroll_content::DocScrollContent::for_frame(
            &self.edit.renderer,
            root,
            canvas_rect.height(),
        );

        let Some(target_rect) = build_target_reveal(
            &self.edit.renderer,
            &content,
            &self.edit.scroll_area.state,
            match_range,
            canvas_rect,
            self.edit.renderer.layout.row_spacing / 2.0,
        ) else {
            return;
        };
        self.edit
            .scroll_area
            .reveal(&content, target_rect, Align::Center);
    }
}

/// Build a [`Reveal`] spanning a Grapheme range — works for a single
/// cursor (`(g, g)`), a multi-line selection, or a find match that
/// crosses a wrap boundary. Each endpoint independently prefers a
/// fragment-derived intra-row position so `Align::Center` lands the
/// span at viewport center and `Align::Nearest` knows precisely which
/// screen lines need to be in view; falls back to a row-anchor when
/// no fragment exists (off-viewport block not yet shaped). The range
/// is reordered to `(min, max)` so callers can pass raw `(anchor,
/// cursor)` selections.
pub(crate) fn build_target_reveal(
    renderer: &MdRender,
    content: &scroll_content::DocScrollContent<'_, '_>,
    state: &crate::widgets::affine_scroll::ScrollArea<DocRowId>,
    range: (Grapheme, Grapheme),
    canvas_rect: Rect,
    pad: f32,
) -> Option<Reveal<DocRowId>> {
    let (start, end) = if range.0 <= range.1 {
        range
    } else {
        (range.1, range.0)
    };
    let top = endpoint_offset(
        renderer,
        content,
        state,
        start,
        canvas_rect,
        EndpointSide::Top,
        pad,
    )?;
    let bottom = endpoint_offset(
        renderer,
        content,
        state,
        end,
        canvas_rect,
        EndpointSide::Bottom,
        pad,
    )?;
    Some(Reveal { top, bottom })
}

#[derive(Clone, Copy)]
enum EndpointSide {
    Top,
    Bottom,
}

fn endpoint_offset(
    renderer: &MdRender,
    content: &scroll_content::DocScrollContent<'_, '_>,
    state: &crate::widgets::affine_scroll::ScrollArea<DocRowId>,
    target: Grapheme,
    canvas_rect: Rect,
    side: EndpointSide,
    pad: f32,
) -> Option<Offset<DocRowId>> {
    use crate::widgets::affine_scroll::Rows as _;
    if let Some(frag) = renderer.fragment_at_offset(target).filter(|f| !f.far) {
        let y_range = frag.rect.y_range().expand(pad);
        let y = match side {
            EndpointSide::Top => y_range.min,
            EndpointSide::Bottom => y_range.max,
        };
        return state.offset_at_viewport_y(content, y - canvas_rect.min.y);
    }
    let row = content.find_text_row(target)?;
    Some(match side {
        EndpointSide::Top => Offset::at_top_of(row),
        EndpointSide::Bottom => Offset {
            anchor: row,
            intra_precise: content.precise(&row),
        },
    })
}

pub fn print_ast<'a>(root: &'a AstNode<'a>) {
    print_recursive(root, "");
}

fn print_recursive<'a>(node: &'a AstNode<'a>, indent: &str) {
    let last_child = node.next_sibling().is_none();
    let data = node.data.borrow();
    let kind = if data.value.block() { "□" } else { "☰" };
    let children = node.children().count();
    let child_count = if children > 0 {
        format!(" +{children} ")
    } else {
        "".into()
    };
    let child_kind = if children > 0 {
        if !data.value.block() || data.value.contains_inlines() {
            "☰"
        } else {
            "□"
        }
    } else {
        ""
    };
    let branch = if indent.is_empty() {
        ""
    } else if last_child {
        "└>"
    } else {
        "├>"
    };
    println!(
        "{indent}{branch}{kind} {:?} {}{child_count}{child_kind}",
        data.value, data.sourcepos
    );
    drop(data);
    for child in node.children() {
        print_recursive(
            child,
            &format!("{}{}", indent, if last_child { "  " } else { "│ " }),
        );
    }
}

/// Headless `Context` with the editor's test fonts registered.
#[cfg(test)]
pub(crate) fn test_egui_ctx() -> Context {
    let ctx = Context::default();
    register_font_system(&ctx, fonts::default_font_database());
    ctx
}

/// Headless editor harness: tests read like the platform call sites
/// (`replace`, `enter_frame`, …).
#[cfg(test)]
mod test {
    use super::*;
    use crate::palette::{Mode, Theme};
    use editor_core::offset_types::Grapheme;
    use egui::RawInput;
    use input::{Event, Location, Region};

    struct TestEditor {
        editor: Editor,
        pending: Vec<Event>,
    }

    impl TestEditor {
        fn new(md: &str) -> Self {
            let mut harness = Self {
                editor: Editor::test(md),
                pending: vec![],
            };
            harness.enter_frame();
            harness
        }

        fn replace(&mut self, start: usize, end: usize, text: &str) {
            self.pending.push(Event::Replace {
                region: Region::BetweenLocations {
                    start: Location::Grapheme(Grapheme(start)),
                    end: Location::Grapheme(Grapheme(end)),
                },
                text: text.to_string(),
                advance_cursor: true,
            });
        }

        /// Runs a full headless egui frame through Editor::show(),
        /// processing all pending events.
        fn enter_frame(&mut self) {
            let ctx = self.editor.edit.renderer.ctx.clone();
            let pending = std::mem::take(&mut self.pending);
            for event in pending {
                self.editor.push_event(event);
            }
            crate::tests::run_frame(&ctx, RawInput::default(), |ui| {
                ui.ctx().set_lb_theme(Theme::default(Mode::Dark));
                self.editor.show(ui);
            });
        }

        fn get_text(&self) -> &str {
            self.editor.edit.renderer.buffer.current.text()
        }

        fn get_selection(&self) -> (usize, usize) {
            let sel = self.editor.edit.renderer.buffer.current.selection;
            (sel.0.0, sel.1.0)
        }
    }

    /// Android autocorrect: the IME deletes the old word then inserts the
    /// replacement, all computed against pre-deletion offsets. The buffer's
    /// OT adjusts the stale insert position so it lands where the deletion
    /// happened.
    #[test]
    fn android_autocorrect() {
        let mut ws = TestEditor::new("hello teh world");

        ws.replace(6, 9, ""); // delete "teh"    → "hello  world"
        ws.replace(9, 9, "the"); // insert at 9     → stale, OT adjusts to 6
        ws.enter_frame();

        assert_eq!(ws.get_text(), "hello the world");
        assert_eq!(ws.get_selection(), (9, 9)); // cursor after "the"
    }
}
