//! The Noura desktop app.

mod file_drag;
mod instance;
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    allow(dead_code, reason = "Linux shows no native menu bar")
)]
mod menu;
mod system;
mod windows;

use std::{path::PathBuf, sync::Arc};

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
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let app_data = app_core::default_app_data_dir().unwrap_or_else(|| {
        tracing::warn!("the system has no app data folder; using the temporary folder");
        std::env::temp_dir().join("Noura")
    });
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
            let app = App::launch_with(&app_data, Arc::new(system));
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
