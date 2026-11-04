//! What the kernel draws for AI: the AI section of Settings, where you
//! connect a provider by pasting its key, which goes to the credential
//! store, and consent for the open workspace, the prompt that asks before a provider
//! receives the workspace's content, and the prompt that asks before a
//! tool call the model wants runs. Plugins draw none of them, so no
//! plugin can show a prompt that grants consent, allow its own tool calls
//! or read a key.

use app_core::{AiConsentPrompt, AiProviderInput, AiProviderRow, App};
use egui::{Button, Frame, Id, Margin, RichText, Stroke, TextEdit, Ui, UiBuilder, accesskit::Role};
use zeroize::Zeroizing;

use crate::{
    common::{BadgeLook, Btn, Size, Variant, badge, description, primary_button},
    theme::{Palette, TITLE_SIZE, radius},
};

/// What the AI section remembers between frames.
#[derive(Default)]
pub(crate) struct AiSettingsView {
    /// The provider row you opened, by its kind, or [`CUSTOM`].
    open: Option<String>,
    /// The key being pasted in the open row. It leaves memory when the
    /// row closes or the key saves.
    key: Zeroizing<String>,
    /// Local or custom's fields.
    custom: CustomForm,
    /// Why the last change to a provider didn't go in.
    problem: Option<String>,
}

/// The row for providers that need an address: Ollama and servers that
/// speak OpenAI's interface.
const CUSTOM: &str = "custom";

/// Local or custom's fields: only what such a provider needs.
#[derive(Default)]
struct CustomForm {
    /// A server that speaks OpenAI's interface rather than Ollama.
    compatible: bool,
    address: String,
    model: String,
    key: Zeroizing<String>,
}

/// The name Settings shows for a provider kind.
fn kind_name<'a>(app: &App, kind: &'a str) -> &'a str {
    app.ai_provider_kinds()
        .iter()
        .find(|(known, _)| *known == kind)
        .map_or(kind, |(_, name)| name)
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

/// A section's title with an optional button at its end, then a line
/// under it, as the legacy AI settings drew its sections.
fn section_title(ui: &mut Ui, title: &str, text: Option<&str>, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            crate::common::field_label(ui, title);
            if let Some(text) = text {
                crate::common::muted(ui, text);
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), add);
    });
    ui.add_space(16.0);
    ui.separator();
}

/// A divider between two rows of a section's list.
fn row_line(ui: &mut Ui) {
    let palette = Palette::of(ui.ctx());
    let y = ui.cursor().top();
    ui.painter()
        .hline(ui.max_rect().x_range(), y, Stroke::new(1.0, palette.border));
}

/// A row you open and close: its name, a badge at its end and a caret.
/// Returns whether you clicked it.
fn provider_row(
    ui: &mut Ui,
    name: &str,
    badge_text: Option<(&str, BadgeLook)>,
    open: bool,
) -> bool {
    let palette = Palette::of(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 48.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(Role::Button, true, name));
    ui.ctx()
        .accesskit_node_builder(response.id, |node| node.set_expanded(open));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(
                rect,
                crate::theme::radius(1.0),
                palette.muted.gamma_multiply(0.5),
            );
        }
        if response.has_focus() {
            crate::common::focus_ring(painter, rect, crate::theme::radius(1.0), &palette);
        }
        let title = painter.layout_no_wrap(
            name.to_owned(),
            crate::theme::font(
                ui.ctx(),
                crate::theme::BODY_SIZE,
                crate::theme::Weight::Medium,
            ),
            palette.foreground,
        );
        painter.galley(
            egui::pos2(rect.left() + 8.0, rect.center().y - title.size().y / 2.0),
            title,
            palette.foreground,
        );
        let caret = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 16.0, rect.center().y),
            egui::vec2(16.0, 16.0),
        );
        let icon = if open {
            crate::icons::CARET_DOWN
        } else {
            crate::icons::CARET_RIGHT
        };
        crate::icons::paint(painter, icon, caret, palette.muted_foreground, false);
    }
    if let Some((text, look)) = badge_text {
        let galley = ui.painter().layout_no_wrap(
            text.to_owned(),
            crate::theme::font(
                ui.ctx(),
                crate::theme::SUPPORTING_SIZE,
                crate::theme::Weight::Medium,
            ),
            palette.foreground,
        );
        let width = galley.size().x + 16.0;
        let badge_rect = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 32.0 - width / 2.0, rect.center().y),
            egui::vec2(width, 20.0),
        );
        let mut badge_ui = ui.new_child(UiBuilder::new().max_rect(badge_rect));
        badge(&mut badge_ui, text, look);
    }
    response.clicked()
}

/// The open row's key field: its title with a key icon, what to paste,
/// the field and Save key, then Remove key once it is connected. Returns
/// whether you saved the key.
fn key_field(
    ui: &mut Ui,
    name: &str,
    key: &mut Zeroizing<String>,
    connected: bool,
    remove: &mut bool,
) -> bool {
    let mut save = false;
    Frame::new()
        .inner_margin(Margin {
            left: 8,
            right: 8,
            top: 0,
            bottom: 16,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.horizontal(|ui| {
                let palette = Palette::of(ui.ctx());
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                crate::icons::paint(
                    ui.painter(),
                    crate::icons::KEY,
                    rect,
                    palette.foreground,
                    false,
                );
                crate::common::field_label(ui, name);
            });
            description(ui, "Paste a key you created with the provider.");
            ui.horizontal(|ui| {
                let field = ui
                    .add(
                        TextEdit::singleline(&mut **key)
                            .password(true)
                            .hint_text("sk-...")
                            .desired_width(ui.available_width() - 100.0),
                    )
                    .accessible_name("API key");
                save |= field.lost_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    && !key.trim().is_empty();
                save |= Btn::new("Save key")
                    .variant(Variant::Default)
                    .enabled(!key.trim().is_empty())
                    .show(ui)
                    .clicked();
            });
            if connected {
                *remove = Btn::new("Remove key")
                    .variant(Variant::Ghost)
                    .size(Size::Sm)
                    .show(ui)
                    .clicked();
            }
        });
    save
}

/// The AI section of Settings: the providers Noura connects to, each
/// opening a key field, then Local or custom, then what each provider
/// may read in the open workspace.
pub(crate) fn settings(ui: &mut Ui, app: &mut App, view: &mut AiSettingsView) {
    if let Some(problem) = app.ai_settings_problem() {
        alert(ui, &problem);
        return;
    }
    let rows = app.ai_providers();
    let choices = app.ai_provider_choices();
    let workspace = app.workspace().map(|workspace| workspace.name());
    let mut change = None;
    ui.spacing_mut().item_spacing.y = 8.0;
    section_title(ui, "Providers", None, |_| {});
    if let Some(problem) = &view.problem {
        alert(ui, problem);
    }
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut toggled = None;
    let mut connect = None;
    for (index, choice) in choices.iter().enumerate() {
        if index > 0 {
            row_line(ui);
        }
        let open = view.open.as_deref() == Some(choice.kind);
        let builder = UiBuilder::new()
            .accessibility_role(Role::Group)
            .accessibility_label(choice.name);
        ui.push_id(("ai-provider", choice.kind), |ui| {
            ui.scope_builder(builder, |ui| {
                let status = if choice.connected {
                    ("Connected", BadgeLook::Secondary)
                } else {
                    ("Not connected", BadgeLook::Outline)
                };
                if provider_row(ui, choice.name, Some(status), open) {
                    toggled = Some(choice.kind.to_owned());
                }
                if open {
                    let mut remove = false;
                    if key_field(
                        ui,
                        choice.name,
                        &mut view.key,
                        choice.connected,
                        &mut remove,
                    ) {
                        connect = Some(choice.kind);
                    }
                    if remove && let Some(id) = &choice.provider {
                        change = Some(Change::RemoveKey(id.clone()));
                    }
                }
            });
        });
    }
    // Local or custom, last: providers that need an address.
    row_line(ui);
    let custom_rows: Vec<&AiProviderRow> = rows
        .iter()
        .filter(|row| {
            !choices
                .iter()
                .any(|choice| choice.provider.as_deref() == Some(row.id.as_str()))
        })
        .collect();
    let open = view.open.as_deref() == Some(CUSTOM);
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label("Local or custom");
    ui.scope_builder(builder, |ui| {
        let count = (!custom_rows.is_empty()).then(|| format!("{} set up", custom_rows.len()));
        let status = count.as_deref().map(|text| (text, BadgeLook::Secondary));
        if provider_row(ui, "Local or custom", status, open) {
            toggled = Some(CUSTOM.to_owned());
        }
        if open {
            for row in &custom_rows {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new(&row.name).size(crate::theme::BODY_SIZE));
                    crate::common::muted(ui, &row.model);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = format!("Remove {}", row.name);
                        let response = Btn::new("Remove")
                            .variant(Variant::Ghost)
                            .size(Size::Sm)
                            .show(ui);
                        ui.ctx().accesskit_node_builder(response.id, |node| {
                            node.set_label(label);
                        });
                        if response.clicked() {
                            change = Some(Change::Remove(row.id.clone()));
                        }
                    });
                });
            }
            if custom_form(ui, app, &mut view.custom, &mut view.problem) {
                view.open = None;
            }
        }
    });
    if let Some(kind) = connect {
        match app.connect_ai_provider(kind, &view.key) {
            Ok(_) => {
                view.problem = None;
                view.key = Zeroizing::new(String::new());
                view.open = None;
            }
            Err(error) => view.problem = Some(error.message),
        }
    }
    if let Some(kind) = toggled {
        view.key = Zeroizing::new(String::new());
        view.open = (view.open.as_deref() != Some(kind.as_str())).then_some(kind);
    }

    ui.spacing_mut().item_spacing.y = 8.0;
    ui.add_space(32.0);
    section_title(
        ui,
        "Workspace permissions",
        Some("A granted provider receives this workspace's content."),
        |_| {},
    );
    match &workspace {
        _ if rows.is_empty() => {
            description(ui, "Connect a provider first.");
        }
        None => {
            description(ui, "Open a workspace first.");
        }
        Some(_) => {
            for (index, row) in rows.iter().enumerate() {
                if index > 0 {
                    row_line(ui);
                }
                let label = format!("{} in this workspace", row.name);
                let builder = UiBuilder::new()
                    .accessibility_role(Role::Group)
                    .accessibility_label(&label);
                ui.push_id(("ai-consent", &row.id), |ui| {
                    ui.scope_builder(builder, |ui| {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            crate::common::field_label(ui, &row.name);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    if !row.enabled {
                                        return;
                                    }
                                    if row.consented {
                                        if Btn::new("Revoke").size(Size::Sm).show(ui).clicked() {
                                            change = Some(Change::Revoke(row.id.clone()));
                                        }
                                        badge(ui, "Granted", BadgeLook::Secondary);
                                    } else {
                                        if Btn::new("Grant")
                                            .variant(Variant::Default)
                                            .size(Size::Sm)
                                            .show(ui)
                                            .clicked()
                                        {
                                            change = Some(Change::Allow(row.id.clone()));
                                        }
                                        badge(ui, "Not granted", BadgeLook::Outline);
                                    }
                                },
                            );
                        });
                        ui.add_space(8.0);
                    });
                });
            }
        }
    }
    if let Some(change) = change {
        view.problem = change.apply(app).err().map(|error| error.message);
    }
}

/// Local or custom's form: Ollama or a server that speaks OpenAI's
/// interface, its address, its model and, for the latter, its key.
/// Returns whether it saved.
fn custom_form(
    ui: &mut Ui,
    app: &mut App,
    form: &mut CustomForm,
    problem: &mut Option<String>,
) -> bool {
    let mut saved = false;
    let builder = UiBuilder::new()
        .accessibility_role(Role::Form)
        .accessibility_label("Add a local or custom provider");
    ui.scope_builder(builder, |ui| {
        Frame::new()
            .inner_margin(Margin {
                left: 8,
                right: 8,
                top: 8,
                bottom: 16,
            })
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                let mut compatible = form.compatible;
                let builder = UiBuilder::new()
                    .accessibility_role(Role::RadioGroup)
                    .accessibility_label("Server");
                ui.scope_builder(builder, |ui| {
                    crate::common::segmented(
                        ui,
                        &[(false, "Ollama"), (true, "OpenAI-compatible")],
                        &mut compatible,
                    );
                });
                form.compatible = compatible;
                let hint = if form.compatible {
                    "https://"
                } else {
                    "http://localhost:11434/v1/"
                };
                ui.add(
                    TextEdit::singleline(&mut form.address)
                        .hint_text(hint)
                        .desired_width(f32::INFINITY),
                )
                .accessible_name("Address");
                ui.add(
                    TextEdit::singleline(&mut form.model)
                        .hint_text("Model")
                        .desired_width(f32::INFINITY),
                )
                .accessible_name("Model");
                if form.compatible {
                    ui.add(
                        TextEdit::singleline(&mut *form.key)
                            .password(true)
                            .hint_text("sk-...")
                            .desired_width(f32::INFINITY),
                    )
                    .accessible_name("API key");
                }
                let ready = !form.model.trim().is_empty()
                    && (!form.compatible || !form.address.trim().is_empty());
                if Btn::new("Save provider")
                    .variant(Variant::Default)
                    .enabled(ready)
                    .show(ui)
                    .clicked()
                {
                    let (kind, name) = if form.compatible {
                        ("openai", "Custom")
                    } else {
                        ("ollama", "Ollama")
                    };
                    let address = form.address.trim();
                    let endpoint = match address {
                        "" => Some("http://localhost:11434/v1/".to_owned()),
                        address => Some(address.to_owned()),
                    };
                    let key = (form.compatible && !form.key.trim().is_empty())
                        .then(|| (*form.key).clone());
                    let result = app.save_ai_provider(AiProviderInput {
                        id: None,
                        name: name.to_owned(),
                        kind: kind.to_owned(),
                        model: form.model.trim().to_owned(),
                        endpoint,
                        enabled: true,
                        key,
                        context_window: None,
                    });
                    match result {
                        Ok(_) => {
                            *form = CustomForm::default();
                            *problem = None;
                            saved = true;
                        }
                        Err(error) => *problem = Some(error.message),
                    }
                }
            });
    });
    saved
}

/// A labeled one-line text field. A secret one hides what you type.
pub(crate) fn text_field(ui: &mut Ui, label: &str, text: &mut String, secret: bool) {
    ui.label(label);
    ui.add(
        TextEdit::singleline(text)
            .password(secret)
            .desired_width(320.0),
    )
    .accessible_name(label);
}

/// A bordered box around a provider or the form.
pub(crate) fn boxed(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
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

pub(crate) fn alert(ui: &mut Ui, message: &str) {
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
    let modal = crate::common::dialog(ui.ctx(), Id::unique("ai-consent")).show(ui.ctx(), |ui| {
        let builder = UiBuilder::new()
            .accessibility_role(Role::AlertDialog)
            .accessibility_label(&title);
        ui.scope_builder(builder, |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new(&title).font(crate::theme::font(
                ui.ctx(),
                TITLE_SIZE,
                crate::theme::Weight::Medium,
            )));
            ui.add_space(4.0);
            ui.label(details(app, &prompt));
            description(
                ui,
                "Noura asks again if its kind, endpoint or model changes. Revoke it in Settings > AI.",
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

/// What the tool prompt remembers between frames.
#[derive(Default)]
pub(crate) struct ToolPromptView {
    /// The run and the call the prompt asked about last frame.
    asking: Option<(String, String)>,
}

/// The tallest the tool prompt draws a call's input before it scrolls.
const TOOL_INPUT_HEIGHT: f32 = 240.0;

/// The prompt that asks whether a tool call the model wants may run. It
/// names the tool, its plugin and its input, starts on Deny, and Escape
/// or a click outside it counts as Deny. It waits while the consent
/// prompt shows, so you answer one question at a time.
pub(crate) fn tool_prompt(ui: &mut Ui, app: &mut App, view: &mut ToolPromptView) {
    if app.ai_consent_prompt().is_some() {
        return;
    }
    let Some(prompt) = app.ai_tool_prompt() else {
        view.asking = None;
        return;
    };
    let asking = (prompt.run.clone(), prompt.call.clone());
    let opened = view.asking.as_ref() != Some(&asking);
    view.asking = Some(asking);
    let title = "Allow this tool call?";
    let mut answer = None;
    let modal = crate::common::dialog(ui.ctx(), Id::unique("ai-tool")).show(ui.ctx(), |ui| {
        let builder = UiBuilder::new()
            .accessibility_role(Role::AlertDialog)
            .accessibility_label(title);
        ui.scope_builder(builder, |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new(title).font(crate::theme::font(
                ui.ctx(),
                TITLE_SIZE,
                crate::theme::Weight::Medium,
            )));
            ui.add_space(4.0);
            ui.label(format!("{} from {}", prompt.tool, prompt.plugin_name));
            if !prompt.description.trim().is_empty() {
                description(ui, &prompt.description);
            }
            ui.add_space(8.0);
            ui.label("Input");
            // A long input scrolls, so Deny and Allow once stay in view.
            egui::ScrollArea::vertical()
                .id_salt("ai-tool-input")
                .max_height(TOOL_INPUT_HEIGHT)
                .show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(&prompt.input).monospace()).wrap())
                        .accessible_name("Input");
                });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let deny = ui.add(Button::new("Deny"));
                if opened {
                    deny.request_focus();
                }
                if deny.clicked() {
                    answer = Some(false);
                }
                if primary_button(ui, "Allow once", true).clicked() {
                    answer = Some(true);
                }
            });
        });
    });
    if answer.is_none() && modal.should_close() {
        answer = Some(false);
    }
    if let Some(allow) = answer {
        app.answer_ai_tool(&prompt, allow);
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
        "Your messages and this chat go from {} to {}, model {} ({}){address}.",
        prompt.workspace,
        prompt.provider_name,
        prompt.model,
        kind_name(app, &prompt.kind)
    )
}
