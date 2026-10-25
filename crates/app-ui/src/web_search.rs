//! What the kernel draws for web search: the Web search section of
//! Settings, where you keep the Brave Search key in the credential store
//! and allow web search for the open workspace, and the prompt that asks
//! before the AI may search the web there. Plugins draw neither, so no
//! plugin can show a prompt that grants web consent or read the key.

use app_core::App;
use egui::{Button, Id, Modal, RichText, Ui, UiBuilder, accesskit::Role};
use zeroize::Zeroizing;

use crate::{
    ai::{alert, boxed, text_field},
    common::{muted, primary_button},
    theme::TITLE_SIZE,
};

/// What the Web search section remembers between frames.
#[derive(Default)]
pub(crate) struct WebSettingsView {
    /// The key being typed. It leaves memory once saved.
    key: Zeroizing<String>,
    /// Why the last change didn't go in.
    problem: Option<String>,
}

/// The Web search section of Settings.
pub(crate) fn settings(ui: &mut Ui, app: &mut App, view: &mut WebSettingsView) {
    if let Some(problem) = app.web_search_problem() {
        alert(ui, &problem);
        return;
    }
    muted(
        ui,
        "The AI can search the web with Brave Search and read the pages it finds. Add your own \
         Brave Search API key. Noura keeps it in this computer's credential store.",
    );
    ui.add_space(8.0);
    if let Some(problem) = &view.problem {
        alert(ui, problem);
    }
    let row = app.web_search();
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label("Brave Search key");
    let mut changed = None;
    ui.scope_builder(builder, |ui| {
        boxed(ui, |ui| {
            ui.label(RichText::new("Brave Search key").strong());
            muted(
                ui,
                if row.has_key {
                    "Your key is saved."
                } else {
                    "No key yet."
                },
            );
            let label = if row.has_key {
                "New key (leave empty to keep the saved one)"
            } else {
                "Key"
            };
            text_field(ui, label, &mut view.key, true);
            ui.horizontal(|ui| {
                let ready = !view.key.trim().is_empty();
                if primary_button(ui, "Save key", ready).clicked() {
                    changed = Some(app.save_web_search_key(&view.key));
                    view.key = Zeroizing::new(String::new());
                }
                if row.has_key && ui.button("Remove key").clicked() {
                    changed = Some(app.remove_web_search_key());
                }
            });
        });
    });
    ui.add_space(4.0);
    let workspace = app.workspace().map(|workspace| workspace.name());
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label("This workspace");
    ui.scope_builder(builder, |ui| {
        boxed(ui, |ui| {
            ui.label(RichText::new("This workspace").strong());
            let Some(workspace) = &workspace else {
                muted(ui, "Open a workspace to allow web search there.");
                return;
            };
            if row.consented {
                muted(ui, &format!("The AI can search the web in {workspace}."));
                if ui.button("Turn off for this workspace").clicked() {
                    changed = Some(app.revoke_web_consent());
                }
            } else {
                muted(
                    ui,
                    &format!("The AI can't search the web in {workspace} yet."),
                );
                if ui.button("Allow for this workspace").clicked() {
                    changed = Some(app.ask_web_consent());
                }
            }
        });
    });
    if let Some(changed) = changed {
        view.problem = changed.err().map(|error| error.message);
    }
}

/// What the web consent prompt remembers between frames.
#[derive(Default)]
pub(crate) struct WebConsentView {
    /// The prompt showed last frame.
    asking: bool,
}

/// The prompt that asks whether the AI may search the web and read pages
/// in the open workspace. It starts on Deny. Escape or a click outside it
/// counts as Deny.
pub(crate) fn consent_prompt(ui: &mut Ui, app: &mut App, view: &mut WebConsentView) {
    let Some(prompt) = app.web_consent_prompt() else {
        view.asking = false;
        return;
    };
    let opened = !view.asking;
    view.asking = true;
    let title = format!("Let the AI search the web with {}?", prompt.provider_name);
    let mut answer = None;
    let modal = Modal::new(Id::unique("web-consent")).show(ui.ctx(), |ui| {
        let builder = UiBuilder::new()
            .accessibility_role(Role::AlertDialog)
            .accessibility_label(&title);
        ui.scope_builder(builder, |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new(&title).size(TITLE_SIZE).strong());
            ui.add_space(4.0);
            ui.label(format!(
                "When a chat in {} uses the web, the search queries the AI writes go to {}, and \
                 the addresses of the pages it reads go to the sites that host them. Both leave \
                 this computer.",
                prompt.workspace, prompt.provider_name
            ));
            muted(
                ui,
                "Noura doesn't ask again before each search. To take this back, open Settings, \
                 then Web search.",
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let deny = ui.add(Button::new("Deny"));
                if opened {
                    deny.request_focus();
                }
                if deny.clicked() {
                    answer = Some(false);
                }
                if primary_button(ui, "Allow", true).clicked() {
                    answer = Some(true);
                }
            });
        });
    });
    if answer.is_none() && modal.should_close() {
        answer = Some(false);
    }
    match answer {
        Some(true) => app.allow_web_consent(),
        Some(false) => app.deny_web_consent(),
        None => {}
    }
}
