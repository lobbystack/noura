//! Search's contents part: the type, folder and tag filters it offers
//! filtered to contents, the rows of files found by their text, and why
//! the index couldn't answer. `palette.rs` draws the dialog, its field
//! and the list around them.

use app_core::{App, SearchHit, SearchState};
use egui::{RichText, TextEdit, Ui, UiBuilder, accesskit::Role};

use crate::{
    palette::Row,
    theme::{Palette as Theme, SUPPORTING_SIZE},
};

/// A search row: the title and type, then the folder and matching text.
const HIT_ROW_HEIGHT: f32 = 48.0;

/// Search's filters: a radio button per object type the index holds, and
/// fields for a folder and a tag.
pub(crate) fn filters(ui: &mut Ui, app: &mut App) {
    let Some(search) = app.palette() else {
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

/// Why the index couldn't answer, as an alert under the rows.
pub(crate) fn failure(ui: &mut Ui, app: &App, theme: &Theme) {
    let Some(SearchState::Failed(error)) = app.palette().map(|palette| &palette.state) else {
        return;
    };
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
}

/// A file found by its text: the title and type, then the folder and
/// matching text.
pub(crate) fn hit_row(hit: &SearchHit) -> Row {
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
        height: HIT_ROW_HEIGHT,
    }
}
