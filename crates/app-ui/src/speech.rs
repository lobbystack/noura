//! Settings > Speech, laid out as Handy's Models page: the models on this
//! computer, the ones for the language you speak with their grade, wait,
//! size and badges, download and delete; then the language, the plugins'
//! shortcuts and how Noura pastes. Onboarding recommends Parakeet Ultra.

use app_core::{App, SPEECH_LANGUAGES, SpeechModelRow, keys_from_label, keys_label};
use egui::{RichText, Ui, UiBuilder, accesskit::Role};

use crate::{
    ai::{alert, boxed, text_field},
    common::{badge, description, muted, primary_button},
};

/// What the Speech section remembers between frames.
#[derive(Default)]
pub(crate) struct SpeechSettingsView {
    /// Keys being typed for a shortcut, by `<plugin>/<shortcut>`.
    keys: std::collections::BTreeMap<String, String>,
    /// Why the last change didn't go in.
    problem: Option<String>,
}

fn size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1000.0)
    } else {
        format!("{mb:.0} MB")
    }
}

fn group(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label(label);
    ui.scope_builder(builder, |ui| {
        ui.label(RichText::new(label).strong());
        add(ui);
    });
    ui.add_space(16.0);
}

enum Action {
    Download(String),
    Cancel(String),
    Delete(String),
}

/// One model's row: its name, grade, wait, size, badges and button.
fn model_row(ui: &mut Ui, row: &SpeechModelRow, action: &mut Option<Action>) {
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label(&row.name);
    ui.scope_builder(builder, |ui| {
        boxed(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&row.name).strong());
                if let Some(grade) = row.grade {
                    badge(ui, grade);
                }
                if row.recommended {
                    badge(ui, "Recommended");
                }
                if row.streaming {
                    badge(ui, "Live");
                }
                if row.translation {
                    badge(ui, "Translates");
                }
            });
            if !row.description.is_empty() {
                muted(ui, &row.description);
            }
            let mut facts = vec![size(row.size)];
            if let Some(wait) = row.wait_seconds {
                facts.push(format!("{wait:.1} s wait"));
            }
            muted(ui, &facts.join(" · "));
            if let Some(problem) = &row.problem {
                alert(ui, problem);
            }
            ui.horizontal(|ui| match (row.downloaded, row.downloading) {
                (_, Some((received, total))) => {
                    let percent = (received * 100).checked_div(total).unwrap_or(0);
                    muted(ui, &format!("Downloading {percent}%"));
                    let cancel = ui.button("Cancel");
                    cancel.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            Role::Button,
                            true,
                            format!("Cancel {}", row.name),
                        )
                    });
                    if cancel.clicked() {
                        *action = Some(Action::Cancel(row.id.clone()));
                    }
                }
                (true, None) => {
                    let delete = ui.button("Delete");
                    delete.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            Role::Button,
                            true,
                            format!("Delete {}", row.name),
                        )
                    });
                    if delete.clicked() {
                        *action = Some(Action::Delete(row.id.clone()));
                    }
                }
                (false, None) => {
                    let download = primary_button(ui, "Download", true);
                    download.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            Role::Button,
                            true,
                            format!("Download {}", row.name),
                        )
                    });
                    if download.clicked() {
                        *action = Some(Action::Download(row.id.clone()));
                    }
                }
            });
        });
    });
}

/// The Speech section of Settings.
pub(crate) fn settings(ui: &mut Ui, app: &mut App, view: &mut SpeechSettingsView) {
    // Downloads move on outside any frame.
    let _ = app.speech_version();
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(500));
    if let Some(problem) = &view.problem {
        alert(ui, problem);
    }
    let mut action = None;
    let mut changed: Option<Result<(), app_core::AppError>> = None;

    if let Some(ultra) = app.speech_onboarding() {
        group(ui, "Get started", |ui| {
            description(
                ui,
                "Parakeet Ultra hears 25 languages and runs on this computer.",
            );
            model_row(ui, &ultra, &mut action);
            if ui.button("Not now").clicked() {
                changed = Some(app.finish_speech_onboarding());
            }
        });
    }

    let language = app.speech_language().unwrap_or_else(|| "en".to_owned());
    let name = SPEECH_LANGUAGES
        .iter()
        .find(|(code, _)| *code == language)
        .map_or(language.as_str(), |(_, name)| name);
    let mut chosen = None;
    egui::ComboBox::from_label("Language")
        .selected_text(name)
        .show_ui(ui, |ui| {
            for (code, name) in SPEECH_LANGUAGES {
                if ui.selectable_label(code == language, name).clicked() {
                    chosen = Some(code);
                }
            }
        });
    if let Some(code) = chosen {
        changed = Some(app.set_speech_language(Some(code)));
    }
    ui.add_space(16.0);

    let models = app.speech_models();
    let here: Vec<&SpeechModelRow> = models.iter().filter(|row| row.downloaded).collect();
    group(ui, "On this computer", |ui| {
        if here.is_empty() {
            muted(ui, "No models yet");
        }
        for row in &here {
            model_row(ui, row, &mut action);
        }
    });
    let mut yours: Vec<&SpeechModelRow> = models
        .iter()
        .filter(|row| !row.downloaded && row.curated && row.grade.is_some())
        .collect();
    yours.sort_by_key(|row| (!row.recommended, row.grade));
    group(ui, "For your language", |ui| {
        if yours.is_empty() {
            muted(ui, "No other models hear this language well");
        }
        for row in yours {
            model_row(ui, row, &mut action);
        }
    });

    let shortcuts = app.global_shortcuts();
    if !shortcuts.is_empty() {
        group(ui, "Shortcuts", |ui| {
            for shortcut in shortcuts {
                let key = format!("{}/{}", shortcut.plugin, shortcut.id);
                let typed = view
                    .keys
                    .entry(key.clone())
                    .or_insert_with(|| keys_label(&shortcut.keys));
                ui.label(&shortcut.title);
                text_field(ui, &format!("{} keys", shortcut.title), typed, false);
                ui.horizontal(|ui| {
                    let saved = keys_from_label(typed).as_deref() == Some(shortcut.keys.as_str());
                    let ready = !saved && !typed.trim().is_empty();
                    if primary_button(ui, "Save", ready).clicked() {
                        let saved = app.set_shortcut_keys(
                            &shortcut.plugin,
                            &shortcut.id,
                            Some(typed.trim()),
                        );
                        // The field shows the keys as this system writes
                        // them, whichever way you typed them.
                        if saved.is_ok()
                            && let Some(keys) = keys_from_label(typed)
                        {
                            *typed = keys_label(&keys);
                        }
                        changed = Some(saved);
                    }
                    if shortcut.keys != shortcut.declared && ui.button("Reset").clicked() {
                        changed = Some(app.set_shortcut_keys(&shortcut.plugin, &shortcut.id, None));
                        *typed = keys_label(&shortcut.declared);
                    }
                });
            }
        });
    }

    if let Some(action) = action {
        changed = Some(match action {
            Action::Download(id) => app.download_speech_model(&id),
            Action::Cancel(id) => {
                app.cancel_speech_download(&id);
                Ok(())
            }
            Action::Delete(id) => app.delete_speech_model(&id),
        });
    }
    if let Some(changed) = changed {
        view.problem = changed.err().map(|error| error.message);
    }
}
