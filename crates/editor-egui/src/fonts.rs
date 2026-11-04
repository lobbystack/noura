//! Fonts for the editor's text. The embedding app decides which fonts the
//! editor shapes with and passes them to
//! [`register_font_system`](crate::register_font_system).

use glyphon::fontdb;

/// A font database holding the fonts egui bundles: Ubuntu Light for
/// proportional text and Hack for monospace text. Their licenses are listed
/// in the repository's third-party notices.
pub fn default_font_database() -> fontdb::Database {
    let mut db = fontdb::Database::new();
    let definitions = egui::FontDefinitions::default();
    let mut family_of = |name: &str| -> Option<String> {
        let data = definitions.font_data.get(name)?;
        let bytes: &[u8] = data.font.as_ref().as_ref();
        let ids = db.load_font_source(fontdb::Source::Binary(std::sync::Arc::new(bytes.to_vec())));
        let id = *ids.first()?;
        db.face(id)
            .and_then(|face| face.families.first().map(|(family, _)| family.clone()))
    };

    let sans = family_of("Ubuntu-Light");
    let mono = family_of("Hack");
    for name in definitions.font_data.keys() {
        if name != "Ubuntu-Light" && name != "Hack" {
            family_of(name);
        }
    }

    if let Some(sans) = sans {
        db.set_sans_serif_family(sans.clone());
        db.set_serif_family(sans);
    }
    if let Some(mono) = mono {
        db.set_monospace_family(mono);
    }
    db
}

/// Load the faces in `faces`, such as the interface font's subsets and
/// styles, and make the family of the first the one `db` shapes
/// proportional text with.
pub fn prefer_sans(db: &mut fontdb::Database, faces: impl IntoIterator<Item = &'static [u8]>) {
    let mut family = None;
    for bytes in faces {
        let ids = db.load_font_source(fontdb::Source::Binary(std::sync::Arc::new(bytes)));
        if family.is_none() {
            family = ids
                .first()
                .and_then(|id| db.face(*id))
                .and_then(|face| face.families.first().map(|(family, _)| family.clone()));
        }
    }
    if let Some(family) = family {
        db.set_sans_serif_family(family.clone());
        db.set_serif_family(family);
    }
}
