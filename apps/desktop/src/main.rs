//! The Noura desktop app.

mod file_drag;
mod login;
mod system;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

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

/// What the log keeps unless `RUST_LOG` says otherwise: messages from info
/// up, and only warnings from the window and graphics crates.
const LOG_FILTER: &str =
    "info,eframe=warn,egui_wgpu=warn,wgpu_core=warn,wgpu_hal=warn,naga=warn,winit=warn";

/// The size at which the log moves to `noura.log.1` and starts again.
const LOG_FILE_BYTES: u64 = 1_000_000;

/// How many rotated log files stay.
const LOG_FILES_KEPT: u64 = 3;

/// Log to the local diagnostic log in `app_data`, which Copy diagnostics
/// reads, and to the terminal in debug builds. When the log can't open,
/// messages go to the terminal instead.
fn start_logging(app_data: &Path) {
    use tracing_subscriber::{
        EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt,
    };

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(LOG_FILTER));
    let folder = app_core::log_folder(app_data);
    let file = logroller::LogRollerBuilder::new(folder.as_path(), Path::new(app_core::LOG_FILE))
        .rotation(logroller::Rotation::SizeBased(
            logroller::RotationSize::Bytes(LOG_FILE_BYTES),
        ))
        .max_keep_files(LOG_FILES_KEPT)
        .build();
    let (file, failure) = match file {
        Ok(file) => (Some(file), None),
        Err(error) => (None, Some(error)),
    };
    let file_layer = file.map(|file| {
        fmt::layer()
            .with_ansi(false)
            .with_writer(Mutex::new(file))
            .boxed()
    });
    let terminal = (cfg!(debug_assertions) || failure.is_some())
        .then(|| fmt::layer().with_writer(std::io::stderr).boxed());
    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(terminal)
        .init();
    if let Some(error) = failure {
        tracing::warn!(%error, "the diagnostic log couldn't open");
    }
}

fn main() -> eframe::Result {
    let app_data = app_core::default_app_data_dir();
    let has_app_data = app_data.is_some();
    let app_data = app_data.unwrap_or_else(|| std::env::temp_dir().join("Noura"));
    start_logging(&app_data);
    if !has_app_data {
        tracing::warn!("the system has no app data folder; using the temporary folder");
    }
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
            let mut app = App::launch_with(&app_data, Arc::new(system));
            app.set_login_item(Arc::new(login::DesktopLoginItem::new()));
            Ok(Box::new(Desktop {
                ui: NouraUi::new(app, Box::new(SystemFolderPicker)),
                file_drag: file_drag::FileDrag::default(),
            }))
        }),
    )
}
