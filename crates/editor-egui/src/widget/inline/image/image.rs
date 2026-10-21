use std::f32;

use comrak::nodes::{AstNode, NodeValue};
use editor_core::offset_types::{Grapheme, RangeExt as _};
use editor_core::operation_types::Operation;
use egui::{self, Vec2};

use crate::TouchTarget;
use crate::input::{Event, Location, Region};
use crate::palette::Theme;
use crate::resolvers::EmbedState;
use crate::widget::utils::NodeValueExt as _;
use crate::widget::utils::wrap_layout::{EmbedKind, EmbedSpec, Layout};
use crate::{MdEdit, MdRender};
use editor_core::images::ImageDims;

/// Side, in points, of the broken-image marker drawn for an image the
/// embedding app can't find.
const MISSING_IMAGE_SIZE: f32 = 48.0;

/// Paints the marker for an image the embedding app can't find: a framed
/// picture crossed out.
pub(crate) fn paint_missing_image(painter: &egui::Painter, rect: egui::Rect, theme: &Theme) {
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, theme.neutral_bg_tertiary()),
        egui::StrokeKind::Inside,
    );

    let side = (rect.width().min(rect.height()) * 0.5).clamp(6.0, 24.0);
    let frame = egui::Rect::from_center_size(rect.center(), Vec2::splat(side));
    let stroke = egui::Stroke::new(1.25, theme.neutral_fg_secondary());
    painter.rect_stroke(frame, 1.0, stroke, egui::StrokeKind::Middle);
    // A hill inside the frame, then a slash through the whole picture.
    painter.line(
        vec![
            frame.left_bottom() + Vec2::new(side * 0.15, -side * 0.2),
            frame.center() + Vec2::new(-side * 0.05, -side * 0.05),
            frame.right_bottom() + Vec2::new(-side * 0.15, -side * 0.2),
        ],
        stroke,
    );
    painter.line_segment(
        [
            frame.right_top() + Vec2::new(side * 0.15, -side * 0.15),
            frame.left_bottom() + Vec2::new(-side * 0.15, side * 0.15),
        ],
        stroke,
    );
}

impl MdRender {
    /// Prefetch textures under `node` so the embed resolver does not drop
    /// them. Used for off-screen neighbors about to scroll into view.
    pub fn warm_images<'a>(&self, node: &'a comrak::nodes::AstNode<'a>) {
        for descendant in node.descendants() {
            if let NodeValue::Image(link) = &descendant.data.borrow().value {
                self.embeds.prefetch(&link.url);
            }
        }
    }
}

impl<'ast> MdRender {
    /// The size, in pixels, an image's box is laid out from, and what the
    /// box paints: the content at its own size, or the broken-image marker
    /// when the embedding app can't find it.
    pub fn embed_texture_size(&self, url: &str) -> (Vec2, EmbedKind) {
        match self.embeds.state(url) {
            EmbedState::Loading(size) | EmbedState::Loaded(size) => (size, EmbedKind::Image),
            EmbedState::NotFound => (
                Vec2::splat(MISSING_IMAGE_SIZE) * self.ctx.pixels_per_point(),
                EmbedKind::Missing,
            ),
        }
    }

    pub fn image_size(&self, texture_size: Vec2, width: f32, requested: ImageDims) -> Vec2 {
        // Constrain the image to fit the renderer with a margin of breathing
        // room. Clamp to non-negative — when the renderer is too small to
        // satisfy the margin (initial frames before viewport is known,
        // zero-height containers), the image collapses to 0 rather than
        // letting negative dimensions corrupt the surrounding block layout.
        let image_max_size = (Vec2::new(self.width, self.viewport_height)
            - Vec2::splat(self.layout.margin))
        .max(Vec2::ZERO);

        // Texture dims are device pixels; the layout works in logical points.
        // Convert so a Retina screenshot (ppp 2) isn't shown at 2x real size.
        let natural = texture_size / self.ctx.pixels_per_point();
        let aspect = if natural.x > 0.0 {
            natural.y / natural.x
        } else {
            1.0
        };

        // Obsidian-style explicit dims (`![alt|WxH](url)`) are logical points,
        // not device pixels. They set the box — upscaling past natural size is
        // allowed — but still shrink to fit the viewport, preserving the
        // requested aspect ratio. Width-only keeps the natural aspect.
        if requested.width.is_some() || requested.height.is_some() {
            let target = match (requested.width, requested.height) {
                (Some(w), Some(h)) => Vec2::new(w, h),
                (Some(w), None) => Vec2::new(w, w * aspect),
                (None, Some(h)) if aspect > 0.0 => Vec2::new(h / aspect, h),
                (None, Some(h)) => Vec2::new(h, h),
                (None, None) => unreachable!(),
            };
            let scale = (image_max_size.x / target.x)
                .min(image_max_size.y / target.y)
                .min(1.0);
            return (target * scale).max(Vec2::ZERO);
        }

        // A degenerate texture (a decode with no size yet, a failed one)
        // collapses rather than dividing by zero into NaN geometry.
        if natural.x <= 0.0 || natural.y <= 0.0 {
            return Vec2::ZERO;
        }

        // only shrink images, never stretch beyond their natural size
        let width = width.min(natural.x).min(image_max_size.x);
        let height = (natural.y * width / natural.x).min(image_max_size.y);

        // if height was the binding constraint, recompute width to preserve aspect ratio
        let width = natural.x * height / natural.y;

        Vec2::new(width, height)
    }

    /// Logical-point size an `Image` node will render at, constrained
    /// by its block's width and any Obsidian `|WxH` hint. `None` if
    /// `node` isn't an `Image`, has no leaf-block ancestor, or
    /// collapses to zero on the first frame.
    pub fn image_logical_size(&self, node: &'ast AstNode<'ast>) -> Option<Vec2> {
        let url = match &node.data.borrow().value {
            comrak::nodes::NodeValue::Image(link) => link.url.clone(),
            _ => return None,
        };
        let block_ancestor = node
            .ancestors()
            .skip(1)
            .find(|a| a.data.borrow().value.is_leaf_block())?;
        let width = self.width(block_ancestor);
        let (texture_size, _) = self.embed_texture_size(&url);
        let size = self.image_size(texture_size, width, self.image_dims(node));
        (size.x > 0.0 && size.y > 0.0).then_some(size)
    }

    /// Collapsed → emit one `InlineItem::Embed` covering the syntax.
    /// Revealed / `disable_images` / multi-line / unsizable → fall
    /// through to `layout_link` so the source bytes render for editing.
    pub fn layout_image(
        &self,
        layout: &mut Layout,
        node: &'ast AstNode<'ast>,
        range: (Grapheme, Grapheme),
    ) {
        let node_range = self.node_range(node);
        let single_line = range.contains_range(&node_range, true, true);
        let revealed = self.range_revealed_interior(node_range);
        let collapsed_size = (!self.disable_images && !revealed && single_line)
            .then(|| self.image_logical_size(node))
            .flatten();
        let Some(size) = collapsed_size else {
            self.layout_link(layout, node, range);
            return;
        };
        let url = match &node.data.borrow().value {
            comrak::nodes::NodeValue::Image(link) => link.url.clone(),
            _ => return,
        };
        let (_, kind) = self.embed_texture_size(&url);
        // Always interactive: a plain tap selects, a cmd/keyboard-hidden tap opens.
        layout.interaction_open(
            Self::image_interaction_id_salt(node_range),
            egui::Sense::click(),
        );
        layout.push_embed(EmbedSpec {
            advance: size.x,
            ascent: size.y,
            descent: 0.0,
            source_range: node_range,
            url,
            kind,
        });
        layout.interaction_close();
    }

    /// Interaction-response salt for a rendered image; distinct from
    /// [`Self::link_interaction_id_salt`] so the collapsed-image and
    /// revealed-link handlers don't collide.
    pub fn image_interaction_id_salt(node_range: (Grapheme, Grapheme)) -> egui::Id {
        egui::Id::unique(("md_image", node_range))
    }
}

impl<'ast> MdEdit {
    /// The gesture that *opens* an embed rather than selecting it: a cmd-click
    /// (desktop). Touch taps always select.
    pub fn embed_tap_opens(&self, ui: &egui::Ui) -> bool {
        !self.renderer.touch_mode && ui.ctx().input(|i| i.modifiers.command)
    }

    /// Tap handling for one embed fragment: when `open`, a click opens `url`,
    /// else it selects the node. Registers the fragment rects as a touch
    /// target, whose tap selects the node or (read-only) opens; `salt`
    /// identifies the fragment's `Sense::click` scope. No-op if the embed
    /// wasn't rendered this frame.
    #[allow(clippy::too_many_arguments)]
    pub fn handle_embed_tap(
        &mut self,
        root: &'ast AstNode<'ast>,
        node: &'ast AstNode<'ast>,
        ui: &egui::Ui,
        id: egui::Id,
        ops: &mut Vec<Operation>,
        url: &str,
        salt: egui::Id,
        open: bool,
    ) {
        let response = match self
            .renderer
            .interaction_responses
            .get(&ui.scope_id().with(salt))
        {
            Some(r) => r.clone(), // clone to release the `renderer` borrow
            None => return,
        };

        let node_range = self.renderer.node_range(node);
        let tap = if self.renderer.readonly {
            Event::OpenLink {
                url: url.to_string(),
                wikilink: false,
            }
        } else {
            Event::Select {
                region: node_range.into(),
            }
        };
        self.renderer
            .touch_consume_interaction(ui.scope_id().with(salt), TouchTarget::Tap(tap));

        if open && response.hovered() {
            ui.ctx()
                .output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }
        if response.clicked() {
            if open {
                self.renderer.render_events.push(Event::OpenLink {
                    url: url.to_string(),
                    wikilink: false,
                });
            } else {
                ui.memory_mut(|m| m.request_focus(id));
                let region = Region::BetweenLocations {
                    start: Location::Grapheme(node_range.start()),
                    end: Location::Grapheme(node_range.end()),
                };
                self.calc_operations(ui.ctx(), root, Event::Select { region }, ops);
            }
        }
    }

    /// Open or select an inline image clicked this frame (see [`Self::handle_embed_tap`]).
    pub fn handle_image_interactions(
        &mut self,
        root: &'ast AstNode<'ast>,
        ui: &egui::Ui,
        id: egui::Id,
        ops: &mut Vec<Operation>,
    ) {
        let open = self.embed_tap_opens(ui);
        for node in root.descendants() {
            let url = match &node.data.borrow().value {
                NodeValue::Image(link) => link.url.clone(),
                _ => continue,
            };
            let node_range = self.renderer.node_range(node);
            let salt = MdRender::image_interaction_id_salt(node_range);
            self.handle_embed_tap(root, node, ui, id, ops, &url, salt, open);
        }
    }
}
