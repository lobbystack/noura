//! The egui theme, built from the legacy app's design tokens
//! (`apps/app/src/app.css`): its light and dark colors, corner radii, type
//! scale and spacing. The tokens are OKLCH colors; this module converts
//! them to sRGB.

use egui::{
    Color32, CornerRadius, FontFamily, FontId, Margin, Rgba, Stroke, Style, TextStyle, Theme,
    ThemePreference, Visuals, vec2,
};

/// The design tokens of one appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub background: Color32,
    pub foreground: Color32,
    pub popover: Color32,
    pub primary: Color32,
    pub primary_foreground: Color32,
    pub secondary: Color32,
    pub muted: Color32,
    pub muted_foreground: Color32,
    pub accent: Color32,
    pub destructive: Color32,
    pub border: Color32,
    pub input: Color32,
    pub ring: Color32,
    pub sidebar: Color32,
    pub sidebar_accent: Color32,
    pub sidebar_border: Color32,
}

impl Palette {
    pub fn light() -> Self {
        Self {
            background: oklch(1.0, 0.0, 0.0),
            foreground: oklch(0.145, 0.0, 0.0),
            popover: oklch(1.0, 0.0, 0.0),
            primary: oklch(0.205, 0.0, 0.0),
            primary_foreground: oklch(0.985, 0.0, 0.0),
            secondary: oklch(0.97, 0.0, 0.0),
            muted: oklch(0.97, 0.0, 0.0),
            muted_foreground: oklch(0.556, 0.0, 0.0),
            accent: oklch(0.97, 0.0, 0.0),
            destructive: oklch(0.577, 0.245, 27.325),
            border: oklch(0.922, 0.0, 0.0),
            input: oklch(0.922, 0.0, 0.0),
            ring: oklch(0.708, 0.0, 0.0),
            sidebar: oklch(0.985, 0.0, 0.0),
            sidebar_accent: oklch(0.97, 0.0, 0.0),
            sidebar_border: oklch(0.922, 0.0, 0.0),
        }
    }

    pub fn dark() -> Self {
        Self {
            background: oklch(0.145, 0.0, 0.0),
            foreground: oklch(0.985, 0.0, 0.0),
            popover: oklch(0.205, 0.0, 0.0),
            primary: oklch(0.922, 0.0, 0.0),
            primary_foreground: oklch(0.205, 0.0, 0.0),
            secondary: oklch(0.269, 0.0, 0.0),
            muted: oklch(0.269, 0.0, 0.0),
            muted_foreground: oklch(0.708, 0.0, 0.0),
            accent: oklch(0.269, 0.0, 0.0),
            destructive: oklch(0.704, 0.191, 22.216),
            border: oklch_alpha(1.0, 0.0, 0.0, 0.10),
            input: oklch_alpha(1.0, 0.0, 0.0, 0.15),
            ring: oklch(0.556, 0.0, 0.0),
            sidebar: oklch(0.205, 0.0, 0.0),
            sidebar_accent: oklch(0.269, 0.0, 0.0),
            sidebar_border: oklch_alpha(1.0, 0.0, 0.0, 0.10),
        }
    }

    /// The palette of the appearance `ui` draws with.
    pub fn of(ctx: &egui::Context) -> Self {
        match ctx.theme() {
            Theme::Light => Self::light(),
            Theme::Dark => Self::dark(),
        }
    }
}

/// `--radius` is 0.625rem: 10 px.
const RADIUS: f32 = 10.0;

/// The legacy app's radius scale, `calc(var(--radius) * n)`.
pub fn radius(scale: f32) -> CornerRadius {
    CornerRadius::same((RADIUS * scale).round() as u8)
}

/// The UI type scale: supporting (12 px), body (14 px) and title (16 px).
pub const SUPPORTING_SIZE: f32 = 12.0;
pub const BODY_SIZE: f32 = 14.0;
pub const TITLE_SIZE: f32 = 16.0;

/// Install the light and dark styles and follow the system appearance.
pub fn install(ctx: &egui::Context) {
    ctx.set_style_of(Theme::Light, style(Palette::light(), Visuals::light()));
    ctx.set_style_of(Theme::Dark, style(Palette::dark(), Visuals::dark()));
    ctx.options_mut(|options| options.theme_preference = ThemePreference::System);
}

fn style(palette: Palette, mut visuals: Visuals) -> Style {
    let proportional = |size| FontId::new(size, FontFamily::Proportional);
    let mut style = Style {
        text_styles: [
            (TextStyle::Small, proportional(SUPPORTING_SIZE)),
            (TextStyle::Body, proportional(BODY_SIZE)),
            (TextStyle::Button, proportional(BODY_SIZE)),
            (TextStyle::Heading, proportional(TITLE_SIZE)),
            (
                TextStyle::Monospace,
                FontId::new(13.0, FontFamily::Monospace),
            ),
        ]
        .into(),
        ..Style::default()
    };

    // Tailwind's 4 px spacing unit, as the legacy app uses it.
    style.spacing.item_spacing = vec2(8.0, 8.0);
    style.spacing.button_padding = vec2(12.0, 6.0);
    style.spacing.interact_size = vec2(32.0, 32.0);
    style.spacing.window_margin = Margin::same(16);
    style.spacing.menu_margin = Margin::same(4);
    style.spacing.indent = 12.0;

    visuals.override_text_color = None;
    visuals.panel_fill = palette.background;
    visuals.window_fill = palette.popover;
    visuals.window_stroke = Stroke::new(1.0, palette.border);
    visuals.window_corner_radius = radius(1.4);
    visuals.menu_corner_radius = radius(0.8);
    visuals.extreme_bg_color = palette.background;
    visuals.faint_bg_color = palette.muted;
    visuals.code_bg_color = palette.muted;
    visuals.error_fg_color = palette.destructive;
    visuals.hyperlink_color = palette.foreground;
    visuals.selection.bg_fill = palette.ring.gamma_multiply(0.5);
    visuals.selection.stroke = Stroke::new(1.0, palette.foreground);

    let widgets = &mut visuals.widgets;
    for state in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        state.corner_radius = radius(0.8);
        state.fg_stroke = Stroke::new(1.0, palette.foreground);
        state.expansion = 0.0;
    }
    widgets.noninteractive.bg_fill = palette.background;
    widgets.noninteractive.weak_bg_fill = palette.background;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border);
    // Buttons look like the legacy outline buttons.
    widgets.inactive.bg_fill = palette.background;
    widgets.inactive.weak_bg_fill = palette.background;
    widgets.inactive.bg_stroke = Stroke::new(1.0, palette.input);
    widgets.hovered.bg_fill = palette.accent;
    widgets.hovered.weak_bg_fill = palette.accent;
    widgets.hovered.bg_stroke = Stroke::new(1.0, palette.input);
    widgets.active.bg_fill = palette.accent;
    widgets.active.weak_bg_fill = palette.accent;
    widgets.active.bg_stroke = Stroke::new(1.0, palette.ring);
    widgets.open.bg_fill = palette.accent;
    widgets.open.weak_bg_fill = palette.accent;
    widgets.open.bg_stroke = Stroke::new(1.0, palette.input);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.foreground);

    style.visuals = visuals;
    style
}

/// Convert an OKLCH color (lightness 0 to 1, chroma, hue in degrees) to
/// sRGB, clamping colors outside the sRGB gamut.
pub fn oklch(lightness: f32, chroma: f32, hue: f32) -> Color32 {
    oklch_alpha(lightness, chroma, hue, 1.0)
}

/// [`oklch`] with an alpha, as in `oklch(1 0 0 / 10%)`.
pub fn oklch_alpha(lightness: f32, chroma: f32, hue: f32, alpha: f32) -> Color32 {
    let (sin, cos) = hue.to_radians().sin_cos();
    let (a, b) = (chroma * cos, chroma * sin);
    // OKLab to linear sRGB, by the matrices that define OKLab.
    let l = (lightness + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (lightness - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s = (lightness - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    let red = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
    let green = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s;
    let blue = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;
    let linear = Rgba::from_rgb(
        red.clamp(0.0, 1.0),
        green.clamp(0.0, 1.0),
        blue.clamp(0.0, 1.0),
    );
    let [red, green, blue, _] = Color32::from(linear).to_array();
    Color32::from_rgba_unmultiplied(red, green, blue, (alpha * 255.0).round() as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(color: Color32, hex: u32) -> bool {
        let expected = [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8];
        color.to_array()[..3]
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.abs_diff(expected) <= 2)
    }

    /// The tokens are Tailwind's neutral and red palette; these are its
    /// published sRGB values.
    #[test]
    fn tokens_convert_to_the_palette_s_srgb_values() {
        for (color, hex) in [
            (oklch(1.0, 0.0, 0.0), 0xffffff),
            (oklch(0.985, 0.0, 0.0), 0xfafafa),
            (oklch(0.97, 0.0, 0.0), 0xf5f5f5),
            (oklch(0.922, 0.0, 0.0), 0xe5e5e5),
            (oklch(0.708, 0.0, 0.0), 0xa1a1a1),
            (oklch(0.556, 0.0, 0.0), 0x737373),
            (oklch(0.269, 0.0, 0.0), 0x262626),
            (oklch(0.205, 0.0, 0.0), 0x171717),
            (oklch(0.145, 0.0, 0.0), 0x0a0a0a),
            (oklch(0.577, 0.245, 27.325), 0xe7000b),
            (oklch(0.704, 0.191, 22.216), 0xff6467),
        ] {
            assert!(close(color, hex), "{color:?} is not #{hex:06x}");
        }
    }

    #[test]
    fn translucent_tokens_keep_their_alpha() {
        assert_eq!(
            oklch_alpha(1.0, 0.0, 0.0, 0.10),
            Color32::from_rgba_unmultiplied(255, 255, 255, 26)
        );
    }

    #[test]
    fn the_radius_scale_follows_the_legacy_tokens() {
        assert_eq!(
            [radius(0.6), radius(0.8), radius(1.0), radius(1.4)],
            [6, 8, 10, 14].map(CornerRadius::same)
        );
    }
}
