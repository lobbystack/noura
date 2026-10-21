//! Quick open and search, drawn as the palette overlay's two modes: a
//! field, search's type, folder and tag filters, and the files that
//! match. `app-core` keeps the query, the filters, the matches and the
//! highlighted row; Enter opens the highlighted file the way the file
//! tree opens one.

use app_core::{App, Palette, SearchHit, SearchState};
use egui::{
    Align2, Color32, Event, EventFilter, FontId, Frame, Id, Key, Margin, Modal, Modifiers, Pos2,
    Rect, RichText, ScrollArea, Sense, TextEdit, TextWrapMode, Ui, UiBuilder, WidgetInfo,
    WidgetText,
    accesskit::{Action, Role},
    pos2, vec2,
};

use crate::theme::{BODY_SIZE, Palette as Theme, SUPPORTING_SIZE, radius};

const WIDTH: f32 = 560.0;
const LIST_HEIGHT: f32 = 340.0;
/// A quick open row: the name, with its folder beside it.
const FILE_ROW_HEIGHT: f32 = 32.0;
/// A search row: the title and type, then the folder and matching text.
const HIT_ROW_HEIGHT: f32 = 48.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    QuickOpen,
    Search,
}

impl Mode {
    fn of(palette: &Palette) -> Self {
        match palette {
            Palette::QuickOpen(_) => Self::QuickOpen,
            Palette::Search(_) => Self::Search,
        }
    }
}

/// What the palette remembers between frames.
#[derive(Default)]
pub(crate) struct FinderView {
    /// The mode the palette showed last frame, if it showed.
    shown: Option<Mode>,
    /// The widget that had the keyboard before the palette opened.
    return_focus: Option<Id>,
    /// Scroll the highlighted row into view on the next frame.
    reveal: bool,
}

fn field_id() -> Id {
    Id::unique("finder-field")
}

fn row_id(index: usize) -> Id {
    Id::unique(("finder-row", index))
}

/// Mod-O opens quick open, and Mod-Shift-F opens search. Text typed in
/// the same frame, right after the shortcut, goes into the palette's field
/// rather than to the widget that had the keyboard.
pub(crate) fn shortcuts(ui: &Ui, app: &mut App) {
    let pressed = ui.input_mut(|input| {
        let mut mode = None;
        let mut typed = String::new();
        input.events.retain(|event| match event {
            Event::Key {
                key: Key::O,
                pressed: true,
                modifiers,
                ..
            } if modifiers.matches_exact(Modifiers::COMMAND) => {
                mode = Some(Mode::QuickOpen);
                typed.clear();
                false
            }
            Event::Key {
                key: Key::F,
                pressed: true,
                modifiers,
                ..
            } if modifiers.matches_exact(Modifiers::COMMAND | Modifiers::SHIFT) => {
                mode = Some(Mode::Search);
                typed.clear();
                false
            }
            Event::Text(text) if mode.is_some() => {
                typed.push_str(text);
                false
            }
            _ => true,
        });
        mode.map(|mode| (mode, typed))
    });
    let Some((mode, typed)) = pressed else {
        return;
    };
    match mode {
        Mode::QuickOpen => app.open_quick_open(),
        Mode::Search => app.open_search(),
    }
    if !typed.is_empty() {
        app.set_palette_query(&typed);
    }
}

/// The keys the palette takes: Up and Down move the highlight and Enter
/// opens it while the field has the keyboard, and Escape closes the
/// palette.
enum PaletteKey {
    Move(isize),
    Open,
    Close,
}

fn take_keys(ui: &Ui, from_field: bool) -> Vec<PaletteKey> {
    ui.input_mut(|input| {
        let mut taken = Vec::new();
        input.events.retain(|event| {
            let Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            else {
                return true;
            };
            if modifiers.any() {
                return true;
            }
            let found = match key {
                Key::ArrowDown if from_field => PaletteKey::Move(1),
                Key::ArrowUp if from_field => PaletteKey::Move(-1),
                Key::Enter if from_field => PaletteKey::Open,
                Key::Escape => PaletteKey::Close,
                _ => return true,
            };
            taken.push(found);
            false
        });
        taken
    })
}

/// Draw the palette while it's open, and give the keyboard back to where
/// it was once it closes.
pub(crate) fn show(ui: &mut Ui, app: &mut App, view: &mut FinderView) {
    let ctx = ui.ctx().clone();
    let Some(mode) = app.palette().map(Mode::of) else {
        if view.shown.take().is_some() {
            give_back_keyboard(&ctx, view);
        }
        return;
    };
    if view.shown.is_none() {
        view.return_focus = ctx.memory(|memory| memory.focused());
    }
    let opening = view.shown != Some(mode);
    view.shown = Some(mode);

    let theme = Theme::of(&ctx);
    let (name, field_name) = match mode {
        Mode::QuickOpen => ("Quick open", "Find a file by name"),
        Mode::Search => ("Search", "Search the workspace"),
    };
    let mut close = false;
    let mut open = false;
    let mut clicked = None;
    let area =
        Modal::default_area(Id::unique("finder")).anchor(Align2::CENTER_TOP, vec2(0.0, 72.0));
    let modal = Modal::new(Id::unique("finder"))
        .area(area)
        .frame(
            Frame::popup(&ctx.global_style())
                .fill(theme.popover)
                .corner_radius(radius(1.4))
                .inner_margin(Margin::same(8)),
        )
        .show(&ctx, |ui| {
            ui.set_width(WIDTH);
            let builder = UiBuilder::new()
                .accessibility_role(Role::Dialog)
                .accessibility_label(name);
            ui.scope_builder(builder, |ui| {
                let from_field = ui.memory(|memory| memory.has_focus(field_id()));
                for key in take_keys(ui, from_field) {
                    match key {
                        PaletteKey::Move(offset) => {
                            app.move_palette_selection(offset);
                            view.reveal = true;
                        }
                        PaletteKey::Open => open = true,
                        PaletteKey::Close => close = true,
                    }
                }
                let mut query = app
                    .palette()
                    .map(|palette| palette.query().to_owned())
                    .unwrap_or_default();
                let field = ui
                    .add(
                        TextEdit::singleline(&mut query)
                            .id(field_id())
                            .hint_text(field_name)
                            .desired_width(f32::INFINITY),
                    )
                    .accessible_name(field_name);
                if opening {
                    field.request_focus();
                }
                // Up and Down move the highlight, not the keyboard focus.
                ui.memory_mut(|memory| {
                    memory.set_focus_lock_filter(
                        field.id,
                        EventFilter {
                            tab: false,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: false,
                        },
                    );
                });
                if field.changed() {
                    app.set_palette_query(&query);
                }
                if mode == Mode::Search {
                    filters(ui, app);
                }
                ui.add_space(6.0);
                clicked = results(ui, app, view, &theme);
            });
        });
    if modal.should_close() {
        close = true;
    }

    if let Some(row) = clicked {
        app.select_palette_row(row);
        open = true;
    }
    if open {
        app.open_palette_selection();
    } else if close {
        app.close_palette();
    }
    if app.palette().is_none() {
        // Give the keyboard back now, so the next frame's keys go there.
        view.shown = None;
        give_back_keyboard(&ctx, view);
    }
}

fn give_back_keyboard(ctx: &egui::Context, view: &mut FinderView) {
    match view.return_focus.take() {
        Some(id) => ctx.memory_mut(|memory| memory.request_focus(id)),
        None => ctx.memory_mut(|memory| memory.surrender_focus(field_id())),
    }
}

/// Search's filters: a radio button per object type the index holds, and
/// fields for a folder and a tag.
fn filters(ui: &mut Ui, app: &mut App) {
    let Some(Palette::Search(search)) = app.palette() else {
        return;
    };
    let types = search.types.clone();
    let mut chosen_type = search.query.object_type.clone();
    let mut folder = search.query.folder.clone();
    let mut tag = search.query.tag.clone();
    let before = chosen_type.clone();
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Type").size(SUPPORTING_SIZE));
        ui.radio_value(&mut chosen_type, None, "Any type");
        for object_type in &types {
            ui.radio_value(
                &mut chosen_type,
                Some(object_type.clone()),
                type_label(object_type),
            );
        }
    });
    let mut folder_changed = false;
    let mut tag_changed = false;
    ui.horizontal(|ui| {
        let width = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
        folder_changed = ui
            .add(
                TextEdit::singleline(&mut folder)
                    .hint_text("Any folder")
                    .desired_width(width),
            )
            .accessible_name("Folder")
            .changed();
        tag_changed = ui
            .add(
                TextEdit::singleline(&mut tag)
                    .hint_text("Any tag")
                    .desired_width(width),
            )
            .accessible_name("Tag")
            .changed();
    });
    if chosen_type != before {
        app.set_search_type(chosen_type.as_deref());
    }
    if folder_changed {
        app.set_search_folder(&folder);
    }
    if tag_changed {
        app.set_search_tag(&tag);
    }
}

/// An object type as the palette shows it: `task` reads "Task".
fn type_label(object_type: &str) -> String {
    let mut letters = object_type.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
}

/// What the palette lists: the matching files, or why there are none.
/// Returns the row you clicked.
fn results(ui: &mut Ui, app: &App, view: &mut FinderView, theme: &Theme) -> Option<usize> {
    let palette = app.palette()?;
    let selected = palette.selected();
    match palette {
        Palette::QuickOpen(quick_open) if quick_open.query.trim().is_empty() => {
            note(ui, theme, "Type part of a file's name.");
            None
        }
        Palette::QuickOpen(quick_open) if quick_open.matches.is_empty() => {
            note(ui, theme, "No matches");
            None
        }
        Palette::QuickOpen(quick_open) => {
            let rows: Vec<Row> = quick_open
                .matches
                .iter()
                .map(|found| Row {
                    label: found.name.clone(),
                    detail: found.folder.clone(),
                    second_line: None,
                    description: found.folder.clone(),
                })
                .collect();
            list(ui, view, theme, "Files", &rows, selected, FILE_ROW_HEIGHT)
        }
        Palette::Search(search) => match &search.state {
            SearchState::Idle => {
                note(
                    ui,
                    theme,
                    "Type a word to search titles, file names, properties and note text.",
                );
                None
            }
            SearchState::Searching => {
                note(ui, theme, "Searching\u{2026}");
                None
            }
            SearchState::Done(hits) if hits.is_empty() => {
                note(ui, theme, "No matches");
                None
            }
            SearchState::Done(hits) => {
                let rows: Vec<Row> = hits.iter().map(hit_row).collect();
                list(ui, view, theme, "Results", &rows, selected, HIT_ROW_HEIGHT)
            }
            SearchState::Failed(error) => {
                let builder = UiBuilder::new()
                    .accessibility_role(Role::Alert)
                    .accessibility_label(&error.message);
                ui.scope_builder(builder, |ui| {
                    ui.label(
                        RichText::new(&error.message)
                            .size(SUPPORTING_SIZE)
                            .color(theme.destructive),
                    );
                });
                None
            }
        },
    }
}

/// Supporting text in place of the list.
fn note(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.label(
        RichText::new(text)
            .size(SUPPORTING_SIZE)
            .color(theme.muted_foreground),
    );
}

/// One row of the list as the palette draws it.
struct Row {
    label: String,
    /// Muted text beside the label: the folder, or the object type.
    detail: String,
    /// A search hit's folder and matching text, under the label.
    second_line: Option<String>,
    /// What a screen reader hears after the label.
    description: String,
}

fn hit_row(hit: &SearchHit) -> Row {
    let object_type = hit.object_type.as_deref().map(type_label);
    let second_line = [hit.folder.as_str(), hit.snippet.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" \u{b7} ");
    let description = [
        object_type.as_deref().unwrap_or(""),
        hit.folder.as_str(),
        hit.snippet.trim(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(". ");
    Row {
        label: hit.title.clone(),
        detail: object_type.unwrap_or_default(),
        second_line: Some(second_line),
        description,
    }
}

/// The rows as a list box with the highlighted row selected. Returns the
/// row you clicked.
fn list(
    ui: &mut Ui,
    view: &mut FinderView,
    theme: &Theme,
    name: &str,
    rows: &[Row],
    selected: usize,
    height: f32,
) -> Option<usize> {
    let mut clicked = None;
    let builder = UiBuilder::new()
        .accessibility_role(Role::ListBox)
        .accessibility_label(name);
    ui.scope_builder(builder, |ui| {
        ScrollArea::vertical()
            .id_salt(("finder-list", name))
            .max_height(LIST_HEIGHT)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (index, row) in rows.iter().enumerate() {
                    let (rect, _) =
                        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
                    let id = row_id(index);
                    let response = ui.interact(rect, id, Sense::CLICK);
                    let is_selected = index == selected;
                    response
                        .widget_info(|| WidgetInfo::labeled(Role::ListBoxOption, true, &row.label));
                    ui.ctx().accesskit_node_builder(id, |node| {
                        node.set_selected(is_selected);
                        node.add_action(Action::Click);
                        if !row.description.is_empty() {
                            node.set_description(row.description.clone());
                        }
                    });
                    if is_selected && std::mem::take(&mut view.reveal) {
                        response.scroll_to_me(None);
                    }
                    if response.clicked() {
                        clicked = Some(index);
                    }
                    if ui.is_rect_visible(rect) {
                        paint_row(ui, rect, row, is_selected || response.hovered(), theme);
                    }
                }
            });
    });
    clicked
}

fn paint_row(ui: &Ui, rect: Rect, row: &Row, highlighted: bool, theme: &Theme) {
    let painter = ui.painter();
    if highlighted {
        painter.rect_filled(rect, radius(0.6), theme.accent);
    }
    let inner = rect.shrink2(vec2(10.0, 0.0));
    let detail = (!row.detail.is_empty())
        .then(|| line(ui, &row.detail, SUPPORTING_SIZE, inner.width() * 0.4));
    let detail_width = detail.as_ref().map_or(0.0, |galley| galley.size().x + 12.0);
    let label = line(ui, &row.label, BODY_SIZE, inner.width() - detail_width);
    let (label_y, second_y) = match row.second_line {
        Some(_) => (inner.top() + 14.0, inner.top() + 33.0),
        None => (inner.center().y, inner.center().y),
    };
    paint_line(
        ui,
        label,
        pos2(inner.left(), label_y),
        Align2::LEFT_CENTER,
        theme.foreground,
    );
    if let Some(detail) = detail {
        paint_line(
            ui,
            detail,
            pos2(inner.right(), label_y),
            Align2::RIGHT_CENTER,
            theme.muted_foreground,
        );
    }
    if let Some(second) = row.second_line.as_deref().filter(|text| !text.is_empty()) {
        let galley = line(ui, second, SUPPORTING_SIZE, inner.width());
        paint_line(
            ui,
            galley,
            pos2(inner.left(), second_y),
            Align2::LEFT_CENTER,
            theme.muted_foreground,
        );
    }
}

/// `text` on one line, cut short with an ellipsis past `width`.
fn line(ui: &Ui, text: &str, size: f32, width: f32) -> std::sync::Arc<egui::Galley> {
    WidgetText::from(text).into_galley(
        ui,
        Some(TextWrapMode::Truncate),
        width.max(0.0),
        FontId::proportional(size),
    )
}

fn paint_line(
    ui: &Ui,
    galley: std::sync::Arc<egui::Galley>,
    at: Pos2,
    align: Align2,
    color: Color32,
) {
    let rect = align.anchor_size(at, galley.size());
    ui.painter().galley(rect.min, galley, color);
}
