//! Dictation's entries in Noura's menu bar or tray menu, laid out as
//! Handy's tray menu (`src-tauri/src/tray.rs`): Copy Last Dictation, then
//! a submenu named after the model, holding the downloaded models, and a
//! Language submenu when the model hears more than one language. While you
//! dictate, the menu holds Cancel Dictation and Copy Last Dictation only.
//! Noura adds Settings… and Quit below every plugin's entries.

use plugin_sdk::{
    menu_bar::{Entry, EntryKind},
    speech::{Model, Preferences},
};

pub(crate) const COPY_LAST: &str = "copy-last";
pub(crate) const CANCEL: &str = "cancel";
pub(crate) const MODEL_PREFIX: &str = "model:";
pub(crate) const LANGUAGE_PREFIX: &str = "language:";
/// The language entry that follows Settings, Speech.
pub(crate) const DEFAULT_LANGUAGE: &str = "language:default";

const MODEL_MENU: &str = "model";
const LANGUAGE_MENU: &str = "language";
const OTHER_LANGUAGES: &str = "other-languages";

/// More languages than this, and all but the chosen one go in Other
/// Languages, as Handy does past the languages you speak.
const MOST_LANGUAGES_SHOWN: usize = 12;

/// What the menu shows.
pub(crate) struct Inputs<'a> {
    /// Recording, or waiting for the text.
    pub(crate) busy: bool,
    pub(crate) has_last: bool,
    pub(crate) models: &'a [Model],
    pub(crate) preferences: Option<&'a Preferences>,
}

/// The model a dictation uses: the one you chose if it's downloaded,
/// otherwise Noura's recommendation, otherwise the first downloaded one.
pub(crate) fn active_model<'a>(
    models: &'a [Model],
    preferences: Option<&Preferences>,
) -> Option<&'a Model> {
    let downloaded = || models.iter().filter(|model| model.downloaded);
    preferences
        .and_then(|preferences| preferences.model.as_deref())
        .and_then(|chosen| downloaded().find(|model| model.id == chosen))
        .or_else(|| downloaded().find(|model| model.recommended))
        .or_else(|| downloaded().next())
}

fn item(
    id: &str,
    label: &str,
    enabled: bool,
    checked: Option<bool>,
    parent: Option<&str>,
) -> Entry {
    Entry {
        id: id.to_owned(),
        kind: EntryKind::Item,
        label: label.to_owned(),
        enabled,
        checked,
        parent: parent.map(str::to_owned),
    }
}

fn submenu(id: &str, label: &str, enabled: bool, parent: Option<&str>) -> Entry {
    Entry {
        kind: EntryKind::Submenu,
        ..item(id, label, enabled, None, parent)
    }
}

fn separator(id: &str, parent: Option<&str>) -> Entry {
    Entry {
        kind: EntryKind::Separator,
        ..item(id, "", true, None, parent)
    }
}

/// Whether two lists of entries show the same menu. The generated types
/// don't compare.
pub(crate) fn same(a: &[Entry], b: &[Entry]) -> bool {
    let key = |entry: &Entry| {
        let kind = match entry.kind {
            EntryKind::Item => 0,
            EntryKind::Submenu => 1,
            EntryKind::Separator => 2,
        };
        (
            entry.id.clone(),
            kind,
            entry.label.clone(),
            entry.enabled,
            entry.checked,
            entry.parent.clone(),
        )
    };
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| key(a) == key(b))
}

/// The entries to show.
pub(crate) fn entries(inputs: &Inputs<'_>) -> Vec<Entry> {
    let copy_last = item(
        COPY_LAST,
        "Copy Last Dictation",
        inputs.has_last,
        None,
        None,
    );
    if inputs.busy {
        return vec![
            item(CANCEL, "Cancel Dictation", true, None, None),
            copy_last,
        ];
    }
    let mut entries = vec![copy_last, separator("line", None)];

    let active = active_model(inputs.models, inputs.preferences);
    let downloaded: Vec<&Model> = inputs
        .models
        .iter()
        .filter(|model| model.downloaded)
        .collect();
    entries.push(submenu(
        MODEL_MENU,
        active.map_or("Model", |model| model.name.as_str()),
        !downloaded.is_empty(),
        None,
    ));
    for model in &downloaded {
        let chosen = active.is_some_and(|active| active.id == model.id);
        entries.push(item(
            &format!("{MODEL_PREFIX}{}", model.id),
            &model.name,
            true,
            Some(chosen),
            Some(MODEL_MENU),
        ));
    }

    if let Some(model) = active {
        language_entries(&mut entries, model, inputs.preferences);
    }
    entries
}

fn language_entries(entries: &mut Vec<Entry>, model: &Model, preferences: Option<&Preferences>) {
    let mut codes: Vec<&str> = Vec::new();
    for code in &model.languages {
        if !codes.contains(&code.as_str()) {
            codes.push(code);
        }
    }
    if codes.len() < 2 {
        return;
    }
    codes.sort_by_key(|code| crate::languages::name(code));
    let chosen = preferences
        .and_then(|preferences| preferences.language.as_deref())
        .filter(|chosen| codes.contains(chosen));
    let label = match chosen {
        Some(code) => format!("Language ({})", crate::languages::name(code)),
        None => "Language (Default)".to_owned(),
    };
    entries.push(submenu(LANGUAGE_MENU, &label, true, None));
    entries.push(item(
        DEFAULT_LANGUAGE,
        "Default",
        true,
        Some(chosen.is_none()),
        Some(LANGUAGE_MENU),
    ));
    entries.push(separator("language-line", Some(LANGUAGE_MENU)));
    let language = |code: &str, parent: &str| {
        item(
            &format!("{LANGUAGE_PREFIX}{code}"),
            &crate::languages::name(code),
            true,
            Some(chosen == Some(code)),
            Some(parent),
        )
    };
    if codes.len() <= MOST_LANGUAGES_SHOWN {
        for code in &codes {
            entries.push(language(code, LANGUAGE_MENU));
        }
        return;
    }
    if let Some(code) = chosen {
        entries.push(language(code, LANGUAGE_MENU));
    }
    entries.push(submenu(
        OTHER_LANGUAGES,
        "Other Languages",
        true,
        Some(LANGUAGE_MENU),
    ));
    for code in codes.iter().filter(|code| Some(**code) != chosen) {
        entries.push(language(code, OTHER_LANGUAGES));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use plugin_sdk::{
        menu_bar::{Entry, EntryKind},
        speech::{Model, Preferences},
    };

    use super::{Inputs, active_model, entries};

    pub(crate) fn model(id: &str, name: &str, languages: &[&str], downloaded: bool) -> Model {
        Model {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            languages: languages.iter().map(|code| (*code).to_owned()).collect(),
            size: 1,
            streaming: false,
            translation: false,
            downloaded,
            recommended: false,
        }
    }

    pub(crate) fn preferences(model: Option<&str>, language: Option<&str>) -> Preferences {
        Preferences {
            model: model.map(str::to_owned),
            microphone: None,
            system_audio: false,
            language: language.map(str::to_owned),
        }
    }

    /// Each entry as `parent/label` with `[x]` or `[ ]` for a check mark,
    /// `>` for a submenu, `-` for a line and `(off)` when disabled.
    fn shown(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| {
                let mut line = match &entry.parent {
                    Some(parent) => format!("{parent}/"),
                    None => String::new(),
                };
                match entry.kind {
                    EntryKind::Separator => line.push('-'),
                    EntryKind::Submenu => line.push_str(&format!("{} >", entry.label)),
                    EntryKind::Item => {
                        match entry.checked {
                            Some(true) => line.push_str("[x] "),
                            Some(false) => line.push_str("[ ] "),
                            None => {}
                        }
                        line.push_str(&entry.label);
                    }
                }
                if !entry.enabled {
                    line.push_str(" (off)");
                }
                line
            })
            .collect()
    }

    #[test]
    fn the_menu_holds_copy_last_the_models_and_the_languages() {
        let mut ultra = model(
            "parakeet-ultra",
            "Parakeet Ultra",
            &["en", "fr", "de"],
            true,
        );
        ultra.recommended = true;
        let models = [
            ultra,
            model("whisper-tiny", "Whisper Tiny", &["en"], true),
            model("whisper-large", "Whisper Large", &["en"], false),
        ];
        let chosen = preferences(None, Some("fr"));
        let menu = entries(&Inputs {
            busy: false,
            has_last: true,
            models: &models,
            preferences: Some(&chosen),
        });
        assert_eq!(
            shown(&menu),
            [
                "Copy Last Dictation",
                "-",
                "Parakeet Ultra >",
                "model/[x] Parakeet Ultra",
                "model/[ ] Whisper Tiny",
                "Language (French) >",
                "language/[ ] Default",
                "language/-",
                "language/[ ] English",
                "language/[x] French",
                "language/[ ] German",
            ]
        );
        assert_eq!(menu[3].id, "model:parakeet-ultra");
        assert_eq!(menu[9].id, "language:fr");
        assert_eq!(menu[6].id, "language:default");
    }

    #[test]
    fn copy_last_dictation_is_off_until_there_is_one() {
        let menu = entries(&Inputs {
            busy: false,
            has_last: false,
            models: &[],
            preferences: None,
        });
        assert_eq!(
            shown(&menu),
            ["Copy Last Dictation (off)", "-", "Model > (off)"]
        );
    }

    #[test]
    fn while_you_dictate_the_menu_offers_cancel() {
        let models = [model("whisper-tiny", "Whisper Tiny", &["en", "fr"], true)];
        let menu = entries(&Inputs {
            busy: true,
            has_last: true,
            models: &models,
            preferences: None,
        });
        assert_eq!(shown(&menu), ["Cancel Dictation", "Copy Last Dictation"]);
    }

    #[test]
    fn a_model_for_one_language_has_no_language_menu() {
        let models = [model("whisper-tiny-en", "Whisper Tiny", &["en"], true)];
        let menu = entries(&Inputs {
            busy: false,
            has_last: true,
            models: &models,
            preferences: None,
        });
        assert!(!shown(&menu).iter().any(|line| line.starts_with("Language")));
    }

    #[test]
    fn many_languages_go_in_other_languages() {
        let codes = [
            "en", "fr", "de", "es", "it", "pt", "nl", "pl", "cs", "sv", "da", "fi", "el",
        ];
        let models = [model("whisper", "Whisper", &codes, true)];
        let chosen = preferences(None, Some("de"));
        let menu = entries(&Inputs {
            busy: false,
            has_last: true,
            models: &models,
            preferences: Some(&chosen),
        });
        let lines = shown(&menu);
        assert!(
            lines.contains(&"language/[x] German".to_owned()),
            "{lines:?}"
        );
        assert!(lines.contains(&"language/Other Languages >".to_owned()));
        assert!(lines.contains(&"other-languages/[ ] Greek".to_owned()));
        assert!(!lines.contains(&"other-languages/[ ] German".to_owned()));
    }

    #[test]
    fn the_chosen_model_wins_then_the_recommended_one() {
        let mut recommended = model("b", "B", &["en"], true);
        recommended.recommended = true;
        let models = [
            model("a", "A", &["en"], true),
            recommended,
            model("c", "C", &["en"], false),
        ];
        let pick = |chosen: Option<&str>| {
            let preferences = preferences(chosen, None);
            active_model(&models, Some(&preferences)).map(|model| model.id.clone())
        };
        assert_eq!(pick(Some("a")).as_deref(), Some("a"));
        assert_eq!(pick(None).as_deref(), Some("b"));
        // A chosen model that isn't downloaded falls back.
        assert_eq!(pick(Some("c")).as_deref(), Some("b"));
        assert_eq!(
            active_model(&models[..1], None).map(|model| model.id.as_str()),
            Some("a")
        );
    }
}
