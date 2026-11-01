//! The Noura desktop app.
//!
//! `--smoke` opens the first window, prints what this copy found about
//! itself once that window's first frame is on screen, and quits. CI runs
//! it from each installed package. A copy that lacks a plugin it should
//! ship says which and fails without opening a window.
//!
//! `mcp --workspace <folder>` serves that workspace to an AI tool over
//! stdio instead, with `noura-mcp`. It opens no window and doesn't count as
//! a running copy of Noura, so it runs while the app has the workspace
//! open.

// A release build on Windows opens no console window. Output still reaches
// a pipe, so the smoke run's report does too.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod file_drag;
mod instance;
mod licenses;
mod login;
mod mcp;
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    allow(dead_code, reason = "Linux shows no native menu bar")
)]
mod menu;
mod spelling;
mod system;
mod updates;
mod windows;

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use app_core::{App, PendingWrites};
use app_ui::FolderPicker;
use eframe::egui;

use crate::{
    instance::{Claim, Instance},
    windows::{QuitRequest, Windows},
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

/// How long Noura waits, once its windows have closed, for writes still
/// running to reach disk before it quits anyway.
const QUIT_WAIT: Duration = Duration::from_secs(30);

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

/// Give the note editor its fonts, egui's own then the system's so text in
/// other scripts finds glyphs, and the renderer resources it paints with.
fn register_editor(
    creation: &eframe::CreationContext<'_>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let render_state = creation
        .wgpu_render_state
        .as_ref()
        .ok_or("Noura needs the wgpu renderer to draw notes")?;
    let mut fonts = editor_egui::fonts::default_font_database();
    fonts.load_system_fonts();
    let font_system = editor_egui::register_font_system(&creation.egui_ctx, fonts);
    editor_egui::register_render_callback_resources(
        &render_state.device,
        &render_state.queue,
        render_state.target_format,
        &mut render_state.renderer.write(),
        font_system,
        1,
    );
    Ok(())
}

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() == Some(OsStr::new("mcp")) {
        return noura_mcp::run(arguments);
    }
    let smoke = std::env::args_os()
        .skip(1)
        .any(|argument| argument == "--smoke");
    let smoke = match smoke.then(licenses::shipped_plugins_line).transpose() {
        Ok(plugins) => plugins,
        Err(missing) => {
            println!("{missing}");
            return ExitCode::FAILURE;
        }
    };
    match run_app(smoke) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Noura stopped: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Run the app. `smoke` is the plugins line of a smoke run's report.
fn run_app(smoke: Option<String>) -> eframe::Result {
    let launched = Instant::now();
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
    let writes = Arc::new(OnceLock::<PendingWrites>::new());
    let install_at_exit = updates::InstallAtExit::default();
    let closed = eframe::run_native(
        "Noura",
        options,
        Box::new({
            let writes = writes.clone();
            let install_at_exit = install_at_exit.clone();
            move |creation| {
                app_ui::theme::install(&creation.egui_ctx);
                register_editor(creation)?;
                // The last workspace starts reopening now, off the UI thread.
                let system = Arc::new(system::DesktopSystem::new(creation.egui_ctx.clone()));
                let mut app = App::launch_with(&app_data, system.clone());
                let _ = writes.set(app.pending_writes().clone());
                app.set_login_item(Arc::new(login::DesktopLoginItem::new()));
                // The sync account keeps this computer's keys and session in
                // credential store items that never leave it, and signs in
                // through the system browser. The same backend runs the open
                // workspace's sync with that account.
                let browser = system.clone();
                let account = Arc::new(app_core::account::live::LiveAccount::start(
                    app_data.clone(),
                    Arc::new(app_core::sync::DeviceKeychain),
                    Arc::new(move |url: &str| app_core::System::open_url(&*browser, url)),
                ));
                app.set_account_backend(account.clone());
                app.set_share_backend(account.clone());
                app.set_members_backend(account.clone());
                app.set_folder_sync_backend(account);
                if let Some(list) = licenses::bundled_licenses() {
                    app.set_third_party_licenses(list);
                }
                if let Some(plugins) = licenses::bundled_plugins() {
                    app.set_shipped_plugins(plugins);
                }
                mcp::offer(&mut app);
                let quit = QuitRequest::default();
                let updates = set_up_updates(
                    &mut app,
                    &creation.egui_ctx,
                    launched,
                    install_at_exit,
                    quit.clone(),
                );
                if let Some(instance) = &instance {
                    let ctx = creation.egui_ctx.clone();
                    instance.on_activate(move || ctx.request_repaint_of(egui::ViewportId::ROOT));
                }
                let mut windows = Windows::new(app, instance, || Box::new(SystemFolderPicker))
                    .quitting_on(quit)
                    .checking_spelling_with(spelling::system_spell_checker());
                if let Some(plugins) = &smoke {
                    windows = windows.smoke_test(smoke_report(plugins, &updates));
                }
                windows.install_menu_bar(creation);
                Ok(Box::new(windows))
            }
        }),
    );
    // The windows are gone, so nothing starts a new write. Writes still
    // running finish before the process ends, whichever way Noura quit.
    if let Some(writes) = writes.get()
        && !writes.wait_for_running(QUIT_WAIT)
    {
        tracing::error!("Noura quit while a write was still running");
    }
    install_at_exit.run();
    closed
}

/// Turn on updates from the release feed when this copy can replace
/// itself. Returns why not otherwise. On Windows an update waits in
/// `install_at_exit` until Noura has quit.
fn set_up_updates(
    app: &mut App,
    ctx: &egui::Context,
    launched: Instant,
    install_at_exit: updates::InstallAtExit,
    quit: QuitRequest,
) -> Result<(), String> {
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
    // Once the relaunch is arranged, Noura quits as Quit does, and every
    // window comes back in the new version.
    let ctx = ctx.clone();
    let quit = move || quit.ask(&ctx);
    let source = updates::FeedSource::new(
        updates::RELEASE_FEED,
        updates::RELEASE_PUBLIC_KEY,
        version,
        quit,
    )
    .map(|source| {
        if cfg!(windows) {
            source.installing_at_exit(install_at_exit)
        } else {
            source
        }
    });
    match source {
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

fn smoke_report(plugins: &str, updates: &Result<(), String>) -> String {
    let licenses = licenses::bundled_licenses()
        .map_or_else(|| "missing".to_owned(), |path| path.display().to_string());
    let updates = match updates {
        Ok(()) => "on".to_owned(),
        Err(reason) => format!("off ({reason})"),
    };
    format!(
        "Noura {} drew its first window.\nThird-party licenses: {licenses}\n{plugins}\nUpdates: {updates}",
        env!("CARGO_PKG_VERSION")
    )
}
