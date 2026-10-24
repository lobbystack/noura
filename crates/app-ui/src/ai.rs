//! What the kernel draws for AI: the AI providers section of Settings,
//! where you add providers, keep their keys in the credential store and
//! consent for the open workspace, and the prompt that asks before a
//! provider receives the workspace's content. Plugins draw neither, so no
//! plugin can show a prompt that grants consent or read a key.

use app_core::{AiConsentPrompt, AiProviderInput, AiProviderRow, App};
use egui::{
    Button, ComboBox, Frame, Id, Margin, Modal, RichText, Stroke, TextEdit, Ui, UiBuilder,
    accesskit::Role,
};
use zeroize::Zeroizing;

use crate::{
    common::{muted, primary_button},
    theme::{Palette, TITLE_SIZE, radius},
};

/// What the AI providers section remembers between frames.
#[derive(Default)]
pub(crate) struct AiSettingsView {
    /// The provider form, while you add or edit one.
    form: Option<ProviderForm>,
    /// Why the last change to a provider didn't go in.
    problem: Option<String>,
}

/// A change to one provider, by its ID.
enum Change {
    Allow(String),
    Revoke(String),
    RemoveKey(String),
    Remove(String),
}

impl Change {
    fn apply(self, app: &mut App) -> Result<(), app_core::AppError> {
        match self {
            Self::Allow(id) => app.grant_ai_consent(&id),
            Self::Revoke(id) => app.revoke_ai_consent(&id),
            Self::RemoveKey(id) => app.remove_ai_provider_key(&id),
            Self::Remove(id) => app.remove_ai_provider(&id),
        }
    }
}

/// A provider being added or edited.
struct ProviderForm {
    /// The provider being edited, or none for a new one.
    id: Option<String>,
    name: String,
    kind: String,
    model: String,
    endpoint: String,
    /// A new key. It leaves memory when the form closes.
    key: Zeroizing<String>,
    enabled: bool,
    /// Whether the provider has a key already.
    has_key: bool,
    /// Why the last save didn't go in.
    problem: Option<String>,
}

impl ProviderForm {
    fn new(kind: &str) -> Self {
        Self {
            id: None,
            name: String::new(),
            kind: kind.to_owned(),
            model: String::new(),
            endpoint: String::new(),
            key: Zeroizing::new(String::new()),
            enabled: true,
            has_key: false,
            problem: None,
        }
    }

    fn of(row: &AiProviderRow) -> Self {
        Self {
            id: Some(row.id.clone()),
            name: row.name.clone(),
            kind: row.kind.clone(),
            model: row.model.clone(),
            endpoint: row.endpoint.clone().unwrap_or_default(),
            key: Zeroizing::new(String::new()),
            enabled: row.enabled,
            has_key: row.has_key,
            problem: None,
        }
    }
}

/// The name Settings shows for a provider kind.
fn kind_name<'a>(app: &App, kind: &'a str) -> &'a str {
    app.ai_provider_kinds()
        .iter()
        .find(|(known, _)| *known == kind)
        .map_or(kind, |(_, name)| name)
}

/// The AI providers section of Settings.
pub(crate) fn settings(ui: &mut Ui, app: &mut App, view: &mut AiSettingsView) {
    if let Some(problem) = app.ai_settings_problem() {
        alert(ui, &problem);
        return;
    }
    muted(
        ui,
        "Providers and their settings stay on this computer, and their keys stay in its \
         credential store. A chat sends this workspace's content to a provider only once you \
         allow it here or when Noura asks.",
    );
    ui.add_space(8.0);
    let rows = app.ai_providers();
    let workspace = app.workspace().map(|workspace| workspace.name());
    let mut edit = None;
    let mut change = None;
    if let Some(problem) = &view.problem {
        alert(ui, problem);
    }
    if rows.is_empty() {
        muted(ui, "No providers yet.");
    }
    for row in &rows {
        let builder = UiBuilder::new()
            .accessibility_role(Role::Group)
            .accessibility_label(&row.name);
        ui.push_id(("ai-provider", &row.id), |ui| {
            ui.scope_builder(builder, |ui| {
                boxed(ui, |ui| {
                    ui.label(RichText::new(&row.name).strong());
                    muted(ui, &format!("{}, {}", kind_name(app, &row.kind), row.model));
                    if let Some(endpoint) = &row.endpoint {
                        muted(ui, endpoint);
                    }
                    muted(
                        ui,
                        match (row.enabled, row.has_key) {
                            (true, true) => "On, with its key saved",
                            (true, false) => "On, without a key",
                            (false, true) => "Off, with its key saved",
                            (false, false) => "Off, without a key",
                        },
                    );
                    if let Some(workspace) = &workspace {
                        muted(
                            ui,
                            &if row.consented {
                                format!("You allowed it to receive {workspace}'s content.")
                            } else {
                                format!("It can't receive {workspace}'s content yet.")
                            },
                        );
                    }
                    ui.horizontal(|ui| {
                        if workspace.is_some() && row.enabled {
                            if row.consented {
                                if ui.button("Revoke for this workspace").clicked() {
                                    change = Some(Change::Revoke(row.id.clone()));
                                }
                            } else if ui.button("Allow for this workspace").clicked() {
                                change = Some(Change::Allow(row.id.clone()));
                            }
                        }
                        if ui.button("Edit").clicked() {
                            edit = Some(ProviderForm::of(row));
                        }
                        if row.has_key && ui.button("Remove key").clicked() {
                            change = Some(Change::RemoveKey(row.id.clone()));
                        }
                        if ui.button("Remove").clicked() {
                            change = Some(Change::Remove(row.id.clone()));
                        }
                    });
                });
            });
        });
        ui.add_space(4.0);
    }
    if let Some(change) = change {
        view.problem = change.apply(app).err().map(|error| error.message);
    }
    if let Some(form) = edit {
        view.form = Some(form);
    }
    ui.add_space(8.0);
    match &mut view.form {
        None => {
            if ui.button("Add provider").clicked() {
                let kind = app
                    .ai_provider_kinds()
                    .first()
                    .map_or("openai", |(kind, _)| kind);
                view.form = Some(ProviderForm::new(kind));
            }
        }
        Some(form) => {
            if provider_form(ui, app, form) {
                view.form = None;
            }
        }
    }
}

/// The form for a new or edited provider. Returns whether it closed.
fn provider_form(ui: &mut Ui, app: &mut App, form: &mut ProviderForm) -> bool {
    let title = if form.id.is_some() {
        "Edit provider"
    } else {
        "Add provider"
    };
    let builder = UiBuilder::new()
        .accessibility_role(Role::Form)
        .accessibility_label(title);
    let mut closed = false;
    ui.scope_builder(builder, |ui| {
        boxed(ui, |ui| {
            ui.label(RichText::new(title).strong());
            text_field(ui, "Name", &mut form.name, false);
            ui.label("Kind");
            ComboBox::from_id_salt("ai-provider-kind")
                .selected_text(kind_name(app, &form.kind).to_owned())
                .show_ui(ui, |ui| {
                    for (kind, name) in app.ai_provider_kinds() {
                        ui.selectable_value(&mut form.kind, (*kind).to_owned(), *name);
                    }
                })
                .response
                .accessible_name("Kind");
            text_field(ui, "Model", &mut form.model, false);
            text_field(ui, "Endpoint (optional)", &mut form.endpoint, false);
            muted(
                ui,
                "Use an HTTPS address. Plain HTTP works only for a server on this computer, \
                 such as Ollama.",
            );
            let key_label = if form.has_key {
                "New key (leave empty to keep the saved one)"
            } else {
                "Key"
            };
            text_field(ui, key_label, &mut form.key, true);
            ui.checkbox(&mut form.enabled, "Turn this provider on");
            if let Some(problem) = &form.problem {
                alert(ui, problem);
            }
            ui.horizontal(|ui| {
                let ready = !form.model.trim().is_empty() && !form.name.trim().is_empty();
                if primary_button(ui, "Save provider", ready).clicked() {
                    let endpoint = form.endpoint.trim();
                    let saved = app.save_ai_provider(AiProviderInput {
                        id: form.id.clone(),
                        name: form.name.clone(),
                        kind: form.kind.clone(),
                        model: form.model.clone(),
                        endpoint: (!endpoint.is_empty()).then(|| endpoint.to_owned()),
                        enabled: form.enabled,
                        key: (!form.key.trim().is_empty()).then(|| (*form.key).clone()),
                    });
                    match saved {
                        Ok(_) => closed = true,
                        Err(error) => form.problem = Some(error.message),
                    }
                }
                if ui.button("Cancel").clicked() {
                    closed = true;
                }
            });
        });
    });
    closed
}

/// A labeled one-line text field. A secret one hides what you type.
fn text_field(ui: &mut Ui, label: &str, text: &mut String, secret: bool) {
    ui.label(label);
    ui.add(
        TextEdit::singleline(text)
            .password(secret)
            .desired_width(320.0),
    )
    .accessible_name(label);
}

/// A bordered box around a provider or the form.
fn boxed(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let palette = Palette::of(ui.ctx());
    Frame::new()
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(radius(1.0))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical(add);
        });
}

fn alert(ui: &mut Ui, message: &str) {
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new().accessibility_role(Role::Alert);
    ui.scope_builder(builder, |ui| {
        ui.label(RichText::new(message).color(palette.destructive));
    });
}

/// What the consent prompt remembers between frames.
#[derive(Default)]
pub(crate) struct ConsentView {
    /// The provider the prompt asked about last frame.
    asking: Option<String>,
}

/// The prompt that asks whether a provider may receive the open
/// workspace's content. It starts on Deny. Escape or a click outside it
/// counts as Deny.
pub(crate) fn consent_prompt(ui: &mut Ui, app: &mut App, view: &mut ConsentView) {
    let Some(prompt) = app.ai_consent_prompt() else {
        view.asking = None;
        return;
    };
    let opened = view.asking.as_deref() != Some(prompt.provider_id.as_str());
    view.asking = Some(prompt.provider_id.clone());
    let title = format!(
        "Let {} receive this workspace's content?",
        prompt.provider_name
    );
    let mut answer = None;
    let modal = Modal::new(Id::unique("ai-consent")).show(ui.ctx(), |ui| {
        let builder = UiBuilder::new()
            .accessibility_role(Role::AlertDialog)
            .accessibility_label(&title);
        ui.scope_builder(builder, |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new(&title).size(TITLE_SIZE).strong());
            ui.add_space(4.0);
            ui.label(details(app, &prompt));
            muted(
                ui,
                "Noura asks again if you change the provider's kind, endpoint or model. To take \
                 this back, open Settings, then AI providers.",
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
        Some(true) => app.allow_ai_consent(),
        Some(false) => app.deny_ai_consent(),
        None => {}
    }
}

/// What the prompt says goes where.
fn details(app: &App, prompt: &AiConsentPrompt) -> String {
    let address = prompt
        .endpoint
        .as_deref()
        .map(|endpoint| format!(" at {endpoint}"))
        .unwrap_or_default();
    format!(
        "Your messages and the chat so far go from {} to {}, which answers with {} ({}){address}.",
        prompt.workspace,
        prompt.provider_name,
        prompt.model,
        kind_name(app, &prompt.kind)
    )
}
