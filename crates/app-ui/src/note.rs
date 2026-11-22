//! The note screen: the note's name, a notice when a save failed or the
//! properties at the top don't parse, the editor, and the properties panel
//! beside it: the kernel's for Noura's own notes, or the one a plugin draws
//! for an object of its own type. The editor shows notes in the live
//! preview, and plain-text and code files and notes with broken properties
//! as source.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use app_core::sync::workspace::presence::SelectionRange;
use app_core::{App, ConflictChoice, OpenAs, PluginView, TabId};
use editor_egui::{Editor, MdConfig, RemoteCursor, SpellChecker};
use egui::{
    Color32, Event, Frame, Id, Key, Margin, Modifiers, Panel, RichText, ScrollArea, Stroke,
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
    /// The system's spell checker, which notes use while the Check
    /// spelling setting is on.
    pub(crate) spell_checker: Option<Arc<dyn SpellChecker>>,
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
    /// Whether the text opened with a heading that says the note's name,
    /// for the name it was checked against. It is checked when the note
    /// opens and after a rename, never while you type, so the text never
    /// moves under the caret.
    heading_repeats_name: Option<(String, bool)>,
    /// The widget to hand the keyboard back to after the editor's first
    /// frame: a note opened from the keyboard, as with Enter in the file
    /// tree, leaves the keyboard where it was. A click puts the caret in
    /// the text.
    give_focus_back: Option<Id>,
    /// The note path the editor's links resolve from, which a rename
    /// changes.
    links_source: Option<String>,
    /// Whether the editor checks spelling.
    checks_spelling: bool,
    /// The PDFs the note's embeds asked to open in a tab.
    pdf_opens: crate::note_images::PdfOpens,
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

    /// Whether a name field holds typing that hasn't gone to the note yet.
    pub(crate) fn has_drafts(&self) -> bool {
        self.notes.values().any(|note| note.name.is_some())
    }

    /// Hand what the name fields hold to their notes, as leaving the
    /// fields does: the name renames the file. A command, closing the window and quitting
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
        }
    }
}

/// Hand the typing in the note's name field to the note.
fn commit(app: &mut App, tab: TabId, note: &mut OpenNote) {
    commit_name(app, tab, note);
}

/// Rename the note's file to what its name field holds.
fn commit_name(app: &mut App, tab: TabId, note: &mut OpenNote) {
    if let Some(typed) = note.name.take() {
        app.rename_note(tab, &typed);
    }
}

/// Eight colors that stay readable on both themes, for collaborators.
const CURSOR_COLORS: [Color32; 8] = [
    Color32::from_rgb(0xd9, 0x4f, 0x4f),
    Color32::from_rgb(0x2f, 0x8f, 0x5b),
    Color32::from_rgb(0x3b, 0x6f, 0xd9),
    Color32::from_rgb(0xc2, 0x7a, 0x12),
    Color32::from_rgb(0x8e, 0x4f, 0xc9),
    Color32::from_rgb(0x12, 0x8f, 0x9e),
    Color32::from_rgb(0xc9, 0x4f, 0x9a),
    Color32::from_rgb(0x6f, 0x7f, 0x2a),
];

/// The color of a collaborator's cursor, the same on every device.
fn cursor_color(index: u8) -> Color32 {
    CURSOR_COLORS[usize::from(index) % CURSOR_COLORS.len()]
}

fn name_field_id(tab: TabId) -> Id {
    Id::unique(("note-name", tab))
}

/// Draw the note in `tab`, or why it couldn't open, in the window whose ID
/// salt is `window`. Returns whether the tab shows a note.
pub(crate) fn show(
    ui: &mut Ui,
    app: &mut App,
    view: &mut NoteView,
    window: Id,
    tab: TabId,
) -> bool {
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
            heading_repeats_name: None,
            give_focus_back: if clicked {
                None
            } else {
                ui.memory(|memory| memory.focused())
            },
            links_source: None,
            checks_spelling: false,
            pdf_opens: Arc::default(),
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
        note.checks_spelling = false;
        note.links_source = None;
        note.whole_file = whole_file;
        if focused {
            note.editor.focus(ui.ctx());
        }
        app.take_note_sync(tab);
        app.take_note_reset(tab);
    }
    // Links resolve from the note's path, so a renamed or moved note
    // resolves them from where it is now.
    if let Some(links) = app.note_links(tab)
        && note.links_source.as_deref() != Some(links.source())
    {
        note.links_source = Some(links.source().to_owned());
        note.editor
            .set_embed_resolver(Box::new(crate::note_images::WorkspaceImages::new(
                links.clone(),
                ui.ctx().clone(),
                note.pdf_opens.clone(),
            )));
        note.editor
            .set_link_resolver(Box::new(crate::note_links::WorkspaceLinks {
                links,
                ctx: ui.ctx().clone(),
            }));
    }
    // The Check spelling setting reaches every open note.
    let spelling = app
        .preferences()
        .spell_check
        .then(|| view.spell_checker.clone())
        .flatten();
    if spelling.is_some() != note.checks_spelling {
        note.checks_spelling = spelling.is_some();
        note.editor.set_spell_checker(spelling);
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
    // The note's document ended or moved on: what the editor holds goes
    // back to the file, which the document stopped following.
    if note.editor.edit.renderer.in_text_mode() && app.take_note_left(tab) {
        note.editor.edit.renderer.leave_text_mode();
        note.editor.edit.remote_cursors.clear();
    }
    // The note's document arrived: the editor sends it your edits from
    // here on, and shows what it merges as it arrives.
    if !note.editor.edit.renderer.in_text_mode()
        && let Some(document) = app.take_note_document(tab, note.editor.text())
    {
        note.editor.edit.renderer.enter_text_mode(document);
    }
    if note.editor.edit.renderer.in_text_mode() {
        note.editor.edit.renderer.sync_text_mode();
        let (anchor, head) = note.editor.edit.renderer.selection_utf16();
        app.set_note_selections(
            tab,
            vec![SelectionRange {
                anchor: anchor as u32,
                head: head as u32,
            }],
            note.editor.focused(ui.ctx()),
        );
        note.editor.edit.remote_cursors = app
            .note_cursors(tab)
            .iter()
            .flat_map(|cursor| {
                let color = cursor_color(cursor.color);
                let edit = &note.editor.edit;
                cursor.selections.iter().map(move |range| RemoteCursor {
                    anchor: edit.renderer.offset_of_utf16(range.anchor as usize),
                    head: edit.renderer.offset_of_utf16(range.head as usize),
                    color,
                })
            })
            .collect();
        // Remote edits and cursors arrive without any input.
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
    view.drawn = Some(tab);
    if shows_name(ui, app, note, tab) {
        header(ui, app, note, tab);
    }
    properties_notice(ui, app, tab);
    file_notice(ui, app, tab);
    // As in the legacy app, a note shows no properties beside its text. A
    // plugin's object, such as a project, may still show its plugin's
    // panel.
    if let Some(view) = app.plugin_panel(tab) {
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
            .show(ui, |ui| plugin_panel(ui, app, window, tab, &view));
    }
    body(ui, app, note, tab);
    // An embed's Open in tab button opens its PDF at the page its link
    // names.
    let opens = std::mem::take(&mut *note.pdf_opens.lock().unwrap());
    for (path, page) in opens {
        app.open_pdf_page(&path, page, app_core::OpenAs::Kept);
    }
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
    let title = "Properties can\u{2019}t be read";
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
                ui.label(reason);
            });
    });
    ui.add_space(8.0);
}

/// Whether the note's name shows above its text. It doesn't when the text
/// opens with a heading that says the name, as the tab says it too, so the
/// name isn't said three times. It shows again for a rename, as with F2
/// in the text, and while the name has a problem.
fn shows_name(ui: &Ui, app: &App, note: &mut OpenNote, tab: TabId) -> bool {
    let Some(session) = app.note(tab) else {
        return false;
    };
    let name = session.name();
    let repeats = match &note.heading_repeats_name {
        Some((checked, repeats)) if *checked == name => *repeats,
        _ => {
            let repeats = session.heading_repeats_name();
            note.heading_repeats_name = Some((name, repeats));
            repeats
        }
    };
    !repeats
        || note.focus_name
        || note.name.is_some()
        || session.name_problem().is_some()
        || ui.memory(|memory| memory.has_focus(name_field_id(tab)))
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
    // The name on one 24 px line over the toolbar, whose line closes the
    // header, starting where the text starts, as the note's title.
    ui.add_space(-12.0);
    let mut rect = ui.available_rect_before_wrap();
    rect.min.x += text_indent(ui, app, note);
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        let interact = ui.spacing().interact_size.y;
        ui.spacing_mut().interact_size.y = 24.0;
        let response = ui
            .add(
                TextEdit::singleline(&mut draft)
                    .id(id)
                    .hint_text("Untitled")
                    .font(crate::theme::font(
                        ui.ctx(),
                        TITLE_SIZE,
                        crate::theme::Weight::Semibold,
                    ))
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .desired_width(f32::INFINITY),
            )
            .accessible_name("File name");
        ui.spacing_mut().interact_size.y = interact;
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
                // The key ended the field's edit, so the editor, which draws
                // after this and takes the keyboard now, must not see it too.
                ui.input_mut(|input| {
                    input.consume_key(Modifiers::NONE, Key::Enter);
                    input.consume_key(Modifiers::NONE, Key::Escape);
                });
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
    });
    ui.add_space(8.0);
}

/// How far right of the note's pane its text starts: the column the line
/// width setting allows, centered, and the editor's margin inside it.
fn text_indent(ui: &Ui, app: &App, note: &OpenNote) -> f32 {
    let available = ui.available_width();
    let width = app
        .preferences()
        .line_width
        .points()
        .map_or(available, |points| points.min(available));
    (available - width) / 2.0 + note.editor.edit.renderer.layout.margin
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
            "Your text is still here.",
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
            "Your text is still here.",
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

/// A notice in the note with one button, as the legacy alert: a warning
/// icon, its title and text, and the button at its end. Returns whether
/// the button was clicked.
fn banner(ui: &mut Ui, title: &str, text: &str, problem: Option<&str>, button: &str) -> bool {
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new()
        .accessibility_role(Role::Alert)
        .accessibility_label(title);
    let clicked = ui
        .scope_builder(builder, |ui| {
            Frame::new()
                .fill(palette.popover)
                .stroke(Stroke::new(1.0, palette.border))
                .corner_radius(crate::theme::pill())
                .inner_margin(Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(16.0, 20.0), egui::Sense::hover());
                        let icon =
                            egui::Rect::from_center_size(rect.center(), egui::vec2(16.0, 16.0));
                        crate::icons::paint(
                            ui.painter(),
                            crate::icons::WARNING,
                            icon,
                            palette.foreground,
                            false,
                        );
                        let mut clicked = false;
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                            clicked = crate::common::Btn::new(button)
                                .size(crate::common::Size::Sm)
                                .show(ui)
                                .clicked();
                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                crate::common::field_label(ui, title);
                                crate::common::description(ui, text);
                                if let Some(problem) = problem {
                                    ui.label(RichText::new(problem).color(palette.destructive));
                                }
                            });
                        });
                        clicked
                    })
                    .inner
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
    let modal = crate::common::dialog(ui.ctx(), Id::unique(("conflict-review", tab))).show(
        ui.ctx(),
        |ui| {
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
                    crate::common::dialog_title(ui, title);
                    crate::common::description(ui, text);
                    let pressed = crate::common::dialog_footer(
                        ui,
                        vec![
                            crate::common::Btn::new("Cancel"),
                            crate::common::Btn::new("Continue")
                                .variant(crate::common::Variant::Default),
                        ],
                    );
                    let ids = [pressed[0].id, pressed[1].id];
                    if !ui.memory(|memory| memory.focused().is_some_and(|id| ids.contains(&id))) {
                        pressed[1].request_focus();
                    }
                    cancel = pressed[0].clicked();
                    confirm = pressed[1].clicked();
                });
                return;
            }
            let builder = UiBuilder::new()
                .accessibility_role(Role::Dialog)
                .accessibility_label("Choose a version");
            ui.scope_builder(builder, |ui| {
                let width = (ui.ctx().content_rect().width() - 96.0).clamp(360.0, 960.0);
                ui.set_width(width);
                crate::common::dialog_title(ui, "Choose a version");
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
                {
                    let pressed = crate::common::dialog_footer(
                        ui,
                        vec![
                            crate::common::Btn::new("Keep editing"),
                            crate::common::Btn::new("Use the file\u{2019}s version"),
                            crate::common::Btn::new("Use my version")
                                .variant(crate::common::Variant::Default),
                        ],
                    );
                    let (keep_button, file_button, mine_button) =
                        (&pressed[0], &pressed[1], &pressed[2]);
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
                }
            });
        },
    );
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
    // The editor reports itself as the note's text field, "Note text".
    let response = ui.scope_builder(builder, |ui| note.editor.show(ui)).inner;
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
    // A link opens what it leads to as a single click in the tree does.
    // Cmd-click, Ctrl-click and a middle-click open it in a new tab.
    for link in response.opened_links {
        let how = if link.new_tab {
            OpenAs::Kept
        } else {
            OpenAs::Preview
        };
        app.follow_note_link(tab, &link.target, how);
    }
}

/// The properties panel the plugin that holds the object's type draws.
fn plugin_panel(ui: &mut Ui, app: &mut App, window: Id, tab: TabId, view: &PluginView) {
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label("Properties");
    ui.scope_builder(builder, |ui| {
        // A wide view, such as a project's board, scrolls sideways.
        ScrollArea::both()
            .id_salt(("note-plugin-panel-scroll", tab))
            .show(ui, |ui| {
                crate::plugin_view::show_panel(ui, app, window, tab, view);
            });
    });
}

/// The prompt when a note couldn't save before something closed: Try
/// again, Keep editing or Discard changes. Escape keeps editing.
pub(crate) fn unsaved_prompt(ui: &mut Ui, app: &mut App) {
    let Some(prompt) = app.unsaved_prompt().cloned() else {
        return;
    };
    let mut retry = false;
    let mut keep = false;
    let mut discard = false;
    let modal = crate::common::dialog(ui.ctx(), Id::unique("unsaved-note")).show(ui.ctx(), |ui| {
        let builder = UiBuilder::new()
            .accessibility_role(Role::AlertDialog)
            .accessibility_label(prompt.title.clone());
        ui.scope_builder(builder, |ui| {
            ui.set_width(360.0);
            crate::common::dialog_title(ui, &prompt.title);
            crate::common::description(ui, &prompt.message);
            {
                let pressed = crate::common::dialog_footer(
                    ui,
                    vec![
                        crate::common::Btn::new("Discard changes")
                            .variant(crate::common::Variant::Destructive),
                        crate::common::Btn::new("Keep editing"),
                        crate::common::Btn::new("Try again")
                            .variant(crate::common::Variant::Default),
                    ],
                );
                let (discard_button, keep_button, try_again) =
                    (&pressed[0], &pressed[1], &pressed[2]);
                // The keyboard stays in the prompt, starting on Try again,
                // so nothing typed reaches the note behind it.
                let ids = [discard_button.id, keep_button.id, try_again.id];
                if !ui.memory(|memory| memory.focused().is_some_and(|id| ids.contains(&id))) {
                    try_again.request_focus();
                }
                discard = discard_button.clicked();
                keep = keep_button.clicked();
                retry = try_again.clicked();
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collaborators_get_distinct_colors_that_stay_with_their_device() {
        let colors: std::collections::HashSet<_> = (0..8).map(cursor_color).collect();
        assert_eq!(colors.len(), 8);
        assert_eq!(cursor_color(3), cursor_color(3 + 8));
    }
}
