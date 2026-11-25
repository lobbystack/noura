//! Search: one dialog with one field, like Spotlight. As you type it
//! lists files by name, files by contents with the matching text, and
//! commands with their shortcuts, each group under a heading when more
//! than one shows. Mod-O and Mod-Shift-F open it filtered to names or
//! contents, and the filter shows as a chip in the field that a click or
//! Backspace in the empty field removes. `app-core` keeps the query, the
//! filter, the rows and the highlighted row; Enter opens the highlighted
//! file the way the file tree opens one, or runs the highlighted command
//! through the same dispatch as keys and menus.
//!
//! Filtered, search says how many files it found and lists them a page at
//! a time, the next page once you scroll or move the highlight near the
//! end. Unfiltered, a group that holds more files than it lists ends with
//! a Show all row that filters search to that group.

use std::{sync::Arc, time::Instant};

use app_core::{App, PaletteGroup, PaletteItem, PaletteStatus, Platform, SearchScope};
use egui::{
    Align2, Color32, CursorIcon, Event, EventFilter, FontId, Galley, Id, Key, Margin, Modal, Pos2,
    Rect, RichText, ScrollArea, Sense, TextEdit, TextWrapMode, Ui, UiBuilder, WidgetInfo,
    WidgetText,
    accesskit::{Action, Role},
    pos2, vec2,
};

use crate::{
    finder, keys,
    theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, radius},
};

const WIDTH: f32 = 576.0 - 16.0;
const LIST_HEIGHT: f32 = 320.0;
/// A row with one line: a command, or a file name with its folder beside
/// it.
pub(crate) const ROW_HEIGHT: f32 = 32.0;
/// A group's heading above its rows.
const HEADING_HEIGHT: f32 = 24.0;

/// What search remembers between frames.
#[derive(Default)]
pub(crate) struct PaletteView {
    /// The filter search showed last frame, if it showed.
    shown: Option<SearchScope>,
    /// The widget that had the keyboard before search opened.
    return_focus: Option<Id>,
    /// Scroll the highlighted row into view on the next frame.
    reveal: bool,
}

/// Search's field, the same under every filter, so it keeps the keyboard
/// when the filter changes.
fn field_id() -> Id {
    Id::unique("palette-field")
}

fn row_id(index: usize) -> Id {
    Id::unique(("palette-row", index))
}

/// The chip's text for a filter, and the name its remove button reads
/// as.
fn chip(scope: SearchScope) -> Option<(&'static str, &'static str)> {
    match scope {
        SearchScope::All => None,
        SearchScope::Names => Some(("Names", "Remove Names filter")),
        SearchScope::Contents => Some(("Contents", "Remove Contents filter")),
    }
}

fn heading(group: PaletteGroup) -> &'static str {
    match group {
        PaletteGroup::Files => "Files",
        PaletteGroup::Contents => "Contents",
        PaletteGroup::Commands => "Commands",
    }
}

/// The keys search takes: Up and Down move the highlight, Enter acts on
/// it and Backspace in the empty field removes the filter while the field
/// has the keyboard, and Escape closes search.
enum PaletteKey {
    Move(isize),
    Run,
    RemoveFilter,
    Close,
}

fn take_keys(ui: &Ui, from_field: bool, filter_removable: bool) -> Vec<PaletteKey> {
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
                Key::Backspace if from_field && filter_removable => PaletteKey::RemoveFilter,
                Key::Escape => PaletteKey::Close,
                _ => return true,
            };
            taken.push(found);
            false
        });
        taken
    })
}

/// Draw search while it's open, and give the keyboard back to where it
/// was once it closes.
pub(crate) fn show(ui: &mut Ui, app: &mut App, view: &mut PaletteView) {
    let ctx = ui.ctx().clone();
    let Some(scope) = app.palette().map(|palette| palette.scope) else {
        if view.shown.take().is_some() {
            give_back_keyboard(&ctx, view);
        }
        return;
    };
    let opening = view.shown.is_none();
    if opening {
        view.return_focus = ctx.memory(|memory| memory.focused());
    }
    view.shown = Some(scope);

    let theme = Palette::of(&ctx);
    let mut close = false;
    let mut run = false;
    let mut clicked = None;
    let mut more = false;
    let mut remove_filter = false;
    // The legacy palette's top edge sits a third of the way down.
    let top = ctx.content_rect().height() / 3.0;
    let area =
        Modal::default_area(Id::unique("palette")).anchor(Align2::CENTER_TOP, vec2(0.0, top));
    let modal = Modal::new(Id::unique("palette"))
        .area(area)
        .backdrop_color(crate::common::BACKDROP)
        .frame(
            crate::common::menu_frame(&ctx)
                .corner_radius(radius(2.2))
                .inner_margin(Margin::same(8)),
        )
        .show(&ctx, |ui| {
            ui.set_width(WIDTH);
            let builder = UiBuilder::new()
                .accessibility_role(Role::Dialog)
                .accessibility_label("Search");
            ui.scope_builder(builder, |ui| {
                let mut query = app
                    .palette()
                    .map(|palette| palette.query().to_owned())
                    .unwrap_or_default();
                let from_field = ui.memory(|memory| memory.has_focus(field_id()));
                let removable = scope != SearchScope::All && query.is_empty();
                for key in take_keys(ui, from_field, removable) {
                    match key {
                        PaletteKey::Move(offset) => {
                            app.move_palette_selection(offset);
                            view.reveal = true;
                        }
                        PaletteKey::Run => run = true,
                        PaletteKey::RemoveFilter => remove_filter = true,
                        PaletteKey::Close => close = true,
                    }
                }
                let chip = chip(scope).map(|(text, name)| {
                    let galley = line(ui, text, SUPPORTING_SIZE, f32::INFINITY);
                    (galley, name)
                });
                // Room for the chip's text, its close mark and padding.
                let chip_width = chip
                    .as_ref()
                    .map_or(0.0, |(galley, _)| galley.size().x + 30.0);
                // The legacy search field: a pill with a faint magnifying
                // glass at its start, then the filter's chip.
                let field = ui
                    .add(
                        TextEdit::singleline(&mut query)
                            .id(field_id())
                            .hint_text("Search")
                            .margin(Margin {
                                left: (32.0 + chip_width).min(127.0) as i8,
                                right: 10,
                                top: 6,
                                bottom: 6,
                            })
                            .desired_width(f32::INFINITY),
                    )
                    .accessible_name("Search");
                let glass = Rect::from_center_size(
                    pos2(field.rect.left() + 16.0, field.rect.center().y),
                    vec2(16.0, 16.0),
                );
                crate::icons::paint(
                    ui.painter(),
                    crate::icons::SEARCH,
                    glass,
                    theme.foreground.gamma_multiply(0.5),
                    false,
                );
                if let Some((galley, name)) = chip {
                    let rect = Rect::from_min_size(
                        pos2(field.rect.left() + 28.0, field.rect.center().y - 10.0),
                        vec2(chip_width - 4.0, 20.0),
                    );
                    if chip_button(ui, rect, galley, name, &theme).clicked() {
                        remove_filter = true;
                    }
                }
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
                if scope == SearchScope::Contents {
                    finder::filters(ui, app);
                }
                ui.add_space(6.0);
                if let Some(total) = app.palette_total().filter(|total| *total > 0) {
                    let files = if total == 1 { "file" } else { "files" };
                    note(ui, &theme, &format!("{total} {files}"));
                    ui.add_space(4.0);
                }
                let (row, near_end) = results(ui, app, view, &theme);
                clicked = row;
                more = near_end;
            });
        });
    if modal.should_close() {
        close = true;
    }

    if remove_filter {
        app.set_palette_scope(SearchScope::All);
        ctx.memory_mut(|memory| memory.request_focus(field_id()));
    }
    if more {
        app.show_more_results();
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

/// The filter's chip in the field: its name and a close mark, as a
/// button that removes the filter.
fn chip_button(
    ui: &mut Ui,
    rect: Rect,
    galley: Arc<Galley>,
    name: &str,
    theme: &Palette,
) -> egui::Response {
    let response = ui
        .interact(rect, Id::unique("palette-filter"), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, name));
    let painter = ui.painter();
    let fill = if response.hovered() {
        crate::common::mix(theme.muted, theme.foreground, 0.08)
    } else {
        theme.muted
    };
    painter.rect_filled(rect, radius(1.0), fill);
    let text_at = pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0);
    painter.galley(text_at, galley, theme.foreground);
    let mark = Rect::from_center_size(pos2(rect.right() - 10.0, rect.center().y), vec2(10.0, 10.0));
    crate::icons::paint(
        painter,
        crate::icons::CLOSE,
        mark,
        theme.muted_foreground,
        false,
    );
    response
}

fn give_back_keyboard(ctx: &egui::Context, view: &mut PaletteView) {
    match view.return_focus.take() {
        Some(id) => ctx.memory_mut(|memory| memory.request_focus(id)),
        None => ctx.memory_mut(|memory| memory.surrender_focus(field_id())),
    }
}

/// What search lists: its rows in groups, or what shows instead. Returns
/// the row you clicked, and whether the list shows its end, where the next
/// page belongs.
fn results(
    ui: &mut Ui,
    app: &App,
    view: &mut PaletteView,
    theme: &Palette,
) -> (Option<usize>, bool) {
    let Some(selected) = app.palette().map(|palette| palette.selected()) else {
        return (None, false);
    };
    let status = app.palette_status();
    let (clicked, near_end) = match status {
        PaletteStatus::Results => {
            let platform = app.platform();
            let rows: Vec<(PaletteGroup, Row)> = app
                .palette_results()
                .iter()
                .map(|item| (item.group(), row(item, platform)))
                .collect();
            list(ui, view, theme, &rows, selected)
        }
        PaletteStatus::Searching => {
            note(ui, theme, "Searching\u{2026}");
            (None, false)
        }
        PaletteStatus::NoResults => {
            note(ui, theme, "No results");
            (None, false)
        }
        PaletteStatus::Idle | PaletteStatus::Failed => (None, false),
    };
    finder::failure(ui, app, theme);
    (clicked, near_end)
}

/// How search draws `item`.
fn row(item: &PaletteItem, platform: Platform) -> Row {
    match item {
        PaletteItem::File(found) => Row {
            label: found.name.clone(),
            detail: found.folder.clone(),
            second_line: None,
            description: found.folder.clone(),
            shortcut: None,
            height: ROW_HEIGHT,
        },
        PaletteItem::Hit(hit) => finder::hit_row(hit),
        PaletteItem::More { total, .. } => {
            let label = total.map_or_else(
                || "Show all".to_owned(),
                |total| format!("Show all {total}"),
            );
            Row {
                label,
                detail: String::new(),
                second_line: None,
                description: String::new(),
                shortcut: None,
                height: ROW_HEIGHT,
            }
        }
        PaletteItem::Command(command) => {
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
                height: ROW_HEIGHT,
            }
        }
    }
}

/// Supporting text in place of the list.
pub(crate) fn note(ui: &mut Ui, theme: &Palette, text: &str) {
    ui.label(
        RichText::new(text)
            .size(SUPPORTING_SIZE)
            .color(theme.muted_foreground),
    );
}

/// One row of the list as search draws it.
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
    pub(crate) height: f32,
}

/// How close to the end of the list, in rows, the next page loads.
const MORE_AHEAD: f32 = 3.0;

/// The rows, one list box per group named after it, with the highlighted
/// row selected. A group shows its heading only when another group shows
/// too. Returns the row you clicked, and whether the list shows its end.
fn list(
    ui: &mut Ui,
    view: &mut PaletteView,
    theme: &Palette,
    rows: &[(PaletteGroup, Row)],
    selected: usize,
) -> (Option<usize>, bool) {
    let mut groups: Vec<(PaletteGroup, Vec<(usize, &Row)>)> = Vec::new();
    for (index, (group, row)) in rows.iter().enumerate() {
        match groups.last_mut() {
            Some((last, members)) if last == group => members.push((index, row)),
            _ => groups.push((*group, vec![(index, row)])),
        }
    }
    let headings = groups.len() > 1;
    let mut clicked = None;
    let output = ScrollArea::vertical()
        .id_salt("palette-list")
        .max_height(LIST_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (group, members) in &groups {
                let name = heading(*group);
                if headings {
                    let (rect, _) = ui.allocate_exact_size(
                        vec2(ui.available_width(), HEADING_HEIGHT),
                        Sense::hover(),
                    );
                    let galley = line(ui, name, SUPPORTING_SIZE, rect.width());
                    paint_line(
                        ui,
                        galley,
                        pos2(rect.left() + 8.0, rect.bottom() - 6.0),
                        Align2::LEFT_BOTTOM,
                        theme.muted_foreground,
                    );
                }
                let builder = UiBuilder::new()
                    .accessibility_role(Role::ListBox)
                    .accessibility_label(name);
                ui.scope_builder(builder, |ui| {
                    for (index, row) in members {
                        if let Some(index) = draw_row(ui, view, theme, *index, row, selected) {
                            clicked = Some(index);
                        }
                    }
                });
            }
        });
    let seen = output.state.clamped_offset().y + output.inner_rect.height();
    let near_end = seen >= output.content_size.y - MORE_AHEAD * ROW_HEIGHT;
    (clicked, near_end)
}

/// Row `index` of the list. Returns `index` when you click it.
fn draw_row(
    ui: &mut Ui,
    view: &mut PaletteView,
    theme: &Palette,
    index: usize,
    row: &Row,
    selected: usize,
) -> Option<usize> {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), row.height), Sense::hover());
    let id = row_id(index);
    let response = ui.interact(rect, id, Sense::CLICK);
    let is_selected = index == selected;
    response.widget_info(|| WidgetInfo::labeled(Role::ListBoxOption, true, &row.label));
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
    if ui.is_rect_visible(rect) {
        paint_row(ui, rect, row, is_selected || response.hovered(), theme);
    }
    response.clicked().then_some(index)
}

fn paint_row(ui: &Ui, rect: Rect, row: &Row, highlighted: bool, theme: &Palette) {
    let painter = ui.painter();
    if highlighted {
        painter.rect_filled(rect, radius(1.0), theme.muted);
    }
    let inner = rect.shrink2(vec2(8.0, 0.0));
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
