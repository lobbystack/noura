//! The palette: one dialog with a field and a list, in three modes. The
//! command palette (Mod-K) filters every command as you type and lists
//! each with its shortcut; quick open (Mod-O) and search (Mod-Shift-F)
//! list files, drawn by `finder.rs`. `app-core` keeps the mode, the
//! query, the matches and the highlighted row; Enter runs the highlighted
//! command through the same dispatch as keys and menus, or opens the
//! highlighted file the way the file tree opens one.

use std::{sync::Arc, time::Instant};

use app_core::{App, Palette as Overlay, PaletteMode, Platform};
use egui::{
    Align2, Color32, Event, EventFilter, FontId, Frame, Galley, Id, Key, Margin, Modal, Pos2, Rect,
    RichText, ScrollArea, Sense, TextEdit, TextWrapMode, Ui, UiBuilder, WidgetInfo, WidgetText,
    accesskit::{Action, Role},
    pos2, vec2,
};

use crate::{
    finder, keys,
    theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, radius},
};

const WIDTH: f32 = 560.0;
const LIST_HEIGHT: f32 = 340.0;
/// A row with one line: a command, or a file name with its folder beside
/// it.
pub(crate) const ROW_HEIGHT: f32 = 32.0;

/// What the palette remembers between frames.
#[derive(Default)]
pub(crate) struct PaletteView {
    /// The mode the palette showed last frame, if it showed.
    shown: Option<PaletteMode>,
    /// The widget that had the keyboard before the palette opened.
    return_focus: Option<Id>,
    /// Scroll the highlighted row into view on the next frame.
    reveal: bool,
}

/// The palette's field, the same in every mode, so it keeps the keyboard
/// when one mode replaces another.
fn field_id() -> Id {
    Id::unique("palette-field")
}

fn row_id(index: usize) -> Id {
    Id::unique(("palette-row", index))
}

/// The dialog's name, its field's name and the field's hint in `mode`.
fn names(mode: PaletteMode) -> (&'static str, &'static str, &'static str) {
    match mode {
        PaletteMode::Commands => ("Command palette", "Search commands", "Type a command"),
        PaletteMode::QuickOpen => ("Quick open", "Find a file by name", "Find a file by name"),
        PaletteMode::Search => ("Search", "Search the workspace", "Search the workspace"),
    }
}

/// The keys the palette takes: Up and Down move the highlight and Enter
/// acts on it while the field has the keyboard, and Escape closes the
/// palette.
enum PaletteKey {
    Move(isize),
    Run,
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
                Key::Enter if from_field => PaletteKey::Run,
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
pub(crate) fn show(ui: &mut Ui, app: &mut App, view: &mut PaletteView) {
    let ctx = ui.ctx().clone();
    let Some(mode) = app.palette().map(Overlay::mode) else {
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

    let theme = Palette::of(&ctx);
    let (name, field_name, hint) = names(mode);
    let mut close = false;
    let mut run = false;
    let mut clicked = None;
    let area =
        Modal::default_area(Id::unique("palette")).anchor(Align2::CENTER_TOP, vec2(0.0, 72.0));
    let modal = Modal::new(Id::unique("palette"))
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
                        PaletteKey::Run => run = true,
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
                            .hint_text(hint)
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
                match mode {
                    PaletteMode::Commands => {
                        ui.add_space(4.0);
                        clicked = commands(ui, app, view, &theme);
                    }
                    PaletteMode::QuickOpen | PaletteMode::Search => {
                        if mode == PaletteMode::Search {
                            finder::filters(ui, app);
                        }
                        ui.add_space(6.0);
                        clicked = finder::results(ui, app, view, &theme);
                    }
                }
            });
        });
    if modal.should_close() {
        close = true;
    }

    if let Some(row) = clicked {
        app.select_palette_row(row);
        run = true;
    }
    if run {
        app.run_palette_selection(Instant::now());
    } else if close {
        app.close_palette();
    }
    if app.palette().is_none() {
        // Give the keyboard back now, so the next frame's keys go there.
        view.shown = None;
        give_back_keyboard(&ctx, view);
    }
}

fn give_back_keyboard(ctx: &egui::Context, view: &mut PaletteView) {
    match view.return_focus.take() {
        Some(id) => ctx.memory_mut(|memory| memory.request_focus(id)),
        None => ctx.memory_mut(|memory| memory.surrender_focus(field_id())),
    }
}

/// The matching commands, each with its shortcut. Returns the row you
/// clicked.
fn commands(ui: &mut Ui, app: &App, view: &mut PaletteView, theme: &Palette) -> Option<usize> {
    let matches = app.palette_matches();
    let palette = app.palette()?;
    if matches.is_empty() {
        let text = format!("No commands match \u{201c}{}\u{201d}.", palette.query());
        note(ui, theme, &text);
        return None;
    }
    let platform: Platform = app.platform();
    let rows: Vec<Row> = matches
        .iter()
        .map(|command| {
            let keys = command
                .shortcuts
                .first()
                .map(|shortcut| keys::shortcut_text(shortcut, platform));
            Row {
                label: command.label.clone(),
                detail: keys.clone().unwrap_or_default(),
                second_line: None,
                description: keys.clone().unwrap_or_default(),
                shortcut: keys,
            }
        })
        .collect();
    list(
        ui,
        view,
        theme,
        "Commands",
        &rows,
        palette.selected(),
        ROW_HEIGHT,
    )
}

/// Supporting text in place of the list.
pub(crate) fn note(ui: &mut Ui, theme: &Palette, text: &str) {
    ui.label(
        RichText::new(text)
            .size(SUPPORTING_SIZE)
            .color(theme.muted_foreground),
    );
}

/// One row of the list as the palette draws it.
pub(crate) struct Row {
    pub(crate) label: String,
    /// Muted text beside the label: a command's shortcut, a file's folder
    /// or a hit's object type.
    pub(crate) detail: String,
    /// A search hit's folder and matching text, under the label.
    pub(crate) second_line: Option<String>,
    /// What a screen reader hears after the label.
    pub(crate) description: String,
    /// The command's shortcut, for screen readers.
    pub(crate) shortcut: Option<String>,
}

/// The rows as a list box named `name`, with the highlighted row
/// selected. Returns the row you clicked.
pub(crate) fn list(
    ui: &mut Ui,
    view: &mut PaletteView,
    theme: &Palette,
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
            .id_salt(("palette-list", name))
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
                        if let Some(shortcut) = &row.shortcut {
                            node.set_keyboard_shortcut(shortcut.clone());
                        }
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

fn paint_row(ui: &Ui, rect: Rect, row: &Row, highlighted: bool, theme: &Palette) {
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
fn line(ui: &Ui, text: &str, size: f32, width: f32) -> Arc<Galley> {
    WidgetText::from(text).into_galley(
        ui,
        Some(TextWrapMode::Truncate),
        width.max(0.0),
        FontId::proportional(size),
    )
}

fn paint_line(ui: &Ui, galley: Arc<Galley>, at: Pos2, align: Align2, color: Color32) {
    let rect = align.anchor_size(at, galley.size());
    ui.painter().galley(rect.min, galley, color);
}
