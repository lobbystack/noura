use egui::{
    Color32, Id, Stroke, Visuals,
    style::{self},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Theme {
    #[serde(skip)]
    pub current: Mode,

    pub dim: ThemeVariant,
    pub light_prefs: Preferences,

    pub bright: ThemeVariant,
    pub dark_prefs: Preferences,

    #[serde(default)]
    pub code: Option<CodeTheme>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct CodeTheme {
    pub light: CodeVariant,
    pub dark: CodeVariant,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct CodeVariant {
    #[serde(with = "color32_hex")]
    pub background: Color32,
    #[serde(with = "color32_hex")]
    pub text: Color32,
    #[serde(with = "color32_hex")]
    pub comment: Color32,
    #[serde(with = "color32_hex")]
    pub class: Color32,
    #[serde(with = "color32_hex")]
    pub function: Color32,
    #[serde(with = "color32_hex")]
    pub keyword: Color32,
    #[serde(with = "color32_hex")]
    pub string: Color32,
    #[serde(with = "color32_hex")]
    pub number: Color32,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct ThemeVariant {
    #[serde(with = "color32_hex")]
    pub black: Color32,
    #[serde(with = "color32_hex")]
    pub grey: Color32,
    #[serde(with = "color32_hex")]
    pub red: Color32,
    #[serde(with = "color32_hex")]
    pub green: Color32,
    #[serde(with = "color32_hex")]
    pub yellow: Color32,
    #[serde(with = "color32_hex")]
    pub blue: Color32,
    #[serde(with = "color32_hex")]
    pub magenta: Color32,
    #[serde(with = "color32_hex")]
    pub cyan: Color32,
    #[serde(with = "color32_hex")]
    pub white: Color32,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Preferences {
    pub primary: Palette,
    pub secondary: Palette,
    pub tertiary: Palette,
    pub quaternary: Palette,
}

mod color32_hex {
    use egui::Color32;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(color: &Color32, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let hex = format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b());
        serializer.serialize_str(&hex)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Color32, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let s = s.trim_start_matches('#');
        if s.len() != 6 {
            return Err(serde::de::Error::custom("expected 6 hex digits"));
        }
        let r = u8::from_str_radix(&s[0..2], 16).map_err(serde::de::Error::custom)?;
        let g = u8::from_str_radix(&s[2..4], 16).map_err(serde::de::Error::custom)?;
        let b = u8::from_str_radix(&s[4..6], 16).map_err(serde::de::Error::custom)?;
        Ok(Color32::from_rgb(r, g, b))
    }
}

impl Theme {
    pub fn prefs(&self) -> Preferences {
        match self.current {
            Mode::Light => self.light_prefs,
            Mode::Dark => self.dark_prefs,
        }
    }

    pub fn fg(&self) -> ThemeVariant {
        match self.current {
            Mode::Light => self.dim,
            Mode::Dark => self.bright,
        }
    }

    /// Theme primary hue (selection, folders).
    pub fn accent(&self) -> Color32 {
        self.fg().get_color(self.prefs().primary)
    }

    /// Destructive ink — theme red.
    pub fn danger(&self) -> Color32 {
        self.fg().red
    }

    pub fn bg(&self) -> ThemeVariant {
        match self.current {
            Mode::Light => self.bright,
            Mode::Dark => self.dim,
        }
    }

    pub fn light(&self) -> bool {
        match self.current {
            Mode::Light => true,
            Mode::Dark => false,
        }
    }

    pub fn dark(&self) -> bool {
        !self.light()
    }

    pub fn code_variant(&self) -> Option<CodeVariant> {
        self.code.map(|code| match self.current {
            Mode::Light => code.light,
            Mode::Dark => code.dark,
        })
    }

    /// Returns the foreground neutral color i.e. black in light mode, white in
    /// dark mode. Used for text and icons.
    pub fn neutral_fg(&self) -> Color32 {
        match self.current {
            Mode::Light => self.bright.black,
            Mode::Dark => self.dim.white,
        }
    }

    /// Returns the secondary foreground neutral color i.e. off-black in light
    /// mode, off-white in dark mode. Used for de-emphasized foreground elements
    /// like markdown list markers and file tree icons.
    pub fn neutral_fg_secondary(&self) -> Color32 {
        match self.current {
            Mode::Light => self.bright.grey.lerp_to_gamma(self.bright.black, 0.5),
            Mode::Dark => self.dim.grey.lerp_to_gamma(self.dim.white, 0.5),
        }
    }

    /// Mid neutral (grey) — borders / strokes.
    pub fn neutral(&self) -> Color32 {
        match self.current {
            Mode::Light => self.bright.grey.lerp_to_gamma(self.bright.black, 0.3),
            Mode::Dark => self.dim.grey.lerp_to_gamma(self.dim.white, 0.25),
        }
    }

    /// Wash amount: rest → `neutral_fg` on hover.
    pub const HOVER_WASH: f32 = 0.05;
    /// Wash amount: rest → `neutral_fg` while pressed.
    pub const PRESS_WASH: f32 = 0.10;

    /// `rest` blended toward `neutral_fg` by `amount` (clamped 0..=1).
    pub fn wash_toward_neutral_fg(&self, rest: Color32, amount: f32) -> Color32 {
        rest.lerp_to_gamma(self.neutral_fg(), amount.clamp(0.0, 1.0))
    }

    /// Tertiary background — controls / raised plates on secondary bg.
    pub fn neutral_bg_tertiary(&self) -> Color32 {
        match self.current {
            Mode::Light => self.bright.grey.lerp_to_gamma(self.bright.black, 0.15),
            Mode::Dark => self.dim.grey.lerp_to_gamma(self.dim.white, 0.15),
        }
    }

    /// Secondary background — panels / chrome.
    pub fn neutral_bg_secondary(&self) -> Color32 {
        match self.current {
            Mode::Light => self.bright.grey,
            Mode::Dark => self.dim.grey,
        }
    }

    /// Primary background — editable content / extreme bg.
    pub fn neutral_bg(&self) -> Color32 {
        match self.current {
            Mode::Light => self.bright.white,
            Mode::Dark => self.dim.black,
        }
    }
}

/// Returns the color that, painted at opacity `alpha` over `background`,
/// composites to `target` (`target = alpha·x + (1−alpha)·background`,
/// solved for `x`). Painting the result over a *different* background lets
/// that background show through while keeping the `target` look over
/// `background` — e.g. a code pill that matches its opaque appearance over
/// the page yet lets a selection behind it bleed through. Solved in egui's
/// gamma (sRGB-byte) blend space; channels clamp to `[0, 255]`, so an
/// `alpha` too low to reach `target` degrades gracefully.
pub fn translucent_over(target: Color32, background: Color32, alpha: f32) -> Color32 {
    let a = alpha.clamp(f32::EPSILON, 1.0);
    let solve = |t: u8, b: u8| {
        (((t as f32) - (1.0 - a) * (b as f32)) / a)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgba_unmultiplied(
        solve(target.r(), background.r()),
        solve(target.g(), background.g()),
        solve(target.b(), background.b()),
        (a * 255.0).round() as u8,
    )
}

pub fn ensure_normal_text_contrast(color: Color32, background: Color32) -> Color32 {
    if contrast_ratio(color, background).passes_normal_text() {
        return color;
    }

    let toward_black = contrast_ratio(Color32::BLACK, background);
    let toward_white = contrast_ratio(Color32::WHITE, background);
    let target = if toward_black >= toward_white {
        Color32::BLACK
    } else {
        Color32::WHITE
    };

    let mut lo = 0.0f32;
    let mut hi = 1.0f32;
    for _ in 0..12 {
        let mid = (lo + hi) / 2.0;
        if contrast_ratio(color.lerp_to_gamma(target, mid), background).passes_normal_text() {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    color.lerp_to_gamma(target, hi)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContrastRatio {
    /// Below the 3:1 threshold used for large text and non-text UI.
    Low,
    /// Meets 3:1, used for large text and non-text UI.
    LargeText,
    /// Meets 4.5:1, used for normal text.
    NormalText,
    /// Maximum 21:1 contrast, pure black against pure white.
    Maximum,
}

impl ContrastRatio {
    pub fn passes_normal_text(self) -> bool {
        self >= Self::NormalText
    }
}

/// WCAG contrast threshold between two opaque colors.
pub fn contrast_ratio(a: Color32, b: Color32) -> ContrastRatio {
    let a = relative_luminance(a);
    let b = relative_luminance(b);
    let (lighter, darker) = if a >= b { (a, b) } else { (b, a) };
    let ratio = (lighter + 0.05) / (darker + 0.05);
    if (ratio - 21.0).abs() < 0.01 {
        ContrastRatio::Maximum
    } else if ratio >= 4.5 {
        ContrastRatio::NormalText
    } else if ratio >= 3.0 {
        ContrastRatio::LargeText
    } else {
        ContrastRatio::Low
    }
}

fn relative_luminance(color: Color32) -> f32 {
    fn channel(c: u8) -> f32 {
        let c = c as f32 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Light,
    Dark,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Palette {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
}

impl ThemeVariant {
    pub fn get_color(&self, p: Palette) -> Color32 {
        match p {
            Palette::Black => self.black,
            Palette::Red => self.red,
            Palette::Green => self.green,
            Palette::Yellow => self.yellow,
            Palette::Blue => self.blue,
            Palette::Magenta => self.magenta,
            Palette::Cyan => self.cyan,
            Palette::White => self.white,
        }
    }
}

impl TryFrom<&str> for Palette {
    type Error = ();

    fn try_from(name: &str) -> Result<Self, Self::Error> {
        match name {
            "black" => Ok(Palette::Black),
            "red" => Ok(Palette::Red),
            "green" => Ok(Palette::Green),
            "yellow" => Ok(Palette::Yellow),
            "blue" => Ok(Palette::Blue),
            "magenta" => Ok(Palette::Magenta),
            "cyan" => Ok(Palette::Cyan),
            "white" => Ok(Palette::White),
            _ => Err(()),
        }
    }
}

impl Theme {
    pub fn default(current: Mode) -> Self {
        Self::default_theme(current)
    }

    pub fn from_android_material(
        current: Mode,
        dim: ThemeVariant,
        light_prefs: Preferences,
        bright: ThemeVariant,
        dark_prefs: Preferences,
    ) -> Self {
        Self {
            current,
            dim,
            light_prefs,
            bright,
            dark_prefs,
            code: None,
        }
    }

    pub fn with_mode(mut self, mode: Mode) -> Self {
        self.current = mode;
        self
    }

    /// The default theme is mnemonic by travis
    pub fn default_theme(current: Mode) -> Self {
        Self {
            current,
            dim: ThemeVariant {
                black: Color32::from_rgb(0x10, 0x10, 0x10),
                red: Color32::from_rgb(0xDF, 0x20, 0x40),
                green: Color32::from_rgb(0x00, 0xB3, 0x71),
                yellow: Color32::from_rgb(0xE6, 0xAC, 0x00),
                blue: Color32::from_rgb(0x20, 0x7F, 0xDF),
                magenta: Color32::from_rgb(0x78, 0x55, 0xAA),
                cyan: Color32::from_rgb(0x00, 0xBB, 0xCC),
                white: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                grey: Color32::from_rgb(0x1D, 0x1D, 0x1D),
            },
            light_prefs: Preferences {
                primary: Palette::Blue,
                secondary: Palette::Green,
                tertiary: Palette::Magenta,
                quaternary: Palette::Cyan,
            },
            bright: ThemeVariant {
                black: Color32::from_rgb(0x10, 0x10, 0x10),
                grey: Color32::from_rgb(0xF6, 0xF6, 0xF6),
                red: Color32::from_rgb(0xFF, 0x66, 0x80),
                green: Color32::from_rgb(0x67, 0xE4, 0xB6),
                yellow: Color32::from_rgb(0xFF, 0xDB, 0x70),
                blue: Color32::from_rgb(0x66, 0xB2, 0xFF),
                magenta: Color32::from_rgb(0xAC, 0x8C, 0xD9),
                cyan: Color32::from_rgb(0x6E, 0xEC, 0xF7),
                white: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            },
            dark_prefs: Preferences {
                primary: Palette::Blue,
                secondary: Palette::Green,
                tertiary: Palette::Magenta,
                quaternary: Palette::Cyan,
            },
            code: None,
        }
    }

    pub fn apple(current: Mode) -> Self {
        Self {
            current,
            dim: ThemeVariant {
                black: Color32::from_rgb(0x00, 0x00, 0x00),
                grey: Color32::from_rgb(0x1D, 0x1D, 0x1D),
                white: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                red: Color32::from_rgb(0xFF, 0x38, 0x3C),
                green: Color32::from_rgb(0x34, 0xC7, 0x59),
                yellow: Color32::from_rgb(0xFF, 0xCC, 0x00),
                blue: Color32::from_rgb(0x00, 0x6B, 0xC9),
                magenta: Color32::from_rgb(0xCB, 0x30, 0xE0),
                cyan: Color32::from_rgb(0x00, 0xC0, 0xE8),
            },
            light_prefs: Preferences {
                primary: Palette::Blue,
                secondary: Palette::Green,
                tertiary: Palette::Magenta,
                quaternary: Palette::Cyan,
            },
            bright: ThemeVariant {
                black: Color32::from_rgb(0x00, 0x00, 0x00),
                grey: Color32::from_rgb(0xF6, 0xF6, 0xF6),
                white: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                red: Color32::from_rgb(0xFF, 0x42, 0x45),
                green: Color32::from_rgb(0x30, 0xD1, 0x58),
                yellow: Color32::from_rgb(0xFF, 0xD6, 0x00),
                blue: Color32::from_rgb(0x00, 0x91, 0xFF),
                magenta: Color32::from_rgb(0xDB, 0x34, 0xF2),
                cyan: Color32::from_rgb(0x3C, 0xD3, 0xFE),
            },
            dark_prefs: Preferences {
                primary: Palette::Blue,
                secondary: Palette::Green,
                tertiary: Palette::Magenta,
                quaternary: Palette::Cyan,
            },
            code: Some(CodeTheme {
                light: CodeVariant {
                    background: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                    text: Color32::from_rgb(0x00, 0x00, 0x00),
                    comment: Color32::from_rgb(0x5D, 0x6C, 0x79),
                    class: Color32::from_rgb(0x1C, 0x46, 0x4A),
                    function: Color32::from_rgb(0x32, 0x6D, 0x74),
                    keyword: Color32::from_rgb(0x9B, 0x23, 0x93),
                    string: Color32::from_rgb(0xC4, 0x1A, 0x16),
                    number: Color32::from_rgb(0x1C, 0x00, 0xCF),
                },
                dark: CodeVariant {
                    background: Color32::from_rgb(0x25, 0x25, 0x2A),
                    text: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                    comment: Color32::from_rgb(0x6C, 0x79, 0x86),
                    class: Color32::from_rgb(0x9E, 0xF1, 0xDD),
                    function: Color32::from_rgb(0x67, 0xB7, 0xA4),
                    keyword: Color32::from_rgb(0xFC, 0x5F, 0xA3),
                    string: Color32::from_rgb(0xFC, 0x6A, 0x5D),
                    number: Color32::from_rgb(0xD0, 0xBF, 0x69),
                },
            }),
        }
    }

    pub fn apple_macos(current: Mode) -> Self {
        let mut theme = Self::apple(current);
        theme.dim.black = Color32::from_rgb(0x10, 0x10, 0x10);
        theme
    }

    pub fn darcula(current: Mode) -> Self {
        Self {
            current,
            dim: ThemeVariant {
                black: Color32::from_rgb(0x1C, 0x1B, 0x22),
                grey: Color32::from_rgb(0x35, 0x37, 0x45),
                white: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                red: Color32::from_rgb(0xCB, 0x3A, 0x2A),
                green: Color32::from_rgb(0x14, 0x71, 0x0A),
                yellow: Color32::from_rgb(0xFF, 0xB8, 0x6C),
                blue: Color32::from_rgb(0x64, 0x4A, 0xC9),
                magenta: Color32::from_rgb(0xA3, 0x14, 0x4D),
                cyan: Color32::from_rgb(0x03, 0x6A, 0x96),
            },
            dark_prefs: Preferences {
                primary: Palette::Blue,
                secondary: Palette::Green,
                tertiary: Palette::Yellow,
                quaternary: Palette::Cyan,
            },
            bright: ThemeVariant {
                black: Color32::from_rgb(0x1F, 0x1F, 0x1F),
                white: Color32::from_rgb(0xFF, 0xFB, 0xEB),
                grey: Color32::from_rgb(0xCF, 0xCF, 0xDE),
                red: Color32::from_rgb(0xFF, 0x55, 0x55),
                green: Color32::from_rgb(0x50, 0xFA, 0x7B),
                yellow: Color32::from_rgb(0xF1, 0xFA, 0x8C),
                blue: Color32::from_rgb(0xBD, 0x93, 0xF9),
                magenta: Color32::from_rgb(0xFF, 0x79, 0xC6),
                cyan: Color32::from_rgb(0x8B, 0xE9, 0xFD),
            },
            light_prefs: Preferences {
                primary: Palette::Blue,
                secondary: Palette::Green,
                tertiary: Palette::Yellow,
                quaternary: Palette::Cyan,
            },
            code: None,
        }
    }

    pub fn intellij(current: Mode) -> Self {
        Self {
            current,
            dim: ThemeVariant {
                black: Color32::from_rgb(0x1E, 0x1F, 0x22),
                grey: Color32::from_rgb(0x2B, 0x2D, 0x30),
                white: Color32::from_rgb(0xDF, 0xE1, 0xE5),
                red: Color32::from_rgb(0xDB, 0x58, 0x60),
                green: Color32::from_rgb(0x59, 0xA8, 0x69),
                yellow: Color32::from_rgb(0xC5, 0x68, 0x23),
                blue: Color32::from_rgb(0x35, 0x74, 0xF0),
                magenta: Color32::from_rgb(0x87, 0x10, 0x94),
                cyan: Color32::from_rgb(0x00, 0x62, 0x7A),
            },
            dark_prefs: Preferences {
                primary: Palette::Blue,
                secondary: Palette::Green,
                tertiary: Palette::Yellow,
                quaternary: Palette::Cyan,
            },
            bright: ThemeVariant {
                black: Color32::from_rgb(0x27, 0x28, 0x2E),
                grey: Color32::from_rgb(0xEB, 0xEC, 0xF0),
                white: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                red: Color32::from_rgb(0xFF, 0x6B, 0x68),
                green: Color32::from_rgb(0xA5, 0xC2, 0x61),
                yellow: Color32::from_rgb(0xCC, 0x78, 0x32),
                blue: Color32::from_rgb(0x68, 0x97, 0xBB),
                magenta: Color32::from_rgb(0x98, 0x76, 0xAA),
                cyan: Color32::from_rgb(0xA9, 0xB7, 0xC6),
            },
            light_prefs: Preferences {
                primary: Palette::Blue,
                secondary: Palette::Green,
                tertiary: Palette::Yellow,
                quaternary: Palette::Cyan,
            },
            code: None,
        }
    }

    pub fn vscode(current: Mode) -> Self {
        Self {
            current,
            dim: ThemeVariant {
                black: Color32::from_rgb(0x1E, 0x1E, 0x1E),
                grey: Color32::from_rgb(0x33, 0x33, 0x33),
                white: Color32::from_rgb(0xff, 0xff, 0xff),
                red: Color32::from_rgb(0xD0, 0x37, 0x2D),
                green: Color32::from_rgb(0x37, 0x7E, 0x22),
                yellow: Color32::from_rgb(0x6F, 0x55, 0x29),
                blue: Color32::from_rgb(0x00, 0x00, 0xF5),
                magenta: Color32::from_rgb(0x96, 0x26, 0x1F),
                cyan: Color32::from_rgb(0x34, 0x78, 0xC6),
            },
            dark_prefs: Preferences {
                primary: Palette::Cyan,
                secondary: Palette::Blue,
                tertiary: Palette::Green,
                quaternary: Palette::Magenta,
            },
            bright: ThemeVariant {
                black: Color32::from_rgb(0x00, 0x00, 0x00),
                grey: Color32::from_rgb(0xF3, 0xF3, 0xF3),
                white: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                red: Color32::from_rgb(0xD0, 0x37, 0x2D),
                green: Color32::from_rgb(0x6B, 0xA4, 0x56),
                yellow: Color32::from_rgb(0xDC, 0xDC, 0xAF),
                blue: Color32::from_rgb(0x67, 0x9B, 0xD1),
                magenta: Color32::from_rgb(0xC5, 0x94, 0x7C),
                cyan: Color32::from_rgb(0x34, 0x78, 0xC6),
            },
            light_prefs: Preferences {
                primary: Palette::Cyan,
                secondary: Palette::Blue,
                tertiary: Palette::Green,
                quaternary: Palette::Magenta,
            },
            code: None,
        }
    }

    pub fn catppuccin(current: Mode) -> Self {
        Self {
            current,
            bright: ThemeVariant {
                black: Color32::from_rgb(0x4c, 0x4f, 0x69),
                grey: Color32::from_rgb(0xe6, 0xe9, 0xef),
                white: Color32::from_rgb(0xef, 0xf1, 0xf5),
                red: Color32::from_rgb(0xf3, 0x8b, 0xa8),
                green: Color32::from_rgb(0xa6, 0xe3, 0xa1),
                yellow: Color32::from_rgb(0xf9, 0xe2, 0xaf),
                blue: Color32::from_rgb(0x89, 0xb4, 0xfa),
                magenta: Color32::from_rgb(0xeb, 0xa0, 0xac),
                cyan: Color32::from_rgb(0x94, 0xe2, 0xd5),
            },
            dark_prefs: Preferences {
                primary: Palette::Red,
                secondary: Palette::Blue,
                tertiary: Palette::Green,
                quaternary: Palette::Yellow,
            },
            dim: ThemeVariant {
                black: Color32::from_rgb(0x11, 0x11, 0x1b),
                grey: Color32::from_rgb(0x1e, 0x1e, 0x2e),
                white: Color32::from_rgb(0xcd, 0xd6, 0xf4),
                red: Color32::from_rgb(0xd2, 0x0f, 0x39),
                green: Color32::from_rgb(0x40, 0xa0, 0x2b),
                yellow: Color32::from_rgb(0xdf, 0x8e, 0x1d),
                blue: Color32::from_rgb(0x1e, 0x66, 0xf5),
                magenta: Color32::from_rgb(0xe6, 0x45, 0x53),
                cyan: Color32::from_rgb(0x17, 0x92, 0x99),
            },
            light_prefs: Preferences {
                primary: Palette::Red,
                secondary: Palette::Blue,
                tertiary: Palette::Green,
                quaternary: Palette::Yellow,
            },
            code: None,
        }
    }
}

pub trait ThemeExt {
    fn set_lb_theme(&self, t: Theme);
    fn get_lb_theme(&self) -> Theme;
}

impl ThemeExt for egui::Context {
    fn set_lb_theme(&self, t: Theme) {
        self.memory_mut(|m| m.data.insert_temp(Id::unique("theme"), t));
        self.set_visuals(t.base_visuals());
    }

    /// The theme an app set with [`ThemeExt::set_lb_theme`], or the default
    /// theme for egui's light or dark mode.
    fn get_lb_theme(&self) -> Theme {
        self.memory_mut(|m| m.data.get_temp(Id::unique("theme")))
            .unwrap_or_else(|| {
                let mode = if self.global_style().visuals.dark_mode {
                    Mode::Dark
                } else {
                    Mode::Light
                };
                Theme::default(mode)
            })
    }
}

impl Theme {
    fn base_visuals(&self) -> egui::Visuals {
        let mut base = egui::Visuals {
            dark_mode: self.current == Mode::Dark,
            override_text_color: None,
            window_fill: self.neutral_bg_secondary(),
            extreme_bg_color: self.neutral_bg(),
            selection: {
                let accent = self.fg().get_color(self.prefs().primary);
                let paper = self.neutral_bg();
                style::Selection {
                    bg_fill: paper.lerp_to_gamma(accent, 0.18),
                    stroke: Stroke::new(1.0_f32, paper.lerp_to_gamma(accent, 0.45)),
                }
            },
            hyperlink_color: self.fg().get_color(self.prefs().secondary),
            faint_bg_color: self.neutral_bg_secondary(),
            code_bg_color: self.neutral_bg_secondary(),
            warn_fg_color: self.fg().yellow,
            error_fg_color: self.fg().red,
            panel_fill: self.neutral_bg_secondary(),
            ..if self.current == Mode::Light {
                Visuals::light()
            } else {
                Visuals::dark()
            }
        };

        base.widgets.noninteractive.bg_fill = self.neutral_bg_tertiary();
        base.widgets.noninteractive.weak_bg_fill = self.neutral_bg_tertiary();
        base.widgets.noninteractive.fg_stroke.color = self.neutral_fg();
        base.widgets.noninteractive.bg_stroke.color = self.neutral();

        base.widgets.inactive.bg_fill = self.neutral_bg_tertiary();
        base.widgets.inactive.weak_bg_fill = self.neutral_bg_tertiary();
        base.widgets.inactive.fg_stroke.color = self.neutral_fg();
        base.widgets.inactive.bg_stroke.color = self.neutral();

        // Hover/press are programmatic ink washes on the resting control plate
        // (tertiary), not a jump to `neutral()` hairline grey.
        let control_rest = self.neutral_bg_tertiary();
        base.widgets.hovered.bg_fill = self.wash_toward_neutral_fg(control_rest, Self::HOVER_WASH);
        base.widgets.hovered.weak_bg_fill =
            self.wash_toward_neutral_fg(control_rest, Self::HOVER_WASH);
        base.widgets.hovered.fg_stroke.color = self.neutral_fg();
        base.widgets.hovered.bg_stroke.color = self.neutral();

        base.widgets.active.bg_fill = self.wash_toward_neutral_fg(control_rest, Self::PRESS_WASH);
        base.widgets.active.weak_bg_fill =
            self.wash_toward_neutral_fg(control_rest, Self::PRESS_WASH);
        base.widgets.active.fg_stroke.color = self.neutral_fg();
        base.widgets.active.bg_stroke.color = self.neutral();

        base.widgets.open.bg_fill = self.neutral_bg_tertiary();
        base.widgets.open.weak_bg_fill = self.neutral_bg_tertiary();
        base.widgets.open.fg_stroke.color = self.neutral_fg();
        base.widgets.open.bg_stroke.color = self.neutral();

        base
    }
}

/// Deterministic palette for a username — FNV-like rolling hash picks one of
/// six hues so a user has a stable color across sessions without persistence.
pub fn username_color(name: &str) -> Palette {
    const COLORS: [Palette; 6] = [
        Palette::Red,
        Palette::Green,
        Palette::Yellow,
        Palette::Blue,
        Palette::Magenta,
        Palette::Cyan,
    ];
    let hash = name
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    COLORS[hash as usize % COLORS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Composite `src` over `dst` the way egui blends premultiplied colors
    /// in gamma space: `out = src_premul + (1 − src_a)·dst`.
    fn over(src: Color32, dst: Color32) -> [u8; 3] {
        let sa = src.a() as f32 / 255.0;
        let f = |s: u8, d: u8| (s as f32 + (1.0 - sa) * d as f32).round() as u8;
        [
            f(src.r(), dst.r()),
            f(src.g(), dst.g()),
            f(src.b(), dst.b()),
        ]
    }

    /// `translucent_over` must reproduce `target` when composited back over
    /// `background` — verified for the actual code-pill colors in both
    /// themes so clamping never silently shifts the unselected look.
    #[test]
    fn translucent_over_round_trips_code_pill() {
        for (label, mode) in [("light", Mode::Light), ("dark", Mode::Dark)] {
            let theme = Theme::default(mode);
            let target = theme.neutral_bg_secondary();
            let bg = theme.neutral_bg();
            let x = translucent_over(target, bg, 0.5);
            let got = over(x, bg);
            for (g, t) in got.iter().zip([target.r(), target.g(), target.b()]) {
                assert!(
                    (*g as i32 - t as i32).abs() <= 2,
                    "{label}: composite {got:?} != target {:?}",
                    [target.r(), target.g(), target.b()]
                );
            }
        }
    }

    #[test]
    fn contrast_ratio_classifies_wcag_thresholds() {
        assert_eq!(
            contrast_ratio(Color32::BLACK, Color32::WHITE),
            ContrastRatio::Maximum
        );
        assert_eq!(
            contrast_ratio(Color32::WHITE, Color32::WHITE),
            ContrastRatio::Low
        );
    }

    #[test]
    fn apple_theme_accent_passes_inline_code_contrast() {
        for mode in [Mode::Light, Mode::Dark] {
            let theme = Theme::apple(mode);
            let accent = theme.fg().get_color(theme.prefs().primary);
            let background = theme.neutral_bg_secondary();
            assert!(contrast_ratio(accent, background).passes_normal_text());
        }
    }

    #[test]
    fn system_accents_adjust_to_pass_normal_text() {
        let light_accents = [
            Color32::from_rgb(0xFF, 0x38, 0x3C),
            Color32::from_rgb(0xFF, 0x8D, 0x28),
            Color32::from_rgb(0xFF, 0xCC, 0x00),
            Color32::from_rgb(0x34, 0xC7, 0x59),
            Color32::from_rgb(0x00, 0x88, 0xFF),
            Color32::from_rgb(0xCB, 0x30, 0xE0),
            Color32::from_rgb(0xFF, 0x2D, 0x55),
            Color32::from_rgb(0x8E, 0x8E, 0x93),
        ];
        let dark_accents = [
            Color32::from_rgb(0xFF, 0x42, 0x45),
            Color32::from_rgb(0xFF, 0x92, 0x30),
            Color32::from_rgb(0xFF, 0xD6, 0x00),
            Color32::from_rgb(0x30, 0xD1, 0x58),
            Color32::from_rgb(0x00, 0x91, 0xFF),
            Color32::from_rgb(0xDB, 0x34, 0xF2),
            Color32::from_rgb(0xFF, 0x37, 0x5F),
            Color32::from_rgb(0x8E, 0x8E, 0x93),
        ];

        let theme = Theme::apple(Mode::Light);
        for accent in light_accents {
            let adjusted = ensure_normal_text_contrast(accent, theme.bright.grey);
            assert!(contrast_ratio(adjusted, theme.bright.grey).passes_normal_text());
        }
        for accent in dark_accents {
            let adjusted = ensure_normal_text_contrast(accent, theme.dim.grey);
            assert!(contrast_ratio(adjusted, theme.dim.grey).passes_normal_text());
        }

        assert_eq!(
            ensure_normal_text_contrast(theme.dim.blue, theme.bright.grey),
            theme.dim.blue
        );
        assert_eq!(
            ensure_normal_text_contrast(theme.bright.blue, theme.dim.grey),
            theme.bright.blue
        );
    }
}
