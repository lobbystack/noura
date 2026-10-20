//! Headless `Editor` driver for unit and property tests. Configures the
//! editor with `ext: "md"` so the markdown render path fires, and
//! exposes a `push` + `enter_frame` step that mirrors a platform's call
//! sites.

use crate::input::Event;
use crate::palette::{Mode, Theme, ThemeExt as _};
use crate::resolvers::{EmbedResolver, EmbedState, LinkResolver, LinkState, ResolvedLink};
use crate::{Editor, MdConfig};
use egui::{Pos2, RawInput, Rect, Ui, Vec2};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const SCREEN_SIZE: Vec2 = Vec2::new(800., 600.);

pub struct TestEditor {
    pub editor: Editor,
    pending: Vec<Event>,
}

impl TestEditor {
    pub fn new(md: &str) -> Self {
        Self::with_embeds(md, Box::new(()))
    }

    /// Build with a custom `EmbedResolver`. Used by tests that exercise
    /// image-load layout behavior.
    pub fn with_embeds(md: &str, embeds: Box<dyn EmbedResolver>) -> Self {
        let mut harness = Self {
            editor: build_editor(md, "md", embeds),
            pending: vec![],
        };
        harness.enter_frame();
        harness
    }

    /// Build with a [`TestEmbeds`] embed resolver and return a handle so
    /// the caller can complete fake loads via [`TestEmbeds::complete`].
    pub fn with_test_embeds(md: &str) -> (Self, Arc<TestEmbeds>) {
        let embeds = Arc::new(TestEmbeds::default());
        let mut harness = Self {
            editor: build_editor(md, "md", Box::new(embeds.clone())),
            pending: vec![],
        };
        harness.enter_frame();
        (harness, embeds)
    }

    pub fn push(&mut self, event: Event) {
        self.pending.push(event);
    }

    /// Runs a full headless egui frame through `Editor::show`,
    /// processing all queued markdown events. Requests focus so
    /// `reveal_ranges` tracks the selection (`handle_input` skips it
    /// when unfocused).
    pub fn enter_frame(&mut self) {
        self.enter_frame_at(SCREEN_SIZE);
    }

    /// Same as [`enter_frame`] but with a caller-provided viewport size —
    /// lets property tests vary width (and thus exercise `width_seq`).
    pub fn enter_frame_at(&mut self, size: Vec2) {
        self.enter_frame_inner(size, true);
    }

    /// Frame with no focus (`reveal_selection = None`) and a viewport
    /// tall enough to defeat scroll virtualization. Together these
    /// pin `wrap_lines` across the sweep — required by tests that
    /// move the cursor and need layout to stay fixed.
    pub fn enter_frame_unfocused(&mut self) {
        self.enter_frame_inner(Vec2::new(SCREEN_SIZE.x, 100_000.0), false);
    }

    fn enter_frame_inner(&mut self, size: Vec2, focused: bool) {
        let ctx = self.editor.edit.renderer.ctx.clone();
        let pending = std::mem::take(&mut self.pending);
        let screen_rect = Rect::from_min_size(Pos2::ZERO, size);
        for event in pending {
            self.editor.push_event(event);
        }
        crate::tests::run_frame(
            &ctx,
            RawInput {
                screen_rect: Some(screen_rect),
                ..Default::default()
            },
            |ui| {
                let ctx = ui.ctx().clone();
                ctx.set_lb_theme(Theme::default(Mode::Dark));
                if focused {
                    self.editor.focus(&ctx);
                } else {
                    self.editor.surrender_focus(&ctx);
                }
                egui::CentralPanel::default().show(ui, |ui| {
                    self.editor.show(ui);
                });
            },
        );
    }

    pub fn get_text(&self) -> &str {
        self.editor.edit.renderer.buffer.current.text()
    }

    /// Inject raw egui key/text events for one frame *without* forcing editor
    /// focus, so focus stays where a widget (e.g. find) parked it.
    pub fn enter_frame_with_input(&mut self, events: Vec<egui::Event>) {
        let ctx = self.editor.edit.renderer.ctx.clone();
        let pending = std::mem::take(&mut self.pending);
        let screen_rect = Rect::from_min_size(Pos2::ZERO, SCREEN_SIZE);
        for event in pending {
            self.editor.push_event(event);
        }
        crate::tests::run_frame(
            &ctx,
            RawInput {
                screen_rect: Some(screen_rect),
                events,
                ..Default::default()
            },
            |ui| {
                ui.ctx().set_lb_theme(Theme::default(Mode::Dark));
                egui::CentralPanel::default().show(ui, |ui| {
                    self.editor.show(ui);
                });
            },
        );
    }
}

/// `EmbedResolver` for tests that exercise async-load completion. Per-URL
/// sizes default to placeholder dims; [`Self::complete`] swaps in a new
/// size and bumps `seq` — the same shape as a real worker thread
/// finishing an image decode.
#[derive(Default)]
pub struct TestEmbeds {
    sizes: Mutex<HashMap<String, Vec2>>,
    seq: AtomicU64,
}

impl TestEmbeds {
    /// Replace `url`'s size and bump `seq`. Mimics a real load completing.
    pub fn complete(&self, url: &str, size: Vec2) {
        self.sizes.lock().unwrap().insert(url.to_string(), size);
        self.seq.fetch_add(1, Ordering::Relaxed);
    }
}

impl EmbedResolver for TestEmbeds {
    fn state(&self, url: &str) -> EmbedState {
        match self.sizes.lock().unwrap().get(url) {
            Some(size) => EmbedState::Loaded(*size),
            None => EmbedState::Loading(Vec2::splat(200.)),
        }
    }
    /// Worst-case resolver: allocates its rect. Real resolvers are paint-only,
    /// but callers must survive an allocating one — embeds paint for
    /// off-screen neighbor rows, where an allocation drags the layout cursor
    /// off the viewport (#4892). Callers pass a non-advancing child ui.
    fn show(&self, ui: &mut Ui, _url: &str, rect: Rect, _rounding: egui::CornerRadius) {
        ui.allocate_rect(rect, egui::Sense::hover());
    }
    fn prefetch(&self, _url: &str) {}
    fn seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }
}

impl EmbedResolver for Arc<TestEmbeds> {
    fn state(&self, url: &str) -> EmbedState {
        (**self).state(url)
    }
    fn show(&self, ui: &mut Ui, url: &str, rect: Rect, rounding: egui::CornerRadius) {
        (**self).show(ui, url, rect, rounding)
    }
    fn prefetch(&self, url: &str) {
        (**self).prefetch(url)
    }
    fn seq(&self) -> u64 {
        (**self).seq()
    }
}

/// `LinkResolver` that resolves every URL to itself (External) and every
/// wikilink to nothing.
pub struct TestLinks;

impl LinkResolver for TestLinks {
    fn resolve_link(&self, url: &str) -> Option<ResolvedLink> {
        Some(ResolvedLink::External(url.to_string()))
    }
    fn resolve_wikilink(&self, _title: &str) -> Option<String> {
        None
    }
    fn link_state(&self, _url: &str) -> LinkState {
        LinkState::Normal
    }
    fn wikilink_state(&self, _title: &str) -> LinkState {
        LinkState::Normal
    }
}

/// `ext` selects the parse path: `"md"` runs the full markdown
/// pipeline; `""` or `"txt"` renders each line as plain text.
fn build_editor(md: &str, ext: &str, embeds: Box<dyn EmbedResolver>) -> Editor {
    let ctx = super::super::test_egui_ctx();
    let mut editor = Editor::new(
        &ctx,
        md,
        MdConfig {
            readonly: false,
            ext: ext.to_string(),
        },
    );
    editor.set_link_resolver(Box::new(TestLinks));
    editor.set_embed_resolver(embeds);
    editor
}
