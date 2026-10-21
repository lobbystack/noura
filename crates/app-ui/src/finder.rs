//! Quick open's and search's part of the palette: search's type, folder
//! and tag filters, and the files that match. `palette.rs` draws the
//! dialog, its field and the list around them.

use app_core::{App, Palette, SearchHit, SearchState};
use egui::{RichText, TextEdit, Ui, UiBuilder, accesskit::Role};

use crate::{
    palette::{PaletteView, ROW_HEIGHT, Row, list, note},
    theme::{Palette as Theme, SUPPORTING_SIZE},
};

/// A search row: the title and type, then the folder and matching text.
const HIT_ROW_HEIGHT: f32 = 48.0;

/// Search's filters: a radio button per object type the index holds, and
/// fields for a folder and a tag.
pub(crate) fn filters(ui: &mut Ui, app: &mut App) {
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
pub(crate) fn results(
    ui: &mut Ui,
    app: &App,
    view: &mut PaletteView,
    theme: &Theme,
) -> Option<usize> {
    let palette = app.palette()?;
    let selected = palette.selected();
    match palette {
        Palette::Commands(_) => None,
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
                    shortcut: None,
                })
                .collect();
            list(ui, view, theme, "Files", &rows, selected, ROW_HEIGHT)
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
        shortcut: None,
    }
}
