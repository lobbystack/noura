//! Dictation's settings, which live in the workspace file
//! `Settings/Dictation.md`, an object of the plugin's own type
//! `dictation-settings`: your custom words, how the shortcut starts and
//! stops a recording, and how the text goes in. The file holds them as
//! frontmatter, so you can edit it in any app; a value the plugin can't
//! read counts as its default.

use plugin_sdk::paste::Method;
use serde_json::{Value, json};

use crate::activation::Mode;

/// The type of the settings file.
pub(crate) const SETTINGS_TYPE: &str = "dictation-settings";

/// Where Dictation writes its settings file the first time you change one.
pub(crate) const SETTINGS_PATH: &str = "Settings/Dictation.md";

/// The settings type's schema, in the workspace format's declaration.
pub(crate) const SCHEMA: &str = r#"{"properties":[
  {"name":"words","kind":{"list":"string"},"default":[]},
  {"name":"activation","kind":{"enum":["hold-or-toggle","push-to-talk","toggle"]},"default":"hold-or-toggle"},
  {"name":"paste","kind":{"enum":["clipboard","typing"]},"default":"clipboard"}
]}"#;

/// How the text goes into the app you're typing in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PasteMethod {
    /// Through the clipboard, which Noura puts back afterwards.
    #[default]
    Clipboard,
    /// Key by key, for apps that refuse a paste.
    Typing,
}

impl PasteMethod {
    /// Each method, its value in the settings file and its label.
    pub(crate) const ALL: [(Self, &'static str, &'static str); 2] = [
        (Self::Clipboard, "clipboard", "Clipboard"),
        (Self::Typing, "typing", "Typing"),
    ];

    pub(crate) fn value(self) -> &'static str {
        match self {
            Self::Clipboard => "clipboard",
            Self::Typing => "typing",
        }
    }

    pub(crate) fn from_value(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|(_, known, _)| *known == value)
            .map(|(method, ..)| *method)
    }

    pub(crate) fn method(self) -> Method {
        match self {
            Self::Clipboard => Method::Clipboard,
            Self::Typing => Method::Typing,
        }
    }
}

/// What the settings file holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Settings {
    pub(crate) words: Vec<String>,
    pub(crate) activation: Mode,
    pub(crate) paste: PasteMethod,
}

impl Settings {
    /// The settings in an object's properties, a JSON object. Anything
    /// missing or unreadable keeps its default, so a hand edit gone wrong
    /// costs only that value.
    pub(crate) fn from_properties(properties: &str) -> Self {
        let value: Value = serde_json::from_str(properties).unwrap_or(Value::Null);
        let mut words: Vec<String> = Vec::new();
        for word in value["words"].as_array().into_iter().flatten() {
            if let Some(word) = word.as_str().and_then(crate::words::clean_word)
                && !words.contains(&word)
            {
                words.push(word);
            }
        }
        Self {
            words,
            activation: value["activation"]
                .as_str()
                .and_then(Mode::from_value)
                .unwrap_or_default(),
            paste: value["paste"]
                .as_str()
                .and_then(PasteMethod::from_value)
                .unwrap_or_default(),
        }
    }

    /// The properties to write, as a JSON object.
    pub(crate) fn to_properties(&self) -> String {
        json!({
            "words": self.words,
            "activation": self.activation.value(),
            "paste": self.paste.value(),
        })
        .to_string()
    }
}

/// A change you made on the settings page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    AddWord(String),
    RemoveWord(String),
    Activation(Mode),
    Paste(PasteMethod),
}

impl Change {
    /// `settings` with the change made, or why it can't be.
    pub(crate) fn apply(&self, settings: &Settings) -> Result<Settings, String> {
        let mut changed = settings.clone();
        match self {
            Self::AddWord(word) => {
                let Some(word) = crate::words::clean_word(word) else {
                    return Err(format!(
                        "Use 1 to {} characters.",
                        crate::words::LONGEST_WORD
                    ));
                };
                if changed.words.contains(&word) {
                    return Err(format!("\u{201c}{word}\u{201d} is already in the list."));
                }
                changed.words.push(word);
            }
            Self::RemoveWord(word) => changed.words.retain(|known| known != word),
            Self::Activation(mode) => changed.activation = *mode,
            Self::Paste(method) => changed.paste = *method,
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use workspace_format::{ObjectTypeRegistry, PropertySchema, WorkspaceObject};

    use super::{Change, PasteMethod, SCHEMA, SETTINGS_TYPE, Settings};
    use crate::activation::Mode;

    fn registry() -> ObjectTypeRegistry {
        let mut types = ObjectTypeRegistry::new();
        types
            .register(SETTINGS_TYPE, serde_json::from_str(SCHEMA).unwrap())
            .unwrap();
        types
    }

    fn object(properties: serde_json::Value) -> WorkspaceObject {
        WorkspaceObject {
            id: "dictation-settings_01j00000000000000000000000".into(),
            object_type: SETTINGS_TYPE.into(),
            title: "Dictation".into(),
            body: String::new(),
            relative_path: "Settings/Dictation.md".into(),
            revision: String::new(),
            created: None,
            updated: None,
            properties: serde_json::from_value(properties).unwrap(),
        }
    }

    #[test]
    fn the_schema_registers_and_fills_in_the_defaults() {
        let _: PropertySchema = serde_json::from_str(SCHEMA).unwrap();
        let types = registry();
        let mut empty = object(json!({}));
        types.normalize(&mut empty).unwrap();
        assert_eq!(
            json!(empty.properties),
            json!({"words": [], "activation": "hold-or-toggle", "paste": "clipboard"})
        );
        let defaults = Settings::from_properties(&json!(empty.properties).to_string());
        assert_eq!(defaults, Settings::default());
    }

    #[test]
    fn what_the_plugin_writes_fits_its_schema_and_reads_back() {
        let settings = Settings {
            words: vec!["Noura".into(), "ChargeBee".into()],
            activation: Mode::Toggle,
            paste: PasteMethod::Typing,
        };
        let properties: serde_json::Value =
            serde_json::from_str(&settings.to_properties()).unwrap();
        let mut written = object(properties.clone());
        registry().normalize(&mut written).unwrap();
        assert_eq!(json!(written.properties), properties);
        assert_eq!(
            Settings::from_properties(&settings.to_properties()),
            settings
        );
    }

    #[test]
    fn the_schema_refuses_an_unknown_mode() {
        let mut wrong = object(json!({"activation": "sometimes"}));
        let error = registry().normalize(&mut wrong).unwrap_err();
        assert_eq!(
            serde_json::to_value(error).unwrap(),
            json!({"code": "not_allowed", "property": "activation"})
        );
    }

    #[test]
    fn a_hand_edit_gone_wrong_keeps_the_rest() {
        let settings = Settings::from_properties(
            &json!({
                "words": ["Noura", 7, "  Noura ", "<b>", ""],
                "activation": "sometimes",
                "paste": "typing",
            })
            .to_string(),
        );
        assert_eq!(settings.words, ["Noura", "b"]);
        assert_eq!(settings.activation, Mode::HoldOrToggle);
        assert_eq!(settings.paste, PasteMethod::Typing);
        assert_eq!(Settings::from_properties("not json"), Settings::default());
    }

    #[test]
    fn a_word_goes_in_once_and_comes_out() {
        let settings = Settings::default();
        let added = Change::AddWord(" Noura ".into()).apply(&settings).unwrap();
        assert_eq!(added.words, ["Noura"]);
        assert_eq!(
            Change::AddWord("Noura".into()).apply(&added).unwrap_err(),
            "\u{201c}Noura\u{201d} is already in the list."
        );
        assert_eq!(
            Change::AddWord("  ".into()).apply(&added).unwrap_err(),
            "Use 1 to 50 characters."
        );
        let removed = Change::RemoveWord("Noura".into()).apply(&added).unwrap();
        assert!(removed.words.is_empty());
    }
}
