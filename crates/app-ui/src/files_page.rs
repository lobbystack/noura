//! The Files page: one folder of the workspace as a list, folders first,
//! then files, each with its icon, its name and when it last changed, as a
//! file manager's list shows them. The path bar over the list names the
//! folders above it and goes back up to any of them. A folder opens in
//! place, a file in its own tab, and a right click shows the tree's menu
//! for the row. `app-core` decides which folder the page lists.

use app_core::{App, EntryKind, FilesEntry, FilesListing, OpenAs, TreeRow};
use egui::{
    Align2, Event, Id, Key, Modifiers, Pos2, Rect, RichText, Sense, Ui, UiBuilder, WidgetInfo,
    accesskit::Role, pos2, vec2,
};

use crate::{
    common::{self, Btn, Variant},
    icons,
    theme::{BODY_SIZE, Palette, TITLE_SIZE, Weight, font, radius},
};

/// A row's height.
const ROW_HEIGHT: f32 = 32.0;

/// The row whose menu is open, and where a right click opened it.
#[derive(Clone)]
struct OpenMenu {
    relative_path: String,
    at: Option<Pos2>,
}

fn menu_id() -> Id {
    Id::unique("files-page-menu")
}

fn row_id(relative_path: &str) -> Id {
    Id::unique(("files-page-row", relative_path))
}

pub(crate) fn show(ui: &mut Ui, app: &mut App) {
    let Some(listing) = app.files_listing().cloned() else {
        return;
    };
    path_bar(ui, app, &listing);
    if listing.entries.is_empty() {
        common::empty_state_with(
            ui,
            Some(icons::FOLDER_OPEN),
            "Empty folder",
            None,
            Some(|ui: &mut Ui| {
                let clicked = Btn::new("New note")
                    .icon(icons::NEW_NOTE)
                    .variant(Variant::Default)
                    .show(ui)
                    .clicked();
                if clicked {
                    app.new_note(Some(&listing.folder));
                }
            }),
        );
        return;
    }
    list(ui, app, &listing);
}

/// The folders from the top of the workspace down to the listed one, as
/// the page's title: each one above it goes back up to it.
fn path_bar(ui: &mut Ui, app: &mut App, listing: &FilesListing) {
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new()
        .accessibility_role(Role::Navigation)
        .accessibility_label("Path");
    let mut go = None;
    ui.scope_builder(builder, |ui| {
        ui.horizontal(|ui| {
            ui.set_min_height(36.0);
            ui.spacing_mut().item_spacing.x = 4.0;
            let last = listing.path.len().saturating_sub(1);
            for (index, step) in listing.path.iter().enumerate() {
                if index == last {
                    ui.label(
                        RichText::new(&step.name)
                            .font(font(ui.ctx(), TITLE_SIZE, Weight::Semibold))
                            .color(palette.foreground),
                    );
                    break;
                }
                let response = ui
                    .add(
                        egui::Button::new(
                            RichText::new(&step.name)
                                .font(font(ui.ctx(), TITLE_SIZE, Weight::Regular))
                                .color(palette.muted_foreground),
                        )
                        .frame(false),
                    )
                    .accessible_name(&step.name);
                if response.clicked() {
                    go = Some(step.relative_path.clone());
                }
                let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                icons::paint(
                    ui.painter(),
                    icons::CARET_RIGHT,
                    rect,
                    palette.muted_foreground,
                    false,
                );
            }
        });
        // The header's line runs the page's whole width, as on the other
        // pages.
        let rect = ui.min_rect();
        let y = rect.bottom() + 12.0;
        let width = ui.max_rect().x_range().expand(24.0);
        ui.painter()
            .hline(width, y, egui::Stroke::new(1.0, palette.border));
        let gap = y - ui.cursor().top();
        ui.add_space(gap.max(0.0) + 8.0);
    });
    if let Some(folder) = go {
        app.show_files_folder(&folder);
    }
}

/// What a press on a row asks for.
enum Press {
    Into(String),
    Open(String, OpenAs),
    Menu(String, Option<Pos2>),
    Up,
}

fn list(ui: &mut Ui, app: &mut App, listing: &FilesListing) {
    let palette = Palette::of(ui.ctx());
    let selected = app.tree().and_then(|tree| {
        let row = tree.focused()?;
        tree.rows().get(row).map(|row| row.relative_path.clone())
    });
    let names = crate::dates::DateNames::of(app.locale().as_deref());
    let zone = app.time_zone();
    let today = app.today();
    let mut menu: Option<OpenMenu> = ui.data(|data| data.get_temp(menu_id()));
    let mut press = None;
    let focused = ui.memory(|memory| memory.focused());
    let focused_index = listing
        .entries
        .iter()
        .position(|entry| Some(row_id(&entry.relative_path)) == focused);
    // The arrow keys move between rows, Return opens one, and Backspace
    // goes up a folder, while a row has the keyboard.
    let mut move_to = None;
    if let Some(index) = focused_index {
        let keys = ui.input_mut(|input| {
            let mut keys = Vec::new();
            input.events.retain(|event| match event {
                Event::Key {
                    key: key @ (Key::ArrowUp | Key::ArrowDown | Key::Enter | Key::Backspace),
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.is_none() || *modifiers == Modifiers::ALT => {
                    keys.push((*key, *modifiers));
                    false
                }
                _ => true,
            });
            keys
        });
        for (key, modifiers) in keys {
            match key {
                Key::ArrowDown => move_to = Some((index + 1).min(listing.entries.len() - 1)),
                Key::ArrowUp if modifiers == Modifiers::ALT => press = Some(Press::Up),
                Key::ArrowUp => move_to = Some(index.saturating_sub(1)),
                Key::Backspace => press = Some(Press::Up),
                Key::Enter => press = Some(activate(&listing.entries[index], OpenAs::Preview)),
                _ => {}
            }
        }
    }
    let builder = UiBuilder::new()
        .accessibility_role(Role::ListBox)
        .accessibility_label("Files");
    ui.scope_builder(builder, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (index, entry) in listing.entries.iter().enumerate() {
            let id = row_id(&entry.relative_path);
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
            let response = ui.interact(rect, id, Sense::click());
            let changed = entry.modified.map(|at| {
                let at = at.to_zoned(zone.clone());
                names.changed(at.date(), at.time(), today)
            });
            let description = match (&entry.kind, &changed) {
                (EntryKind::Folder, Some(changed)) => format!("Folder, {changed}"),
                (EntryKind::Folder, None) => "Folder".to_owned(),
                (EntryKind::File, Some(changed)) => changed.clone(),
                (EntryKind::File, None) => String::new(),
            };
            let is_selected = selected.as_deref() == Some(entry.relative_path.as_str());
            response.widget_info(|| {
                WidgetInfo::selected(Role::ListBoxOption, true, is_selected, &entry.name)
            });
            ui.ctx().accesskit_node_builder(id, |node| {
                if !description.is_empty() {
                    node.set_description(description.clone());
                }
            });
            if move_to == Some(index) {
                response.request_focus();
                response.scroll_to_me(None);
            }
            if ui.is_rect_visible(rect) {
                draw_row(
                    ui,
                    rect,
                    entry,
                    changed.as_deref(),
                    &response,
                    is_selected,
                    &palette,
                );
            }
            if response.secondary_clicked() {
                press = Some(Press::Menu(
                    entry.relative_path.clone(),
                    response.interact_pointer_pos(),
                ));
            } else if (response.middle_clicked() || response.double_clicked())
                && entry.kind == EntryKind::File
            {
                // A middle-click or a double-click keeps the file's tab.
                press = Some(Press::Open(entry.relative_path.clone(), OpenAs::Kept));
            } else if response.clicked() {
                let command = ui.input(|input| input.modifiers.command);
                let how = if command {
                    OpenAs::Kept
                } else {
                    OpenAs::Preview
                };
                press = Some(activate(entry, how));
            }
            if menu
                .as_ref()
                .is_some_and(|menu| menu.relative_path == entry.relative_path)
            {
                let row = TreeRow {
                    name: entry.name.clone(),
                    relative_path: entry.relative_path.clone(),
                    kind: entry.kind,
                    depth: 0,
                    expanded: None,
                    not_downloaded: entry.not_downloaded,
                };
                let at = menu.as_ref().and_then(|menu| menu.at);
                if !crate::tree::item_menu(&response, &row, at, app) {
                    menu = None;
                }
            }
        }
    });
    match press {
        Some(Press::Into(folder)) => app.show_files_folder(&folder),
        Some(Press::Open(path, how)) => app.open_from_files(&path, how),
        Some(Press::Menu(path, at)) => {
            menu = Some(OpenMenu {
                relative_path: path,
                at,
            });
        }
        Some(Press::Up) => {
            if let Some(parent) = listing.path.iter().rev().nth(1) {
                app.show_files_folder(&parent.relative_path);
            }
        }
        None => {}
    }
    ui.data_mut(|data| match menu {
        Some(menu) => {
            data.insert_temp(menu_id(), menu);
        }
        None => data.remove::<OpenMenu>(menu_id()),
    });
}

fn activate(entry: &FilesEntry, how: OpenAs) -> Press {
    match entry.kind {
        EntryKind::Folder => Press::Into(entry.relative_path.clone()),
        EntryKind::File => Press::Open(entry.relative_path.clone(), how),
    }
}

fn draw_row(
    ui: &Ui,
    rect: Rect,
    entry: &FilesEntry,
    changed: Option<&str>,
    response: &egui::Response,
    selected: bool,
    palette: &Palette,
) {
    let painter = ui.painter();
    let inner = rect.shrink2(vec2(0.0, 1.0));
    if selected || response.hovered() {
        painter.rect_filled(inner, radius(0.8), palette.accent);
    }
    if response.has_focus() {
        common::focus_ring(painter, inner, radius(0.8), palette);
    }
    let icon = match entry.kind {
        EntryKind::Folder => icons::FOLDER,
        EntryKind::File if entry.relative_path.to_ascii_lowercase().ends_with(".pdf") => icons::PDF,
        EntryKind::File => icons::NOTE,
    };
    let center = rect.center().y;
    let icon_rect = Rect::from_center_size(pos2(rect.left() + 18.0, center), vec2(16.0, 16.0));
    icons::paint(painter, icon, icon_rect, palette.muted_foreground, false);
    let date = changed.map(|changed| {
        painter.layout_no_wrap(
            changed.to_owned(),
            font(ui.ctx(), BODY_SIZE, Weight::Regular),
            palette.muted_foreground,
        )
    });
    let date_width = date.as_ref().map_or(0.0, |date| date.size().x + 24.0);
    let name_left = rect.left() + 36.0;
    let name = egui::WidgetText::from(RichText::new(&entry.name).font(font(
        ui.ctx(),
        BODY_SIZE,
        Weight::Regular,
    )))
    .into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (rect.right() - 12.0 - date_width - name_left).max(0.0),
        egui::TextStyle::Body,
    );
    let color = if entry.not_downloaded {
        palette.muted_foreground
    } else {
        palette.foreground
    };
    let at = Align2::LEFT_CENTER.anchor_size(pos2(name_left, center), name.size());
    painter.galley(at.min, name, color);
    if let Some(date) = date {
        let at = Align2::RIGHT_CENTER.anchor_size(pos2(rect.right() - 12.0, center), date.size());
        painter.galley(at.min, date, palette.muted_foreground);
    }
}
