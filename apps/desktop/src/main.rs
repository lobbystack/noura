//! The Noura desktop app.

mod file_drag;
mod instance;
mod login;
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    allow(dead_code, reason = "Linux shows no native menu bar")
)]
mod menu;
mod system;
mod windows;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use app_core::App;
use app_ui::FolderPicker;
use eframe::egui;

use crate::{
    instance::{Claim, Instance},
    windows::Windows,
};

/// The system folder dialog.
struct SystemFolderPicker;

impl FolderPicker for SystemFolderPicker {
    fn pick_folder(&mut self, title: &str) -> Option<PathBuf> {
        rfd::FileDialog::new().set_title(title).pick_folder()
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

/// macOS: no winit default menu, since muda installs the menu bar.
/// Windows: menu shortcuts go through the accelerator table.
fn configure_event_loop(builder: &mut winit::event_loop::EventLoopBuilder<eframe::UserEvent>) {
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::EventLoopBuilderExtMacOS as _;
        builder.with_default_menu(false);
    }
    #[cfg(target_os = "windows")]
    {
        use winit::platform::windows::EventLoopBuilderExtWindows as _;
        builder.with_msg_hook(menu::native::translate_accelerator);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = builder;
}

fn main() -> eframe::Result {
    let app_data = app_core::default_app_data_dir();
    let has_app_data = app_data.is_some();
    let app_data = app_data.unwrap_or_else(|| std::env::temp_dir().join("Noura"));
    start_logging(&app_data);
    if !has_app_data {
        tracing::warn!("the system has no app data folder; using the temporary folder");
    }
    // A launch while Noura runs brings the running copy forward and stops
    // here, before it opens a workspace or an index.
    let instance: Option<Instance> = match instance::claim(&app_data) {
        Ok(Claim::First(instance)) => Some(instance),
        Ok(Claim::Running) => return Ok(()),
        Err(error) => {
            tracing::warn!(%error, "couldn't check for a running copy of Noura");
            None
        }
    };
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: windows::builder(None),
        event_loop_builder: Some(Box::new(configure_event_loop)),
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
            if let Some(instance) = &instance {
                let ctx = creation.egui_ctx.clone();
                instance.on_activate(move || ctx.request_repaint_of(egui::ViewportId::ROOT));
            }
            let mut windows = Windows::new(app, instance, || Box::new(SystemFolderPicker));
            windows.install_menu_bar(creation);
            Ok(Box::new(windows))
        }),
    )
}
