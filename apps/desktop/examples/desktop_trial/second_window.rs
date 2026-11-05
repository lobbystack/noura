//! The second window, drawn as a deferred egui viewport.

use std::sync::Arc;

use egui::mutex::Mutex;
use egui::{ViewportBuilder, ViewportId};

use crate::files::DropList;
use crate::trial::text_fields;

/// The second window's state. eframe draws a deferred viewport on its own
/// schedule, so the state sits behind a lock the main window shares.
#[derive(Default)]
pub struct SecondWindow {
    open: bool,
    single_line: String,
    multi_line: String,
    files: DropList,
}

/// The second window's viewport.
pub fn viewport_id() -> ViewportId {
    ViewportId::from_hash_of("noura desktop trial: second window")
}

impl SecondWindow {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        self.open = true;
    }
}

/// Keeps the second window on screen while it is open. Call it every frame
/// of the main window.
pub fn show(ctx: &egui::Context, window: &Arc<Mutex<SecondWindow>>) {
    if !window.lock().open {
        return;
    }
    let window = Arc::clone(window);
    let builder = ViewportBuilder::default()
        .with_title("Noura Desktop Trial: second window")
        .with_inner_size([480.0, 520.0]);
    ctx.show_viewport_deferred(viewport_id(), builder, move |ui, _class| {
        let mut window = window.lock();
        let window = &mut *window;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("This is the second window.");
            text_fields(ui, &mut window.single_line, &mut window.multi_line);
            window.files.ui(ui, false);
            if ui.button("Close this window").clicked() {
                window.open = false;
            }
        });
        if ui.input(|input| input.viewport().close_requested()) {
            window.open = false;
        }
        if !window.open {
            // The main window stops showing this viewport, which closes it.
            ui.ctx().request_repaint_of(ViewportId::ROOT);
        }
    });
}
