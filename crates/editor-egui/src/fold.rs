//! Section folding. A fold tag ([`FOLD_TAG`]) at the end of a heading or
//! list item's first line hides the section's contents and renders as a
//! `···` chip.
//!
//! The chip represents the fold *tag* in that, when you place your cursor to
//! the right of the chip, you are between the fold tag and the folded content.
//! This is why newline in a folded list item inserts a new list item.
//!
//! The chip represents the fold *content* in that, when you replace/delete/copy
//! it, you replace/delete/copy the content. This is accomplished in the logic
//! that handles those operations.

use comrak::nodes::AstNode;
use editor_core::offset_types::{Grapheme, IntoRangeExt as _, RangeExt as _};

use crate::TouchTarget;
use crate::bounds::FoldBounds;
use crate::widget::utils::wrap_layout::{FontFamily, Format, Layout, StyleInfo};
use crate::{Event, MdRender};

pub use editor_core::fold::FOLD_TAG;

/// Visible glyph of the chip a folded section's tag renders as — a
/// single ellipsis glyph, so the dots are spaced as drawn rather than
/// as three text glyphs.
pub const FOLD_CHIP_TEXT: &str = "\u{2026}";

/// Click-target id for a fold's chip. Keyed by tag range so layout and
/// interaction handling derive the same id without the AST node.
fn fold_chip_id_salt(tag: (Grapheme, Grapheme)) -> egui::Id {
    egui::Id::unique(("md_fold_chip", tag.start().0, tag.end().0))
}

// ─── fold bounds ─────────────────────────────────────────────────────

// ─── find reveal ─────────────────────────────────────────────────────

// ─── chip rendering + interaction ────────────────────────────────────

impl<'ast> MdRender {
    /// Lay out an active fold's tag: a `···` chip while the contents are
    /// hidden, a zero-width anchor while a find match force-reveals them
    /// (the chip would misread as hidden content). Never show tag source.
    pub fn layout_fold_chip(
        &self,
        layout: &mut Layout,
        parent: &'ast AstNode<'ast>,
        fold: FoldBounds,
        node_range: (Grapheme, Grapheme),
    ) {
        if self.fold_revealed(&fold) {
            layout.push_override(node_range, "", self.text_format_html_inline(parent));
            return;
        }
        // Breathing room between the preceding text and the chip,
        // outside the capsule. Zero-length source: a click here lands
        // left of the chip.
        layout.push_override(
            node_range.start().into_range(),
            " ",
            self.text_format(parent),
        );
        // Interaction outside the style scope so the capsule's side
        // pads are part of the click target.
        let format = Format {
            family: FontFamily::Phosphor,
            ..self.text_format_html_inline(parent)
        };
        layout.interaction_open(fold_chip_id_salt(fold.tag), egui::Sense::click());
        layout.style_open(StyleInfo {
            format: format.clone(),
            source_range: node_range,
            chip: true,
        });
        layout.push_override(node_range, FOLD_CHIP_TEXT, format);
        layout.style_close();
        layout.interaction_close();
    }

    /// Expand a folded section when its `···` chip is clicked.
    /// Must run after `interact_fragments`.
    pub fn handle_fold_interactions(&mut self, ui: &egui::Ui) {
        if !self.interactive {
            return;
        }
        let parent_base = ui.scope_id();
        for i in 0..self.bounds.folds.len() {
            let fold = self.bounds.folds[i];
            let id = parent_base.with(fold_chip_id_salt(fold.tag));
            let Some(response) = self.interaction_responses.get(&id).cloned() else {
                continue;
            };

            let unfold = Event::Unfold { tag: fold.tag };
            self.touch_consume_interaction(id, TouchTarget::Tap(unfold.clone()));

            if response.hovered() {
                ui.ctx()
                    .output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                self.render_events.push(unfold);
            }
            response.on_hover_text("Show Contents");
        }
    }
}

// ─── navigation + selection atoms ────────────────────────────────────

// ─── capsule semantics for edits ─────────────────────────────────────

// ─── event hooks ─────────────────────────────────────────────────────
