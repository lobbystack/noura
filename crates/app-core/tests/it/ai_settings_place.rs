//! Where the AI settings live: on the page in Plugins of each plugin that
//! asks the AI providers, never as a section of their own. "Open AI
//! settings" and Search settings reach them whichever of those plugins is
//! on, so Transcription keeps them in reach while the AI plugin is off.

use crate::support;

use app_core::{SettingsGroup, settings_section};
use support::{Fixture, ai, plugins, plugins::PluginFolder};

const TRANSCRIPTION: &str = include_str!("../../../plugins/transcription/plugin.toml");

fn transcription() -> Option<PluginFolder> {
    plugins::built("transcription_plugin")
        .map(|component| PluginFolder::new(TRANSCRIPTION, "transcription_plugin.wasm", &component))
}

/// The titles Search settings shows for `query`, by group.
fn found(app: &mut app_core::App, query: &str) -> Vec<(SettingsGroup, Vec<String>)> {
    app.set_settings_query(query);
    app.matching_settings_sections()
        .into_iter()
        .map(|(group, sections)| {
            (
                group,
                sections
                    .into_iter()
                    .map(|section| section.title.clone())
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn settings_lists_no_ai_section() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    app.open_settings();
    let titles: Vec<String> = app
        .settings_sections()
        .into_iter()
        .flat_map(|(_, sections)| sections)
        .map(|section| section.title.clone())
        .collect();
    assert!(!titles.contains(&"AI".to_owned()), "{titles:?}");
    // With no plugin that asks the providers, the AI settings open the
    // Plugins list, and search finds no AI page.
    app.select_settings_section(settings_section::AI);
    let settings = app.settings().unwrap();
    assert_eq!(settings.section, settings_section::PLUGINS);
    assert_eq!(settings.plugin_details, None);
    assert!(
        found(&mut app, "api keys")
            .iter()
            .all(|(_, titles)| !titles.contains(&"AI".to_owned()))
    );
}

#[test]
fn the_ai_settings_open_on_the_ai_plugins_page_even_while_it_is_off() {
    let Some(ai_plugin) = ai::folder() else {
        return;
    };
    let Some(transcription) = transcription() else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    plugins::install(&mut app, ai_plugin.path());
    plugins::install_and_enable(&mut app, transcription.path(), "transcription");
    assert!(
        !app.plugins()
            .iter()
            .any(|row| row.id == "ai" && row.enabled)
    );

    app.open_settings_at(settings_section::AI);
    let settings = app.settings().unwrap();
    assert_eq!(settings.section, settings_section::PLUGINS);
    assert_eq!(settings.plugin_details.as_deref(), Some("ai"));

    // Web search lives on the same page, and another section closes it.
    app.select_settings_section(settings_section::GENERAL);
    app.select_settings_section(settings_section::WEB_SEARCH);
    assert_eq!(
        app.settings().unwrap().plugin_details.as_deref(),
        Some("ai")
    );
}

#[test]
fn with_the_ai_plugin_gone_transcription_holds_the_ai_settings() {
    let Some(transcription) = transcription() else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    plugins::install_and_enable(&mut app, transcription.path(), "transcription");
    let row = app
        .plugins()
        .into_iter()
        .find(|row| row.id == "transcription")
        .unwrap();
    assert!(row.uses_ai());
    assert_eq!(app.ai_settings_plugin().as_deref(), Some("transcription"));

    app.open_settings_at(settings_section::AI);
    assert_eq!(
        app.settings().unwrap().plugin_details.as_deref(),
        Some("transcription")
    );
    // Search settings finds them as AI, after Plugins.
    let workspace = found(&mut app, "API keys")
        .into_iter()
        .find(|(group, _)| *group == SettingsGroup::Workspace)
        .map(|(_, titles)| titles)
        .unwrap_or_default();
    assert_eq!(workspace, ["AI"]);
    app.set_settings_query("plug");
    app.set_settings_query("anthropic");
    app.select_settings_section(settings_section::AI);
    assert_eq!(
        app.settings().unwrap().plugin_details.as_deref(),
        Some("transcription")
    );
}
