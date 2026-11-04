//! The interface font: Public Sans, the legacy app's, bundled with Noura
//! as the subsets the legacy app shipped (Latin, Latin Extended and
//! Vietnamese), each a variable font with a weight axis. egui's own fonts
//! stay behind it for the glyphs it lacks. See
//! docs/provenance/public-sans.md.

use std::sync::{Arc, OnceLock};

use egui::{FontData, FontDefinitions, FontFamily};

/// A font behind Public Sans and egui's own, such as the system's
/// interface font, for symbols Public Sans lacks like ⌘ and ⇧.
static FALLBACK: OnceLock<Arc<FontData>> = OnceLock::new();

/// Draw the glyphs Public Sans and egui's fonts lack, such as the
/// keyboard's modifier symbols, from the font in `bytes`. The first call
/// wins.
pub fn set_fallback(bytes: Vec<u8>) {
    let _ = FALLBACK.set(Arc::new(FontData::from_owned(bytes)));
}

/// The upright subsets, by the key egui knows each by.
const UPRIGHT: [(&str, &[u8]); 3] = [
    (
        "public-sans-latin",
        include_bytes!("../assets/fonts/PublicSans-latin.ttf"),
    ),
    (
        "public-sans-latin-ext",
        include_bytes!("../assets/fonts/PublicSans-latin-ext.ttf"),
    ),
    (
        "public-sans-vietnamese",
        include_bytes!("../assets/fonts/PublicSans-vietnamese.ttf"),
    ),
];

/// The italic subsets, which only the note editor draws with: egui slants
/// the upright font.
const ITALIC: [&[u8]; 3] = [
    include_bytes!("../assets/fonts/PublicSans-latin-Italic.ttf"),
    include_bytes!("../assets/fonts/PublicSans-latin-ext-Italic.ttf"),
    include_bytes!("../assets/fonts/PublicSans-vietnamese-Italic.ttf"),
];

/// egui's fonts with Public Sans in front of its proportional family, and
/// the medium and semibold weights as families of their own.
pub fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let keys: Vec<&str> = UPRIGHT.iter().map(|(key, _)| *key).collect();
    for (key, bytes) in UPRIGHT {
        // The variable font's default instance is its thinnest weight, so
        // the regular face asks for 400.
        let mut tweak = egui::FontTweak::default();
        tweak.coords.push("wght", 400.0);
        fonts.font_data.insert(
            key.to_owned(),
            Arc::new(FontData::from_static(bytes).tweak(tweak)),
        );
    }
    if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
        for (index, key) in keys.iter().enumerate() {
            family.insert(index, (*key).to_owned());
        }
    }
    if let Some(fallback) = FALLBACK.get() {
        fonts
            .font_data
            .insert("fallback".to_owned(), fallback.clone());
        if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
            family.push("fallback".to_owned());
        }
    }
    crate::theme::add_weights(&mut fonts, &keys);
    fonts
}

/// Every Public Sans face, upright then italic, for the note editor's
/// font database.
pub fn editor_faces() -> impl Iterator<Item = &'static [u8]> {
    UPRIGHT.into_iter().map(|(_, bytes)| bytes).chain(ITALIC)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_face_parses_with_a_weight_axis_and_the_latin_letters() {
        for bytes in editor_faces() {
            let data = FontData::from_static(bytes);
            let axes = data.variation_axes();
            assert!(
                axes.iter().any(|axis| axis.tag.to_string() == "wght"),
                "{axes:?}"
            );
        }
        let fonts = definitions();
        assert_eq!(
            fonts.families[&FontFamily::Proportional][0],
            "public-sans-latin"
        );
        assert!(
            fonts
                .families
                .contains_key(&FontFamily::Name("semibold".into()))
        );
    }
}
