//! The note screen: the note's name, a notice when a save failed, the
//! live-preview editor, and the properties panel beside it for Noura's own
//! notes.

use std::{collections::HashMap, time::Instant};

use app_core::{App, Property, PropertyValue, TabId};
use editor_egui::{Editor, MdConfig};
use egui::{
    Button, Frame, Grid, Id, Key, Margin, Modifiers, Panel, RichText, ScrollArea, Stroke, TextEdit,
    Ui, UiBuilder,
    accesskit::Role,
    text::{CCursor, CCursorRange},
    text_edit::TextEditState,
};

use crate::{
    common::muted,
    theme::{Palette, SUPPORTING_SIZE, TITLE_SIZE, radius},
};

/// How wide the properties panel starts.
const PROPERTIES_WIDTH: f32 = 260.0;

/// The editors of the notes open in tabs.
#[derive(Default)]
pub(crate) struct NoteView {
    notes: HashMap<TabId, OpenNote>,
}

/// One note's editor and the fields around it.
struct OpenNote {
    editor: Editor,
    /// What the name field holds while you type in it; `None` shows the
    /// file's name.
    name: Option<String>,
    /// Move the keyboard to the name field on the next frame.
    focus_name: bool,
    /// What each property's field holds while you edit it.
    drafts: HashMap<String, String>,
    /// The widget to hand the keyboard back to after the editor's first
    /// frame: a note opened from the keyboard, as with Enter in the file
    /// tree, leaves the keyboard where it was. A click puts the caret in
    /// the text.
    give_focus_back: Option<Id>,
}

impl NoteView {
    /// Whether the editor of the note in `tab` has the keyboard.
    pub(crate) fn editor_focused(&self, ctx: &egui::Context, tab: Option<TabId>) -> bool {
        tab.and_then(|tab| self.notes.get(&tab))
            .is_some_and(|note| note.editor.focused(ctx))
    }

    /// Drop the editors of tabs that closed.
    pub(crate) fn retain(&mut self, app: &App) {
        self.notes
            .retain(|tab, _| app.tabs().iter().any(|open| open.id() == *tab));
    }
}

fn name_field_id(tab: TabId) -> Id {
    Id::unique(("note-name", tab))
}

/// Draw the note in `tab`, or why it couldn't open. Returns whether the tab
/// shows a note.
pub(crate) fn show(ui: &mut Ui, app: &mut App, view: &mut NoteView, tab: TabId) -> bool {
    if let Some(error) = app.note_unreadable(tab) {
        let message = error.message.clone();
        ui.centered_and_justified(|ui| muted(ui, &message));
        return true;
    }
    let Some(session) = app.note(tab) else {
        return false;
    };
    let note = view.notes.entry(tab).or_insert_with(|| {
        let clicked = ui.input(|input| input.pointer.any_released());
        let config = if session.shows_source() {
            // Any extension but `md` shows the text as source.
            MdConfig {
                ext: "txt".into(),
                ..MdConfig::default()
            }
        } else {
            MdConfig::default()
        };
        OpenNote {
            editor: Editor::new(ui.ctx(), session.text(), config),
            name: None,
            focus_name: false,
            drafts: HashMap::new(),
            give_focus_back: if clicked {
                None
            } else {
                ui.memory(|memory| memory.focused())
            },
        }
    });
    // A save brought the file in line with the editor, or merged an edit
    // made outside Noura.
    if let Some(sync) = app.take_note_sync(tab) {
        note.editor.record_disk_save(sync.saved.0, sync.saved.1);
        if let Some(merged) = sync.merged {
            note.editor.apply_disk_version(merged);
        }
    }
    header(ui, app, note, tab);
    save_notice(ui, app, tab);
    if let Some(properties) = app.note(tab).and_then(|session| session.properties()) {
        let palette = Palette::of(ui.ctx());
        Panel::right(("note-properties", tab))
            .resizable(true)
            .default_size(PROPERTIES_WIDTH)
            .frame(
                Frame::new()
                    .fill(palette.background)
                    .stroke(Stroke::new(1.0, palette.border))
                    .inner_margin(Margin::same(12)),
            )
            .show(ui, |ui| properties_panel(ui, app, note, tab, &properties));
    }
    body(ui, app, note, tab);
    true
}

/// The note's name, which renames its file: Enter or leaving the field
/// renames, and Escape keeps the old name.
fn header(ui: &mut Ui, app: &mut App, note: &mut OpenNote, tab: TabId) {
    let Some(session) = app.note(tab) else {
        return;
    };
    let shown = session.name();
    let problem = session.name_problem().map(str::to_owned);
    let id = name_field_id(tab);
    let mut draft = note.name.clone().unwrap_or_else(|| shown.clone());
    let response = ui
        .add(
            TextEdit::singleline(&mut draft)
                .id(id)
                .hint_text("Untitled")
                .font(egui::FontId::proportional(TITLE_SIZE + 6.0))
                .frame(egui::Frame::NONE)
                .desired_width(f32::INFINITY),
        )
        .accessible_name("File name");
    if response.changed() {
        note.name = Some(draft);
    }
    if note.focus_name {
        note.focus_name = false;
        response.request_focus();
        let mut state = TextEditState::load(ui.ctx(), id).unwrap_or_default();
        let end = CCursor::new(shown.chars().count());
        state
            .cursor
            .set_char_range(Some(CCursorRange::two(CCursor::new(0), end)));
        state.store(ui.ctx(), id);
    }
    if response.lost_focus() {
        let (enter, escape) = ui.input(|input| {
            (
                input.key_pressed(Key::Enter),
                input.key_pressed(Key::Escape),
            )
        });
        let typed = note.name.take();
        if !escape && let Some(typed) = typed {
            app.rename_note(tab, &typed);
        }
        if enter || escape {
            note.editor.focus(ui.ctx());
        }
    }
    if let Some(problem) = problem {
        let palette = Palette::of(ui.ctx());
        ui.scope_builder(UiBuilder::new().accessibility_role(Role::Alert), |ui| {
            ui.label(
                RichText::new(problem)
                    .size(SUPPORTING_SIZE)
                    .color(palette.destructive),
            );
        });
    }
    ui.add_space(4.0);
}

/// Why the last save failed, with Try again. The next edit tries again too.
fn save_notice(ui: &mut Ui, app: &mut App, tab: TabId) {
    let Some(problem) = app
        .note(tab)
        .and_then(|session| session.save_problem())
        .cloned()
    else {
        return;
    };
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new()
        .accessibility_role(Role::Alert)
        .accessibility_label("Changes aren\u{2019}t saved");
    ui.scope_builder(builder, |ui| {
        Frame::new()
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(radius(1.0))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("Changes aren\u{2019}t saved")
                        .strong()
                        .color(palette.destructive),
                );
                ui.label(&problem.reason);
                if ui.button("Try again").clicked() {
                    app.save_note_now(tab);
                }
            });
    });
    ui.add_space(8.0);
}

/// The editor, as wide as the line width setting lets it run, centered,
/// with its text at the text size setting.
fn body(ui: &mut Ui, app: &mut App, note: &mut OpenNote, tab: TabId) {
    // F2 in the text moves to the name field.
    if note.editor.focused(ui.ctx())
        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::F2))
    {
        note.focus_name = true;
        ui.ctx().request_repaint();
    }
    let available = ui.available_rect_before_wrap();
    let preferences = app.preferences();
    let width = preferences
        .line_width
        .points()
        .map_or(available.width(), |points| points.min(available.width()));
    note.editor.edit.renderer.layout.max_width = width;
    note.editor.set_text_size(preferences.text_size.points());
    let left = available.left() + (available.width() - width) / 2.0;
    let rect = egui::Rect::from_min_size(
        egui::pos2(left, available.top()),
        egui::vec2(width, available.height()),
    );
    let builder = UiBuilder::new()
        .max_rect(rect)
        .accessibility_role(Role::Document)
        .accessibility_label("Note text");
    let response = ui.scope_builder(builder, |ui| note.editor.show(ui)).inner;
    // The widget that takes the keyboard reads as the note's text field.
    ui.ctx().accesskit_node_builder(note.editor.id(), |node| {
        node.set_role(Role::MultilineTextInput);
        node.set_label("Note text");
    });
    if let Some(id) = note.give_focus_back.take() {
        ui.memory_mut(|memory| memory.request_focus(id));
    }
    if response.text_updated
        && app
            .note(tab)
            .is_some_and(|session| session.text() != note.editor.text())
    {
        app.edit_note(
            tab,
            note.editor.version(),
            note.editor.text(),
            Instant::now(),
        );
    }
}

/// The note's frontmatter: ID, type, created and updated read-only, and
/// strings, numbers, booleans and dates edited in place.
fn properties_panel(
    ui: &mut Ui,
    app: &mut App,
    note: &mut OpenNote,
    tab: TabId,
    properties: &[Property],
) {
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label("Properties");
    ui.scope_builder(builder, |ui| {
        ui.label(RichText::new("Properties").strong());
        ui.add_space(4.0);
        ScrollArea::vertical()
            .id_salt(("note-properties-scroll", tab))
            .show(ui, |ui| {
                Grid::new(("note-properties-grid", tab))
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        for property in properties {
                            muted(ui, &property.key);
                            if let Some(value) = property_value(ui, note, property) {
                                app.set_note_property(tab, &property.key, value);
                            }
                            ui.end_row();
                        }
                    });
            });
    });
}

/// Draw one property's value. Returns the new value once an edit is done.
fn property_value(ui: &mut Ui, note: &mut OpenNote, property: &Property) -> Option<PropertyValue> {
    let key = &property.key;
    if property.read_only {
        let text = match &property.value {
            PropertyValue::Text(text) | PropertyValue::Date(text) | PropertyValue::Other(text) => {
                text.clone()
            }
            PropertyValue::Number(number) => number.to_string(),
            PropertyValue::Bool(value) => value.to_string(),
        };
        // Selectable and copyable, but not editable.
        ui.add(
            TextEdit::singleline(&mut text.as_str())
                .frame(Frame::NONE)
                .desired_width(f32::INFINITY),
        )
        .accessible_name(key.clone());
        return None;
    }
    match &property.value {
        PropertyValue::Bool(value) => {
            let mut checked = *value;
            let response = ui.checkbox(&mut checked, "").accessible_name(key.clone());
            response.changed().then_some(PropertyValue::Bool(checked))
        }
        PropertyValue::Date(text) => {
            let mut date: jiff::civil::Date = text.parse().ok()?;
            let response = ui.add(
                egui_extras::DatePickerButton::new(&mut date)
                    .id_salt(key)
                    .show_icon(false)
                    .calendar_week(false),
            );
            response.accessible_name(key.clone());
            let picked = date.to_string();
            (picked != *text).then_some(PropertyValue::Date(picked))
        }
        PropertyValue::Text(text) => text_field(ui, note, key, text).map(PropertyValue::Text),
        PropertyValue::Number(number) => {
            let typed = text_field(ui, note, key, &number.to_string())?;
            PropertyValue::number(&typed)
        }
        PropertyValue::Other(_) => None,
    }
}

/// A one-line field for a property. Returns what you typed once you press
/// Enter or leave the field and it differs from `value`.
fn text_field(ui: &mut Ui, note: &mut OpenNote, key: &str, value: &str) -> Option<String> {
    let mut draft = note
        .drafts
        .get(key)
        .cloned()
        .unwrap_or_else(|| value.to_owned());
    let response = ui
        .add(TextEdit::singleline(&mut draft).desired_width(f32::INFINITY))
        .accessible_name(key.to_owned());
    if response.changed() {
        note.drafts.insert(key.to_owned(), draft);
    }
    if !response.lost_focus() {
        return None;
    }
    let escape = ui.input(|input| input.key_pressed(Key::Escape));
    let typed = note.drafts.remove(key)?;
    (!escape && typed != value).then_some(typed)
}

/// The prompt when a note couldn't save before something closed: Try
/// again, Keep editing or Discard changes. Escape keeps editing.
pub(crate) fn unsaved_prompt(ui: &mut Ui, app: &mut App) {
    let Some(prompt) = app.unsaved_prompt().cloned() else {
        return;
    };
    let palette = Palette::of(ui.ctx());
    let mut retry = false;
    let mut keep = false;
    let mut discard = false;
    let modal = egui::Modal::new(Id::unique("unsaved-note")).show(ui.ctx(), |ui| {
        let builder = UiBuilder::new()
            .accessibility_role(Role::AlertDialog)
            .accessibility_label(prompt.title.clone());
        ui.scope_builder(builder, |ui| {
            ui.set_width(360.0);
            ui.label(RichText::new(&prompt.title).size(TITLE_SIZE).strong());
            ui.label(&prompt.message);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let discard_button = ui.add(Button::new(
                    RichText::new("Discard changes").color(palette.destructive),
                ));
                let keep_button = ui.button("Keep editing");
                let try_again = ui.add(
                    Button::new(RichText::new("Try again").color(palette.primary_foreground))
                        .fill(palette.primary),
                );
                // The keyboard stays in the prompt, starting on Try again,
                // so nothing typed reaches the note behind it.
                let ids = [discard_button.id, keep_button.id, try_again.id];
                if !ui.memory(|memory| memory.focused().is_some_and(|id| ids.contains(&id))) {
                    try_again.request_focus();
                }
                discard = discard_button.clicked();
                keep = keep_button.clicked();
                retry = try_again.clicked();
            });
        });
    });
    if retry {
        app.retry_leaving();
    } else if discard {
        app.discard_and_leave();
    } else if keep || modal.should_close() {
        app.keep_editing();
    }
}
