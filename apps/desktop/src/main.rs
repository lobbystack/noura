//! The Noura desktop app.
//!
//! `--smoke` opens the window, prints what this copy found about itself
//! once the first frame is on screen, and quits. CI runs it from each
//! installed package.

// A release build on Windows opens no console window. Output still reaches
// a pipe, so the smoke run's report does too.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod file_drag;
mod licenses;
mod login;
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    allow(dead_code, reason = "Linux shows no native menu bar")
)]
mod menu;
mod system;
mod updates;

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, channel},
    },
    time::Instant,
};

use app_core::App;
use app_ui::{FolderPicker, NouraUi};
use eframe::egui;
use menu::{Choice, EditInput};

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
    /// Menu choices, sent from muda's event handler.
    choices: Receiver<Choice>,
    edit: EditInput,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    native_menu: Option<menu::native::NativeMenu>,
    /// The smoke run's report and how many frames it has drawn.
    smoke: Option<(String, u32)>,
}

impl Desktop {
    fn new(cc: &eframe::CreationContext<'_>, ui: NouraUi, smoke: Option<String>) -> Self {
        let (sender, choices) = channel();
        let ctx = cc.egui_ctx.clone();
        muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
            if let Some(choice) = menu::choice_for(event.id())
                && sender.send(choice).is_ok()
            {
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
        }));
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let native_menu = match menu::native::install(cc, &ui.menu_bar()) {
            Ok(native_menu) => Some(native_menu),
            Err(error) => {
                tracing::error!(%error, "the native menu bar failed to install");
                None
            }
        };
        Self {
            ui,
            file_drag: file_drag::FileDrag::default(),
            choices,
            edit: EditInput::default(),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            native_menu,
            smoke: smoke.map(|report| (report, 0)),
        }
    }
}

impl eframe::App for Desktop {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(choice) = self.choices.try_recv() {
            match choice {
                Choice::Command(id) => {
                    self.ui.run_menu_command(&id, Instant::now());
                }
                Choice::System(item) => self.edit.run(ctx, item),
            }
        }
        // Quit from the menu works while the window is hidden too.
        self.ui.close_requested_windows(ctx);
        // A command that registered, or a page that moved, changes the
        // menus.
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(native_menu) = &mut self.native_menu
            && let Err(error) = native_menu.update(&self.ui.menu_bar())
        {
            tracing::error!(%error, "the native menu bar failed to update");
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.ui.ui(ui);
        let ctx = ui.ctx().clone();
        self.file_drag.after_frame(&mut self.ui, &ctx, frame);
        if let Some((report, frames)) = &mut self.smoke {
            *frames += 1;
            // The first frame is on screen once the second one starts.
            if *frames == 2 {
                println!("{report}");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.request_repaint();
            }
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.file_drag.complete_input(ctx, raw_input);
        self.edit.inject(raw_input);
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
    let launched = Instant::now();
    let smoke = std::env::args()
        .skip(1)
        .any(|argument| argument == "--smoke");
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
            let updates = set_up_updates(&mut app, &creation.egui_ctx, launched);
            let smoke = smoke.then(|| smoke_report(&updates));
            let ui = NouraUi::new(app, Box::new(SystemFolderPicker));
            Ok(Box::new(Desktop::new(creation, ui, smoke)))
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
