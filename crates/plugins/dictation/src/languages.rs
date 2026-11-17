//! English names for the language codes a model hears, for the Language
//! submenu, as Handy's `src-tauri/src/languages.rs` names them: ISO 639's
//! names through `isolang`, with the names people use where ISO's differ.

use isolang::Language;

/// Names people use that differ from ISO 639's, such as "Modern Greek".
const OVERRIDES: &[(&str, &str)] = &[
    ("el", "Greek"),
    ("ht", "Haitian Creole"),
    ("nb", "Norwegian"),
    ("no", "Norwegian"),
    ("pa", "Punjabi"),
    ("ps", "Pashto"),
    ("yue", "Cantonese"),
];

/// The English name of `code`, an ISO 639-1 or 639-3 code, with any
/// region after it, as `en-US`. The code itself when it names nothing.
pub(crate) fn name(code: &str) -> String {
    let identity = code
        .split(['-', '_'])
        .next()
        .unwrap_or(code)
        .to_ascii_lowercase();
    if let Some((_, name)) = OVERRIDES.iter().find(|(known, _)| *known == identity) {
        return (*name).to_owned();
    }
    let language = match identity.len() {
        2 => Language::from_639_1(&identity),
        3 => Language::from_639_3(&identity),
        _ => None,
    };
    language.map_or(identity, |language| {
        // ISO names some languages with a qualifier after a semicolon or a
        // parenthesis, as "Spanish; Castilian".
        language
            .to_name()
            .split([';', '('])
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::name;

    #[test]
    fn codes_get_the_names_people_use() {
        assert_eq!(name("en"), "English");
        assert_eq!(name("en-US"), "English");
        assert_eq!(name("fr"), "French");
        assert_eq!(name("zh"), "Chinese");
        assert_eq!(name("es"), "Spanish");
        assert_eq!(name("el"), "Greek");
        assert_eq!(name("no"), "Norwegian");
        assert_eq!(name("yue"), "Cantonese");
        assert_eq!(name("xx"), "xx");
    }
}
