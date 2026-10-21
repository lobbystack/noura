//! The Noura desktop app.

mod file_drag;
mod system;

use std::{path::PathBuf, sync::Arc};

use app_core::App;
use app_ui::{FolderPicker, NouraUi};
use eframe::egui;

/// The system folder dialog.
struct SystemFolderPicker;

impl FolderPicker for SystemFolderPicker {
    fn pick_folder(&mut self, title: &str) -> Option<PathBuf> {
        rfd::FileDialog::new().set_title(title).pick_folder()
    }
}

struct Desktop {
    ui: NouraUi,
    file_drag: file_drag::FileDrag,
}

impl eframe::App for Desktop {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.ui.ui(ui);
        let ctx = ui.ctx().clone();
        self.file_drag.after_frame(&mut self.ui, &ctx, frame);
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.file_drag.complete_input(ctx, raw_input);
    }
}

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let app_data = app_core::default_app_data_dir().unwrap_or_else(|| {
        tracing::warn!("the system has no app data folder; using the temporary folder");
        std::env::temp_dir().join("Noura")
    });
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("Noura")
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Noura",
        options,
        Box::new(move |creation| {
            app_ui::theme::install(&creation.egui_ctx);
            // The last workspace starts reopening now, off the UI thread.
            let system = system::DesktopSystem::new(creation.egui_ctx.clone());
            let app = App::launch_with(&app_data, Arc::new(system));
            Ok(Box::new(Desktop {
                ui: NouraUi::new(app, Box::new(SystemFolderPicker)),
                file_drag: file_drag::FileDrag::default(),
            }))
        }),
    )
}
