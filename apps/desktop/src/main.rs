//! The Noura desktop app.

mod file_drag;
mod login;
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    allow(dead_code, reason = "Linux shows no native menu bar")
)]
mod menu;
mod system;

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
}

impl Desktop {
    fn new(cc: &eframe::CreationContext<'_>, ui: NouraUi) -> Self {
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
        event_loop_builder: Some(Box::new(configure_event_loop)),
        ..Default::default()
    };
    eframe::run_native(
        "Noura",
        options,
        Box::new(move |creation| {
            app_ui::theme::install(&creation.egui_ctx);
            register_editor(creation)?;
            // The last workspace starts reopening now, off the UI thread.
            let system = system::DesktopSystem::new(creation.egui_ctx.clone());
            let mut app = App::launch_with(&app_data, Arc::new(system));
            app.set_login_item(Arc::new(login::DesktopLoginItem::new()));
            let ui = NouraUi::new(app, Box::new(SystemFolderPicker));
            Ok(Box::new(Desktop::new(creation, ui)))
        }),
    )
}
