//! The Noura desktop app.

mod file_drag;
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    allow(dead_code, reason = "Linux shows no native menu bar")
)]
mod menu;
mod system;

use std::{
    path::PathBuf,
    sync::{
        Arc,
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
            let app = App::launch_with(&app_data, Arc::new(system));
            let ui = NouraUi::new(app, Box::new(SystemFolderPicker));
            Ok(Box::new(Desktop::new(creation, ui)))
        }),
    )
}
