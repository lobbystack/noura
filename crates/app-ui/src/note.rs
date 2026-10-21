//! The note screen: the note's name, a notice when a save failed or the
//! properties at the top don't parse, the editor, and the properties panel
//! beside it: the kernel's for Noura's own notes, or the one a plugin draws
//! for an object of its own type. The editor shows notes in the live
//! preview, and plain-text and code files and notes with broken properties
//! as source.

use std::{collections::HashMap, time::Instant};

use app_core::{App, ConflictChoice, PluginView, Property, PropertyValue, TabId};
use editor_egui::{Editor, MdConfig};
use egui::{
    Button, Event, Frame, Grid, Id, Key, Margin, Modifiers, Panel, RichText, ScrollArea, Stroke,
    TextEdit, Ui, UiBuilder,
    accesskit::{Action, NodeId, Role},
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
    /// The note this frame drew.
    drawn: Option<TabId>,
}

/// One note's editor and the fields around it.
struct OpenNote {
    editor: Editor,
    /// Whether `editor` shows the file's whole text rather than a managed
    /// note's text after its frontmatter.
    whole_file: bool,
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

    /// Drop the editors of tabs that closed, and start a frame that draws
    /// no note yet.
    pub(crate) fn retain(&mut self, app: &App) {
        self.notes
            .retain(|tab, _| app.tabs().iter().any(|open| open.id() == *tab));
        self.drawn = None;
    }

    /// Whether a name or property field holds typing that hasn't gone to
    /// the note yet.
    pub(crate) fn has_drafts(&self) -> bool {
        self.notes
            .values()
            .any(|note| note.name.is_some() || !note.drafts.is_empty())
    }

    /// Hand what the name and property fields hold to their notes, as
    /// leaving the fields does: the name renames the file, and each
    /// property's value goes in. A command, closing the window and quitting
    /// do this first, since they can close a note before its fields lose
    /// the keyboard.
    pub(crate) fn commit_drafts(&mut self, app: &mut App) {
        for (tab, note) in &mut self.notes {
            commit(app, *tab, note);
        }
    }

    /// Hand the typing in the fields of the notes this frame didn't draw
    /// to their notes, as after a switch to another tab while you typed in
    /// one: egui reports no lost focus for a field it doesn't draw.
    pub(crate) fn commit_hidden_drafts(&mut self, app: &mut App) {
        let drawn = self.drawn;
        for (tab, note) in &mut self.notes {
            if Some(*tab) != drawn {
                commit(app, *tab, note);
            }
        }
    }

    /// Hand the typing in a field to its note when this frame's pointer
    /// press, or a click from assistive technology, lands anywhere else,
    /// as leaving the field does. The window calls this before any view
    /// handles the frame's input: egui reports the lost keyboard only when
    /// the field draws again, and a click on the tab's close button or in
    /// the file tree can close the note first.
    pub(crate) fn commit_drafts_left(&mut self, ctx: &egui::Context, app: &mut App) {
        let (pressed, clicked) = ctx.input(|input| {
            let clicked: Vec<NodeId> = input
                .events
                .iter()
                .filter_map(|event| match event {
                    Event::AccessKitActionRequest(request) if request.action == Action::Click => {
                        Some(request.target_node)
                    }
                    _ => None,
                })
                .collect();
            (input.pointer.any_pressed(), clicked)
        });
        if !pressed && clicked.is_empty() {
            return;
        }
        let left = |id: Id| {
            let pressed_elsewhere = pressed
                && !ctx
                    .read_response(id)
                    .is_some_and(|response| response.contains_pointer());
            pressed_elsewhere || clicked.iter().any(|node| *node != id.accesskit_id())
        };
        for (tab, note) in &mut self.notes {
            if note.name.is_some() && left(name_field_id(*tab)) {
                commit_name(app, *tab, note);
            }
            let keys: Vec<String> = note
                .drafts
                .keys()
                .filter(|key| left(property_field_id(*tab, key)))
                .cloned()
                .collect();
            for key in keys {
                commit_property(app, *tab, note, &key);
            }
        }
    }
}

/// Hand the typing in the note's name and property fields to the note.
fn commit(app: &mut App, tab: TabId, note: &mut OpenNote) {
    commit_name(app, tab, note);
    let keys: Vec<String> = note.drafts.keys().cloned().collect();
    for key in keys {
        commit_property(app, tab, note, &key);
    }
}

/// Rename the note's file to what its name field holds.
fn commit_name(app: &mut App, tab: TabId, note: &mut OpenNote) {
    if let Some(typed) = note.name.take() {
        app.rename_note(tab, &typed);
    }
}

/// Give the property `key` the value its field holds.
fn commit_property(app: &mut App, tab: TabId, note: &mut OpenNote, key: &str) {
    let Some(typed) = note.drafts.remove(key) else {
        return;
    };
    let property = app
        .note(tab)
        .and_then(|session| session.properties())
        .and_then(|properties| properties.into_iter().find(|property| property.key == key));
    if let Some(value) = property.and_then(|property| typed_value(&property, &typed)) {
        app.set_note_property(tab, key, value);
    }
}

/// The value `typed` gives the property, or `None` when the property
/// keeps its value: `typed` is the value it has, or no number where the
/// property holds one.
fn typed_value(property: &Property, typed: &str) -> Option<PropertyValue> {
    if property.read_only {
        return None;
    }
    match &property.value {
        PropertyValue::Text(text) => (typed != text).then(|| PropertyValue::Text(typed.to_owned())),
        PropertyValue::Number(number) => (typed != number.to_string())
            .then(|| PropertyValue::number(typed))
            .flatten(),
        _ => None,
    }
}

fn name_field_id(tab: TabId) -> Id {
    Id::unique(("note-name", tab))
}

fn property_field_id(tab: TabId, key: &str) -> Id {
    Id::unique(("note-property", tab, key))
}

/// Draw the note in `tab`, or why it couldn't open. Returns whether the tab
/// shows a note.
pub(crate) fn show(ui: &mut Ui, app: &mut App, view: &mut NoteView, tab: TabId) -> bool {
    if let Some(error) = app.note_unreadable(tab) {
        let message = error.message.clone();
        let mut retry = false;
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            muted(ui, &message);
            ui.add_space(8.0);
            retry = ui.button("Try again").clicked();
        });
        if retry {
            app.retry_note(tab);
        }
        return true;
    }
    let Some(session) = app.note(tab) else {
        return false;
    };
    let shows_source = session.shows_source();
    let whole_file = session.shows_whole_file();
    let note = view.notes.entry(tab).or_insert_with(|| {
        let clicked = ui.input(|input| input.pointer.any_released());
        OpenNote {
            editor: editor_for(ui.ctx(), session),
            whole_file,
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
    // The file shows another way now, as when a save left a note's
    // properties readable or a rename made a text file Markdown: the editor
    // starts again from the note's text, with an empty undo history, and
    // keeps the keyboard.
    if (note.editor.plaintext_mode() != shows_source || note.whole_file != whole_file)
        && let Some(session) = app.note(tab)
    {
        let focused = note.editor.focused(ui.ctx());
        note.editor = editor_for(ui.ctx(), session);
        note.whole_file = whole_file;
        if focused {
            note.editor.focus(ui.ctx());
        }
        app.take_note_sync(tab);
        app.take_note_reset(tab);
    }
    // A save brought the file in line with the editor, or merged an edit
    // made outside Noura.
    if let Some(sync) = app.take_note_sync(tab) {
        note.editor.record_disk_save(sync.saved.0, sync.saved.1);
        if let Some(merged) = sync.merged {
            note.editor.apply_disk_version(merged);
        }
    }
    // Discard changes dropped the edits that weren't on disk: the editor
    // takes the file's text as an outside edit of everything it shows.
    if let Some(text) = app.take_note_reset(tab) {
        let (version, shown) = note.editor.flush_for_save();
        note.editor.record_disk_save(version, shown);
        note.editor.apply_disk_version(text);
    }
    view.drawn = Some(tab);
    header(ui, app, note, tab);
    properties_notice(ui, app, tab);
    file_notice(ui, app, tab);
    let palette = Palette::of(ui.ctx());
    let panel = Panel::right(("note-properties", tab))
        .resizable(true)
        .default_size(PROPERTIES_WIDTH)
        .frame(
            Frame::new()
                .fill(palette.background)
                .stroke(Stroke::new(1.0, palette.border))
                .inner_margin(Margin::same(12)),
        );
    // An object of a running plugin's type shows the plugin's panel in
    // place of the kernel's.
    if let Some(view) = app.plugin_panel(tab) {
        panel.show(ui, |ui| plugin_panel(ui, app, tab, &view));
    } else if let Some(properties) = app.note(tab).and_then(|session| session.properties()) {
        panel.show(ui, |ui| properties_panel(ui, app, note, tab, &properties));
    }
    body(ui, app, note, tab);
    // The prompt before leaving the note asks first.
    if app.unsaved_prompt().is_none() {
        conflict_review(ui, app, tab);
    }
    true
}

/// An editor for the note `session` shows: its text as source for a
/// plain-text or code file and a note whose properties don't parse, or in
/// the live preview with the formatting toolbar.
fn editor_for(ctx: &egui::Context, session: &app_core::NoteSession) -> Editor {
    let config = if session.shows_source() {
        // Any extension but `md` shows the text as source.
        MdConfig {
            ext: "txt".into(),
            ..MdConfig::default()
        }
    } else {
        MdConfig {
            toolbar: true,
            ..MdConfig::default()
        }
    };
    Editor::new(ctx, session.text(), config)
}

/// Why the properties at the top of a Markdown file don't parse, above
/// its text, where you fix them.
fn properties_notice(ui: &mut Ui, app: &App, tab: TabId) {
    let Some(reason) = app
        .note(tab)
        .and_then(|session| session.broken_properties())
    else {
        return;
    };
    let title = "The properties at the top of this file have an error";
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new()
        .accessibility_role(Role::Alert)
        .accessibility_label(title);
    ui.scope_builder(builder, |ui| {
        Frame::new()
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(radius(1.0))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(title).strong());
                ui.label(format!("{reason} Fix them in the text below."));
            });
    });
    ui.add_space(8.0);
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

/// What keeps the note's edits off the disk: another app's edit of the
/// same text, a deleted file, or a failed save.
fn file_notice(ui: &mut Ui, app: &mut App, tab: TabId) {
    let Some(session) = app.note(tab) else {
        return;
    };
    if session.conflict().is_some() {
        let compare = banner(
            ui,
            "This file changed in another app",
            "Your text is still here. Compare both versions and choose one.",
            None,
            "Compare",
        );
        if compare {
            app.review_conflict(tab);
        }
    } else if session.is_deleted() {
        let problem = session.restore_problem().map(str::to_owned);
        let restore = banner(
            ui,
            "This file was deleted",
            "Your text is still here. You can put the file back.",
            problem.as_deref(),
            "Restore",
        );
        if restore {
            app.restore_note(tab);
        }
    } else {
        save_notice(ui, app, tab);
    }
}

/// A notice in the note with one button. Returns whether it was clicked.
fn banner(ui: &mut Ui, title: &str, text: &str, problem: Option<&str>, button: &str) -> bool {
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new()
        .accessibility_role(Role::Alert)
        .accessibility_label(title);
    let clicked = ui
        .scope_builder(builder, |ui| {
            Frame::new()
                .stroke(Stroke::new(1.0, palette.border))
                .corner_radius(radius(1.0))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(title).strong());
                    ui.label(text);
                    if let Some(problem) = problem {
                        ui.label(RichText::new(problem).color(palette.destructive));
                    }
                    ui.button(button).clicked()
                })
                .inner
        })
        .inner;
    ui.add_space(8.0);
    clicked
}

/// The conflict review: your version and the file's side by side, with
/// Keep editing, Use the file's version and Use my version. Each choice
/// asks for confirmation first. Escape keeps editing.
fn conflict_review(ui: &mut Ui, app: &mut App, tab: TabId) {
    let Some(conflict) = app
        .note(tab)
        .and_then(|session| session.conflict())
        .filter(|conflict| conflict.is_reviewing())
    else {
        return;
    };
    let confirming = conflict.confirming();
    let problem = conflict.problem().map(str::to_owned);
    let theirs = conflict.file_text().to_owned();
    let mine = app.note(tab).map(|session| session.text().to_owned());
    let mine = mine.unwrap_or_default();
    let palette = Palette::of(ui.ctx());
    let mut chosen = None;
    let mut keep = false;
    let mut cancel = false;
    let mut confirm = false;
    let modal = egui::Modal::new(Id::unique(("conflict-review", tab))).show(ui.ctx(), |ui| {
        if let Some(choice) = confirming {
            let (title, text) = match choice {
                ConflictChoice::Mine => (
                    "Use your version?",
                    "The file\u{2019}s version is kept as a backup first.",
                ),
                ConflictChoice::File => (
                    "Use the file\u{2019}s version?",
                    "Your version is kept as a backup first.",
                ),
            };
            let builder = UiBuilder::new()
                .accessibility_role(Role::AlertDialog)
                .accessibility_label(title);
            ui.scope_builder(builder, |ui| {
                ui.set_width(360.0);
                ui.label(RichText::new(title).size(TITLE_SIZE).strong());
                ui.label(text);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let cancel_button = ui.button("Cancel");
                    let continue_button = ui.add(
                        Button::new(RichText::new("Continue").color(palette.primary_foreground))
                            .fill(palette.primary),
                    );
                    let ids = [cancel_button.id, continue_button.id];
                    if !ui.memory(|memory| memory.focused().is_some_and(|id| ids.contains(&id))) {
                        continue_button.request_focus();
                    }
                    cancel = cancel_button.clicked();
                    confirm = continue_button.clicked();
                });
            });
            return;
        }
        let builder = UiBuilder::new()
            .accessibility_role(Role::Dialog)
            .accessibility_label("Choose a version");
        ui.scope_builder(builder, |ui| {
            let width = (ui.ctx().content_rect().width() - 96.0).clamp(360.0, 960.0);
            ui.set_width(width);
            ui.label(RichText::new("Choose a version").size(TITLE_SIZE).strong());
            ui.label("Both versions are kept until you choose.");
            if let Some(problem) = &problem {
                ui.label(RichText::new(problem).color(palette.destructive));
            }
            ui.add_space(8.0);
            let height = (ui.ctx().content_rect().height() * 0.5).max(160.0);
            ui.columns(2, |columns| {
                version(
                    &mut columns[0],
                    ("conflict-mine", tab),
                    "Your version",
                    &mine,
                    height,
                );
                version(
                    &mut columns[1],
                    ("conflict-file", tab),
                    "The file\u{2019}s version",
                    &theirs,
                    height,
                );
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let keep_button = ui.button("Keep editing");
                let file_button = ui.button("Use the file\u{2019}s version");
                let mine_button = ui.add(
                    Button::new(RichText::new("Use my version").color(palette.primary_foreground))
                        .fill(palette.primary),
                );
                // The keyboard stays in the review, starting on Keep
                // editing, so nothing typed reaches the note behind it.
                let ids = [keep_button.id, file_button.id, mine_button.id];
                if !ui.memory(|memory| memory.focused().is_some_and(|id| ids.contains(&id))) {
                    keep_button.request_focus();
                }
                keep = keep_button.clicked();
                if file_button.clicked() {
                    chosen = Some(ConflictChoice::File);
                }
                if mine_button.clicked() {
                    chosen = Some(ConflictChoice::Mine);
                }
            });
        });
    });
    let escape = modal.should_close();
    if confirm {
        app.confirm_version(tab);
    } else if cancel || (escape && confirming.is_some()) {
        app.cancel_version_choice(tab);
    } else if let Some(choice) = chosen {
        app.choose_version(tab, choice);
    } else if keep || escape {
        app.close_conflict_review(tab);
    }
}

/// One side of the conflict review: a heading and the text, read-only.
fn version(ui: &mut Ui, id: (&str, TabId), heading: &str, text: &str, height: f32) {
    ui.label(RichText::new(heading).strong());
    ScrollArea::vertical()
        .id_salt(id)
        .max_height(height)
        .show(ui, |ui| {
            ui.add(
                TextEdit::multiline(&mut &*text)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            )
            .accessible_name(heading);
        });
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

/// The properties panel the plugin that holds the object's type draws.
fn plugin_panel(ui: &mut Ui, app: &mut App, tab: TabId, view: &PluginView) {
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label("Properties");
    ui.scope_builder(builder, |ui| {
        ScrollArea::vertical()
            .id_salt(("note-plugin-panel-scroll", tab))
            .show(ui, |ui| crate::plugin_view::show_panel(ui, app, tab, view));
    });
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
                            if let Some(value) = property_value(ui, note, tab, property) {
                                app.set_note_property(tab, &property.key, value);
                            }
                            ui.end_row();
                        }
                    });
            });
    });
}

/// Draw one property's value. Returns the new value once an edit is done.
fn property_value(
    ui: &mut Ui,
    note: &mut OpenNote,
    tab: TabId,
    property: &Property,
) -> Option<PropertyValue> {
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
        PropertyValue::Text(text) => {
            let typed = text_field(ui, note, tab, key, text)?;
            typed_value(property, &typed)
        }
        PropertyValue::Number(number) => {
            let typed = text_field(ui, note, tab, key, &number.to_string())?;
            typed_value(property, &typed)
        }
        PropertyValue::Other(_) => None,
    }
}

/// A one-line field for a property. Returns what you typed once you press
/// Enter or leave the field, and nothing after Escape.
fn text_field(
    ui: &mut Ui,
    note: &mut OpenNote,
    tab: TabId,
    key: &str,
    value: &str,
) -> Option<String> {
    let mut draft = note
        .drafts
        .get(key)
        .cloned()
        .unwrap_or_else(|| value.to_owned());
    let response = ui
        .add(
            TextEdit::singleline(&mut draft)
                .id(property_field_id(tab, key))
                .desired_width(f32::INFINITY),
        )
        .accessible_name(key.to_owned());
    if response.changed() {
        note.drafts.insert(key.to_owned(), draft);
    }
    if !response.lost_focus() {
        return None;
    }
    let escape = ui.input(|input| input.key_pressed(Key::Escape));
    let typed = note.drafts.remove(key)?;
    (!escape).then_some(typed)
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
