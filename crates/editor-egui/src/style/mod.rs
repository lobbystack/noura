//! The design tokens the editor reads: space, type, corner radii, washes and
//! the overlay scrollbar. Cut down from Lockbook's `style` module to what the
//! editor uses.

use egui::epaint::Shadow;
use egui::{Color32, FontFamily, FontId, Frame, Margin, Sense, Stroke};

use crate::palette::Mode;
pub use crate::palette::{Theme, ThemeExt};

pub mod overlay_scroll;

/// Toward `neutral_fg` on hover (rows / quiet on primary bg).
pub const FG_HOVER: f32 = Theme::HOVER_WASH;
/// Toward `neutral_fg` while pressed.
pub const FG_PRESS: f32 = Theme::PRESS_WASH;

/// Hover fade-in.
pub const HOVER_ANIM_SECS: f32 = 0.20;

/// Resting outline width.
pub const STROKE_HAIRLINE: f32 = 1.0;

/// Corner radius steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Radius {
    /// 4 pt — swatches, tight chips, dense chrome.
    Sm,
    /// 8 pt — buttons, fields, rows.
    Control,
    /// 12 pt — sheets, large panels.
    Surface,
}

impl Radius {
    pub const fn pts(self) -> u8 {
        match self {
            Radius::Sm => 4,
            Radius::Control => 8,
            Radius::Surface => 12,
        }
    }

    pub fn corner(self) -> egui::CornerRadius {
        egui::CornerRadius::same(self.pts())
    }
}

/// Ordered space steps. Values are points (logical px).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Space {
    /// 2 pt
    Xxs,
    /// 5 pt
    Xs,
    /// 10 pt
    Sm,
    /// 15 pt
    Md,
    /// 25 pt
    Lg,
    /// 40 pt
    Xl,
}

impl Space {
    pub const fn pts(self) -> f32 {
        match self {
            Space::Xxs => 2.0,
            Space::Xs => 5.0,
            Space::Sm => 10.0,
            Space::Md => 15.0,
            Space::Lg => 25.0,
            Space::Xl => 40.0,
        }
    }
}

/// Named type roles: 22 title, 16 heading, 14 body, 12 mono.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeRole {
    Title,
    Heading,
    Body,
    Mono,
}

impl TypeRole {
    pub const fn size(self) -> f32 {
        match self {
            TypeRole::Title => 22.0,
            TypeRole::Heading => 16.0,
            TypeRole::Body => 14.0,
            TypeRole::Mono => 12.0,
        }
    }

    /// Line box (em × 1.4).
    pub const fn line_height(self) -> f32 {
        self.size() * 1.4
    }

    pub const fn family(self) -> FontFamily {
        match self {
            TypeRole::Mono => FontFamily::Monospace,
            _ => FontFamily::Proportional,
        }
    }

    pub fn font_id(self) -> FontId {
        FontId::new(self.size(), self.family())
    }
}

/// Control height: vertical pad + line box + vertical pad.
pub fn control_height() -> f32 {
    Space::Xs.pts() * 2.0 + TypeRole::Body.line_height()
}

pub fn sense_click() -> Sense {
    Sense::CLICK
}

fn shadow_ink(t: &Theme) -> Color32 {
    match t.current {
        Mode::Light => Color32::from_black_alpha(36),
        Mode::Dark => Color32::from_black_alpha(160),
    }
}

/// Soft float under menus, pickers, floating plates.
pub fn overlay_shadow(t: &Theme) -> Shadow {
    Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: shadow_ink(t),
    }
}

/// A floating plate over the canvas.
pub fn canvas_overlay_frame(t: &Theme, inner_pad: Space) -> Frame {
    let p = inner_pad.pts() as i8;
    Frame::new()
        .fill(t.neutral_bg())
        .stroke(Stroke::new(STROKE_HAIRLINE, t.neutral()))
        .corner_radius(Radius::Control.corner())
        .inner_margin(Margin::same(p))
        .shadow(overlay_shadow(t))
}
