//! The drop area that lists files dropped from Finder or Explorer.

use std::path::PathBuf;

/// A dropped path and what it pointed to when it arrived.
struct Listed {
    path: PathBuf,
    is_folder: bool,
}

/// The paths dropped on one window, in the order they arrived.
#[derive(Default)]
pub struct DropList {
    items: Vec<Listed>,
}

impl DropList {
    /// Draws the drop area and the list, and adds anything dropped on this
    /// window during this frame. With `drag_out`, file rows can be dragged,
    /// and the result is the file whose row started a drag this frame.
    pub fn ui(&mut self, ui: &mut egui::Ui, drag_out: bool) -> Option<PathBuf> {
        let (hovering, dropped) = ui.input(|input| {
            let dropped: Vec<PathBuf> = input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_owned())
                .collect();
            (input.raw.hovered_files.len(), dropped)
        });
        self.items.extend(dropped.into_iter().map(|path| Listed {
            is_folder: path.is_dir(),
            path,
        }));

        let visuals = ui.visuals();
        let (stroke, fill) = if hovering > 0 {
            (visuals.selection.stroke, visuals.selection.bg_fill)
        } else {
            (
                visuals.widgets.noninteractive.bg_stroke,
                visuals.faint_bg_color,
            )
        };
        egui::Frame::group(ui.style())
            .stroke(stroke)
            .fill(fill)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(match hovering {
                    0 => "Drop files or folders here.".to_owned(),
                    1 => "Release to list 1 item.".to_owned(),
                    count => format!("Release to list {count} items."),
                });
            });

        let mut dragged = None;
        for item in &self.items {
            let kind = if item.is_folder { "Folder" } else { "File" };
            let text = format!("{kind}: {}", item.path.display());
            if drag_out && !item.is_folder {
                let row = ui.add(egui::Label::new(text).sense(egui::Sense::drag()));
                if row.drag_started() {
                    dragged = Some(item.path.clone());
                }
            } else {
                ui.label(text);
            }
        }
        dragged
    }
}
