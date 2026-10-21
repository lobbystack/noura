//! The Noura desktop app.
//!
//! `--smoke` opens the window, prints what this copy found about itself
//! once the first frame is on screen, and quits. CI runs it from each
//! installed package.

// A release build on Windows opens no console window. Output still reaches
// a pipe, so the smoke run's report does too.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod licenses;
mod updates;

use std::{path::PathBuf, sync::Arc, time::Instant};

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
    /// The smoke run's report and how many frames it has drawn.
    smoke: Option<(String, u32)>,
}

impl eframe::App for Desktop {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.ui.ui(ui);
        if let Some((report, frames)) = &mut self.smoke {
            *frames += 1;
            // The first frame is on screen once the second one starts.
            if *frames == 2 {
                println!("{report}");
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ui.ctx().request_repaint();
            }
        }
    }
}

fn main() -> eframe::Result {
    let launched = Instant::now();
    let smoke = std::env::args()
        .skip(1)
        .any(|argument| argument == "--smoke");
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
            let mut app = App::launch(&app_data);
            let updates = set_up_updates(&mut app, &creation.egui_ctx, launched);
            let smoke = smoke.then(|| (smoke_report(&updates), 0));
            Ok(Box::new(Desktop {
                ui: NouraUi::new(app, Box::new(SystemFolderPicker)),
                smoke,
            }))
        }),
    )
}

/// Turn on updates from the release feed when this copy can replace
/// itself. Returns why not otherwise.
fn set_up_updates(app: &mut App, ctx: &egui::Context, launched: Instant) -> Result<(), String> {
    let version = env!("CARGO_PKG_VERSION");
    let executable = std::env::current_exe().unwrap_or_default();
    let appimage = std::env::var_os("APPIMAGE");
    let available = updates::availability(
        cfg!(debug_assertions),
        std::env::consts::OS,
        &executable,
        appimage.as_deref(),
    );
    if let Err(reason) = available {
        app.disable_updates(version, reason);
        return Err(reason.to_owned());
    }
    let ctx = ctx.clone();
    let quit = move || ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    match updates::FeedSource::new(
        updates::RELEASE_FEED,
        updates::RELEASE_PUBLIC_KEY,
        version,
        quit,
    ) {
        Ok(source) => {
            app.enable_updates(version, Arc::new(source), launched);
            Ok(())
        }
        Err(error) => {
            tracing::error!(%error, "the update feed is misconfigured");
            let reason = "This copy can't check for updates. Download new versions from noura.app.";
            app.disable_updates(version, reason);
            Err(reason.to_owned())
        }
    }
}

fn smoke_report(updates: &Result<(), String>) -> String {
    let licenses = licenses::bundled_licenses()
        .map_or_else(|| "missing".to_owned(), |path| path.display().to_string());
    let updates = match updates {
        Ok(()) => "on".to_owned(),
        Err(reason) => format!("off ({reason})"),
    };
    format!(
        "Noura {} drew its first window.\nThird-party licenses: {licenses}\nUpdates: {updates}",
        env!("CARGO_PKG_VERSION")
    )
}
