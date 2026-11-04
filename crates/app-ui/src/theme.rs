//! The egui theme, built from the legacy app's design tokens
//! (`apps/app/src/app.css`): its light and dark colors, corner radii, type
//! scale and spacing. The tokens are OKLCH colors; this module converts
//! them to sRGB.

use egui::{
    Color32, CornerRadius, FontFamily, FontId, Margin, Rgba, Shadow, Stroke, Style, TextStyle,
    Theme, ThemePreference, Visuals,
    theme::{DefaultStyle, StyleProvider},
    vec2,
    widget_style::{StyleArgs, TextEditStyle, WidgetState},
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

/// The radius of the legacy app's buttons and fields, `rounded-2xl`: a
/// pill at every height they come in.
pub fn pill() -> CornerRadius {
    radius(1.8)
}

/// The UI type scale: supporting (12 px), body (14 px) and title (16 px).
pub const SUPPORTING_SIZE: f32 = 12.0;
pub const BODY_SIZE: f32 = 14.0;
pub const TITLE_SIZE: f32 = 16.0;

/// The weights the legacy app sets its labels and titles in, besides
/// regular: `font-medium` and `font-semibold`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
}

impl Weight {
    fn family_name(self) -> Option<&'static str> {
        match self {
            Self::Regular => None,
            Self::Medium => Some("medium"),
            Self::Semibold => Some("semibold"),
        }
    }

    fn value(self) -> f32 {
        match self {
            Self::Regular => 400.0,
            Self::Medium => 500.0,
            Self::Semibold => 600.0,
        }
    }
}

/// Give `fonts` the medium and semibold weights of the interface font
/// `key`, when it is a variable font: each weight is a font family of its
/// own, with the proportional fonts behind it for missing glyphs.
pub fn add_weights(fonts: &mut egui::FontDefinitions, key: &str) {
    let Some(data) = fonts.font_data.get(key).cloned() else {
        return;
    };
    let fallbacks = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for weight in [Weight::Medium, Weight::Semibold] {
        let Some(name) = weight.family_name() else {
            continue;
        };
        let mut tweak = data.tweak.clone();
        tweak.coords.push("wght", weight.value());
        let weighted = format!("{key}-{name}");
        fonts.font_data.insert(
            weighted.clone(),
            std::sync::Arc::new(egui::FontData {
                tweak,
                ..(*data).clone()
            }),
        );
        let mut family = vec![weighted];
        family.extend(fallbacks.iter().cloned());
        fonts.families.insert(FontFamily::Name(name.into()), family);
    }
}

/// The interface font at `size` in `weight`, or in the regular weight
/// when the interface font has no weights.
pub fn font(ctx: &egui::Context, size: f32, weight: Weight) -> FontId {
    let family = weight
        .family_name()
        .map(|name| FontFamily::Name(name.into()))
        .filter(|family| ctx.fonts(|fonts| fonts.definitions().families.contains_key(family)))
        .unwrap_or(FontFamily::Proportional);
    FontId::new(size, family)
}

/// Install the light and dark styles and follow the system appearance
/// until the app applies this device's choice. egui's own zoom keys stay
/// off: zoom steps through `app-core`'s levels instead.
pub fn install(ctx: &egui::Context) {
    ctx.set_style_of(Theme::Light, style(Palette::light(), Visuals::light()));
    ctx.set_style_of(Theme::Dark, style(Palette::dark(), Visuals::dark()));
    ctx.replace_widget_theme::<TextEditStyle>(Fields);
    ctx.options_mut(|options| {
        options.theme_preference = ThemePreference::System;
        options.zoom_with_keyboard = false;
    });
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

    // Buttons are the legacy outline buttons: a pill with a hairline
    // border, filled on hover.
    let (fill, hover) = if palette == Palette::dark() {
        (Color32::TRANSPARENT, palette.input.gamma_multiply(0.3))
    } else {
        (palette.background, palette.muted)
    };
    visuals.text_edit_bg_color = Some(palette.input.gamma_multiply(0.5));
    let widgets = &mut visuals.widgets;
    for state in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        state.corner_radius = pill();
        state.fg_stroke = Stroke::new(1.0, palette.foreground);
        state.bg_stroke = Stroke::new(1.0, palette.border);
        state.expansion = 0.0;
    }
    widgets.noninteractive.bg_fill = palette.background;
    widgets.noninteractive.weak_bg_fill = palette.background;
    widgets.inactive.bg_fill = palette.input.gamma_multiply(0.9);
    widgets.inactive.weak_bg_fill = fill;
    widgets.hovered.bg_fill = palette.input;
    widgets.hovered.weak_bg_fill = hover;
    widgets.active.bg_fill = palette.input;
    widgets.active.weak_bg_fill = hover;
    widgets.active.bg_stroke = Stroke::new(1.0, palette.ring);
    widgets.open.bg_fill = palette.muted;
    widgets.open.weak_bg_fill = palette.muted;

    style.visuals = visuals;
    style
}

/// The legacy app's fields: a pill filled with `--input` at half
/// strength and no border, ringed in `--ring` while it has the keyboard.
struct Fields;

impl StyleProvider<TextEditStyle> for Fields {
    fn style(&mut self, args: &StyleArgs<'_>) -> TextEditStyle {
        let mut style: TextEditStyle = DefaultStyle.style(args);
        let palette = if args.style.visuals.dark_mode {
            Palette::dark()
        } else {
            Palette::light()
        };
        let frame = &mut style.atom_layout.frame;
        frame.corner_radius = pill();
        frame.inner_margin = Margin::symmetric(10, 6);
        if frame.fill != Color32::TRANSPARENT {
            frame.fill = palette.input.gamma_multiply(0.5);
        }
        if args.state == WidgetState::Active {
            frame.stroke = Stroke::new(1.0, palette.ring);
            frame.shadow = Shadow {
                offset: [0, 0],
                blur: 0,
                spread: 3,
                color: palette.ring.gamma_multiply(0.3),
            };
        } else {
            frame.stroke = Stroke::NONE;
        }
        style.hint_text_color = palette.muted_foreground;
        style
    }
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
            [radius(0.6), radius(0.8), radius(1.0), radius(1.4), pill()],
            [6, 8, 10, 14, 18].map(CornerRadius::same)
        );
    }
}
