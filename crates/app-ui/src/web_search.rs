//! What the kernel draws for web search: the Web search section of
//! Settings, where you keep the Brave Search key in the credential store.
//! Plugins don't draw it, so no plugin can read the key.

use app_core::App;
use egui::{RichText, Ui, UiBuilder, accesskit::Role};
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
    if let Some(changed) = changed {
        view.problem = changed.err().map(|error| error.message);
    }
}
