//! Forms and their labeled inputs: single-line and multi-line text, a
//! checkbox, a single choice drawn as radio buttons like Settings' choices,
//! a date, and a date and time. Each change sends the input's typed value;
//! the submit button sends every input of the form.
//!
//! What you type stays until the plugin draws the input with a value other
//! than the one it drew before, so a plugin that ignores a change doesn't
//! wipe your typing. A date field sends nothing until it holds a whole
//! `YYYY-MM-DD` date, and says so meanwhile.
//!
//! A name field takes the keyboard when it appears. Enter or leaving it
//! sends the name it holds, and Escape, or leaving it empty or unchanged,
//! cancels. `app-core` keeps what you type in it meanwhile, so a name
//! still in the field when the window closes reaches the plugin first.

use app_core::{
    Field, InputValue, PluginEvent, PluginNaming, Submission, ViewKind, ViewNode, date_time_fields,
    date_time_value, date_value,
};
use egui::{Id, Key, RichText, TextEdit, Ui, UiBuilder, accesskit::Role};

use super::{Kit, boxed};
use crate::{
    common::primary_button,
    theme::{Palette, SUPPORTING_SIZE},
};

/// What an input remembers between frames, in egui's memory under its ID.
#[derive(Clone)]
struct Draft {
    /// The value the plugin drew last.
    drawn: InputValue,
    /// The value the input shows, which changes and submits send.
    shown: InputValue,
    /// The text field's text: the text, or the date.
    text: String,
    /// A date and time's time field.
    time: String,
    /// The fields hold text that isn't a whole value yet.
    invalid: bool,
}

impl Draft {
    fn of(value: &InputValue) -> Self {
        let (text, time) = match value {
            InputValue::Text(text) | InputValue::Multiline(text) => (text.clone(), String::new()),
            InputValue::Date(date) => (date.clone().unwrap_or_default(), String::new()),
            InputValue::DateTime(Some(at)) => date_time_fields(at).unwrap_or_default(),
            _ => (String::new(), String::new()),
        };
        Self {
            drawn: value.clone(),
            shown: value.clone(),
            text,
            time,
            invalid: false,
        }
    }

    /// The draft of the input `id`, or a new one when the plugin drew it
    /// with another value since.
    fn load(ui: &Ui, id: Id, value: &InputValue) -> Self {
        ui.data(|data| data.get_temp::<Self>(id))
            .filter(|draft| draft.drawn == *value)
            .unwrap_or_else(|| Self::of(value))
    }
}

pub(super) fn form(ui: &mut Ui, node: &ViewNode, label: &str, submit: &str, kit: &mut Kit) {
    boxed(ui, Role::Form, Some(label), |ui| {
        for child in &node.children {
            super::node(ui, child, kit);
        }
        ui.add_space(8.0);
        if primary_button(ui, submit, true).clicked() {
            let mut fields = Vec::new();
            let mut whole = true;
            inputs(ui, node, kit, &mut fields, &mut whole);
            if whole {
                kit.events.push(PluginEvent::Submitted(Submission {
                    node: node.id.clone(),
                    fields,
                }));
            }
        }
    });
}

/// The value of each input under `node`, in view order. `whole` turns
/// false when one holds text that isn't a whole value.
fn inputs(ui: &Ui, node: &ViewNode, kit: &Kit, fields: &mut Vec<Field>, whole: &mut bool) {
    for child in &node.children {
        if let ViewKind::Input { value, .. } = &child.kind {
            let draft = Draft::load(ui, kit.id(&child.id), value);
            *whole &= !draft.invalid;
            fields.push(Field {
                node: child.id.clone(),
                value: draft.shown,
            });
        }
        inputs(ui, child, kit, fields, whole);
    }
}

/// A labeled input.
pub(super) fn input(ui: &mut Ui, node: &ViewNode, kit: &mut Kit) {
    let ViewKind::Input {
        label,
        value,
        choices,
    } = &node.kind
    else {
        return;
    };
    let id = kit.id(&node.id);
    let mut draft = Draft::load(ui, id, value);
    let before = draft.shown.clone();
    ui.add_space(4.0);
    match value {
        InputValue::Checkbox(_) => {
            let mut checked = matches!(draft.shown, InputValue::Checkbox(true));
            if ui.checkbox(&mut checked, label.as_str()).changed() {
                draft.shown = InputValue::Checkbox(checked);
            }
        }
        InputValue::Choice(_) => {
            let builder = UiBuilder::new()
                .accessibility_role(Role::RadioGroup)
                .accessibility_label(label.as_str());
            ui.scope_builder(builder, |ui| {
                ui.label(RichText::new(label).strong());
                ui.horizontal_wrapped(|ui| {
                    for choice in choices {
                        let chosen =
                            matches!(&draft.shown, InputValue::Choice(Some(id)) if *id == choice.id);
                        if ui.radio(chosen, choice.label.as_str()).clicked() && !chosen {
                            draft.shown = InputValue::Choice(Some(choice.id.clone()));
                        }
                    }
                });
            });
        }
        InputValue::Text(_) | InputValue::Multiline(_) => {
            ui.label(RichText::new(label).strong());
            let multiline = matches!(value, InputValue::Multiline(_));
            let field = if multiline {
                TextEdit::multiline(&mut draft.text)
                    .desired_rows(3)
                    .lock_focus(false)
            } else {
                TextEdit::singleline(&mut draft.text)
            };
            let response = ui
                .add(field.id(id.with("text")).desired_width(f32::INFINITY))
                .accessible_name(label);
            if response.changed() {
                draft.shown = if multiline {
                    InputValue::Multiline(draft.text.clone())
                } else {
                    InputValue::Text(draft.text.clone())
                };
            }
        }
        InputValue::Date(_) => {
            ui.label(RichText::new(label).strong());
            let response = ui
                .add(
                    TextEdit::singleline(&mut draft.text)
                        .id(id.with("date"))
                        .hint_text("YYYY-MM-DD")
                        .desired_width(120.0),
                )
                .accessible_name(label);
            if response.changed() {
                match date_value(&draft.text) {
                    Some(date) => {
                        draft.invalid = false;
                        draft.shown = InputValue::Date(date);
                    }
                    None => draft.invalid = true,
                }
            }
            if draft.invalid {
                problem(ui, response.id, "Type a date as YYYY-MM-DD.");
            }
        }
        InputValue::DateTime(_) => {
            ui.label(RichText::new(label).strong());
            let held = match &draft.shown {
                InputValue::DateTime(at) => at.clone(),
                _ => None,
            };
            let (date, time) = ui
                .horizontal(|ui| {
                    let date = ui
                        .add(
                            TextEdit::singleline(&mut draft.text)
                                .id(id.with("date"))
                                .hint_text("YYYY-MM-DD")
                                .desired_width(120.0),
                        )
                        .accessible_name(format!("{label}, date"));
                    let time = ui
                        .add(
                            TextEdit::singleline(&mut draft.time)
                                .id(id.with("time"))
                                .hint_text("HH:MM")
                                .desired_width(64.0),
                        )
                        .accessible_name(format!("{label}, time"));
                    (date, time)
                })
                .inner;
            if date.changed() || time.changed() {
                match date_time_value(&draft.text, &draft.time, held.as_deref()) {
                    Some(at) => {
                        draft.invalid = false;
                        draft.shown = InputValue::DateTime(at);
                    }
                    None => draft.invalid = true,
                }
            }
            if draft.invalid {
                problem(
                    ui,
                    date.id,
                    "Type a date as YYYY-MM-DD and a time as HH:MM.",
                );
            }
        }
    }
    if draft.shown != before {
        kit.events.push(PluginEvent::Changed(Field {
            node: node.id.clone(),
            value: draft.shown.clone(),
        }));
    }
    ui.data_mut(|data| data.insert_temp(id, draft));
}

/// What a name field remembers between frames, in egui's memory under its
/// ID.
#[derive(Clone)]
struct NameDraft {
    /// The name the plugin drew the field with.
    drawn: String,
    text: String,
}

/// A name field: it takes the keyboard when it first appears, and leaving
/// it sends the name it holds, or cancels on Escape or with no new name.
pub(super) fn name_field(ui: &mut Ui, node: &ViewNode, label: &str, value: &str, kit: &mut Kit) {
    let id = kit.id(&node.id);
    let held = ui
        .data(|data| data.get_temp::<NameDraft>(id))
        .filter(|draft| draft.drawn == value);
    let appeared = held.is_none();
    let mut draft = held.unwrap_or_else(|| NameDraft {
        drawn: value.to_owned(),
        text: value.to_owned(),
    });
    ui.add_space(4.0);
    let field_id = id.with("name");
    let response = ui
        .add(
            TextEdit::singleline(&mut draft.text)
                .id(field_id)
                .hint_text(label)
                .desired_width(f32::INFINITY),
        )
        .accessible_name(label);
    if appeared {
        response.request_focus();
    }
    let place = kit.place.plugin_place();
    if response.changed() {
        kit.app
            .set_plugin_name_draft(place.clone(), &node.id, value, &draft.text);
    }
    if response.lost_focus() {
        let escaped = ui.input(|input| input.key_pressed(Key::Escape));
        let name = draft.text.trim();
        let event = if escaped || name.is_empty() || name == value.trim() {
            PluginEvent::Cancelled(node.id.clone())
        } else {
            PluginEvent::Named(PluginNaming {
                node: node.id.clone(),
                name: name.to_owned(),
            })
        };
        kit.app.clear_plugin_name_draft(&place, &node.id);
        kit.events.push(event);
    }
    ui.data_mut(|data| data.insert_temp(id, draft));
}

/// Why a field's text isn't a value yet, under the field and in its
/// description for screen readers.
fn problem(ui: &mut Ui, field: Id, text: &str) {
    let palette = Palette::of(ui.ctx());
    ui.label(
        RichText::new(text)
            .size(SUPPORTING_SIZE)
            .color(palette.destructive),
    );
    let text = text.to_owned();
    ui.ctx()
        .accesskit_node_builder(field, |node| node.set_description(text));
}
