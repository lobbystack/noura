//! What the kernel draws for web search: the Web search section of
//! Settings, where you keep the Brave Search key in the credential store
//! and allow web search for the open workspace, and the prompt that asks
//! before the AI may search the web there. Plugins draw neither, so no
//! plugin can show a prompt that grants web consent or read the key.

use app_core::App;
use egui::{Id, RichText, Ui, UiBuilder, accesskit::Role};
use zeroize::Zeroizing;

use crate::{
    ai::{alert, boxed, text_field},
    common::{description, primary_button},
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
    description(
        ui,
        "Uses your own Brave Search key, kept in this computer's credential store.",
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
            description(ui, if row.has_key { "Saved" } else { "No key" });
            let label = if row.has_key { "New key" } else { "Key" };
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
                description(ui, "Open a workspace first.");
                return;
            };
            if row.consented {
                description(ui, &format!("Allowed in {workspace}"));
                if ui.button("Turn off").clicked() {
                    changed = Some(app.revoke_web_consent());
                }
            } else {
                description(ui, &format!("Not allowed in {workspace}"));
                if ui.button("Allow").clicked() {
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
    let modal = crate::common::dialog(ui.ctx(), Id::unique("web-consent")).show(ui.ctx(), |ui| {
        let builder = UiBuilder::new()
            .accessibility_role(Role::AlertDialog)
            .accessibility_label(&title);
        ui.scope_builder(builder, |ui| {
            ui.set_width(440.0);
            crate::common::dialog_title(ui, &title);
            ui.add_space(4.0);
            ui.label(format!(
                "In {}, search queries go to {}, and page addresses go to the sites that host them.",
                prompt.workspace, prompt.provider_name
            ));
            ui.label(format!(
                "Plugins you let use AI can search the web in {} too.",
                prompt.workspace
            ));
            description(
                ui,
                "Noura won't ask before each search. Revoke it in Settings > AI.",
            );
            let pressed = crate::common::dialog_footer(
                ui,
                vec![
                    crate::common::Btn::new("Deny"),
                    crate::common::Btn::new("Allow").variant(crate::common::Variant::Default),
                ],
            );
            if opened {
                pressed[0].request_focus();
            }
            if pressed[0].clicked() {
                answer = Some(false);
            }
            if pressed[1].clicked() {
                answer = Some(true);
            }
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
