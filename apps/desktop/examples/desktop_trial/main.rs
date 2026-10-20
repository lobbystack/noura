//! Trial S6: desktop basics on eframe.
//!
//! A small window with a muda menu bar, Finder drag in and drag out through
//! the `drag` crate, a second window as an egui viewport, and text fields for
//! IME checks. Run it with `cargo run -p desktop --example desktop_trial`.
//! `--smoke` runs Ping and New Window through the menu bar, prints what it
//! saw and quits.

// The test build has no `main` to reach the shell from.
#![cfg_attr(test, allow(dead_code))]

mod command;
mod drag_out;
mod files;
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    allow(dead_code, reason = "Linux shows no native menu bar")
)]
mod menu;
mod second_window;
mod trial;

use std::sync::mpsc::{Receiver, Sender, channel};

use command::Command;
use drag_out::DragOutcome;
use trial::{Platform, Trial};

fn main() -> eframe::Result {
    let smoke = std::env::args().any(|argument| argument == "--smoke");
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("Noura Desktop Trial")
            .with_inner_size([640.0, 760.0]),
        event_loop_builder: Some(Box::new(configure_event_loop)),
        ..Default::default()
    };
    eframe::run_native(
        "Noura Desktop Trial",
        options,
        Box::new(move |cc| Ok(Box::new(Shell::new(cc, smoke)))),
    )
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

/// The platform side of the trial: the native menu, the menu choices and
/// native drags coming back from the OS, and the smoke run.
struct Shell {
    trial: Trial,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    _menu: Option<menu::native::NativeMenu>,
    commands: Receiver<Command>,
    drags: Receiver<DragOutcome>,
    drag_sender: Sender<DragOutcome>,
    smoke: Option<Smoke>,
}

impl Shell {
    fn new(cc: &eframe::CreationContext<'_>, smoke: bool) -> Self {
        let (command_sender, commands) = channel();
        let (drag_sender, drags) = channel();
        menu::forward_menu_choices(cc.egui_ctx.clone(), command_sender);
        #[cfg_attr(
            not(any(target_os = "macos", target_os = "windows")),
            expect(unused_mut, reason = "only a native menu can fail to install")
        )]
        let mut platform = Platform::current();

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let native_menu = match menu::native::install(cc, &menu::menu_bar()) {
            Ok(native_menu) => Some(native_menu),
            Err(error) => {
                eprintln!("The native menu bar failed to install: {error}");
                platform.native_menu = false;
                None
            }
        };

        Self {
            trial: Trial::new(platform),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            _menu: native_menu,
            commands,
            drags,
            drag_sender,
            smoke: smoke.then_some(Smoke {
                frames: 0,
                native_menu: platform.native_menu,
                shortcuts_taken: None,
            }),
        }
    }

    /// Starts the native drag a file row asked for during this frame.
    fn start_drag_out(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(path) = self.trial.take_drag_out_request() else {
            return;
        };
        let sender = self.drag_sender.clone();
        let waker = ctx.clone();
        let done = move |outcome| {
            if sender.send(outcome).is_ok() {
                waker.request_repaint_of(egui::ViewportId::ROOT);
            }
        };
        if let Err(error) = drag_out::start(frame, &path, done) {
            self.trial
                .drag_out_finished(ctx, DragOutcome::Failed(error));
        }
    }
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(command) = self.commands.try_recv() {
            self.trial.run(ctx, command);
        }
        while let Ok(outcome) = self.drags.try_recv() {
            self.trial.drag_out_finished(ctx, outcome);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.trial.ui(ui);
        let ctx = ui.ctx().clone();
        self.start_drag_out(&ctx, frame);
        if let Some(smoke) = &mut self.smoke {
            smoke.frame(&ctx, &mut self.trial);
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.trial.inject_input(raw_input);
    }
}

/// `--smoke`: runs Ping and New Window through the menu bar, then reports
/// and quits. On macOS it presses menu shortcuts on AppKit's main menu, as a
/// key press would reach it; elsewhere it runs the commands as a menu choice
/// would.
struct Smoke {
    frames: u32,
    native_menu: bool,
    /// How many of the pressed shortcuts a menu item took, out of how many.
    shortcuts_taken: Option<(usize, usize)>,
}

impl Smoke {
    fn frame(&mut self, ctx: &egui::Context, trial: &mut Trial) {
        self.frames += 1;
        match self.frames {
            1 => self.shortcuts_taken = press_menu_shortcuts(ctx, trial),
            30 => {
                let second_window = ctx.input(|input| {
                    input
                        .raw
                        .viewports
                        .contains_key(&second_window::viewport_id())
                });
                let shortcuts = match self.shortcuts_taken {
                    Some((taken, pressed)) => format!("{taken} of {pressed} taken by the menu bar"),
                    None => "not pressed".to_owned(),
                };
                println!(
                    "smoke: native menu bar {}; shortcuts {shortcuts}; Ping count {}; second window {}; {} frames",
                    if self.native_menu {
                        "installed"
                    } else {
                        "missing"
                    },
                    trial.pings(),
                    if second_window { "open" } else { "missing" },
                    self.frames,
                );
                trial.run(ctx, Command::Quit);
            }
            _ => {}
        }
        ctx.request_repaint();
    }
}

/// Presses Ping, New Window and three Edit shortcuts on AppKit's main menu.
/// Returns how many a menu item took, out of how many it pressed.
#[cfg(target_os = "macos")]
fn press_menu_shortcuts(_ctx: &egui::Context, _trial: &mut Trial) -> Option<(usize, usize)> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApp, NSEvent, NSEventModifierFlags, NSEventType};
    use objc2_foundation::{NSPoint, NSString};

    let mtm = MainThreadMarker::new()?;
    let menu = NSApp(mtm).mainMenu()?;
    // Characters, macOS virtual key code, and whether Shift is down.
    let shortcuts = [
        ("p", 0x23, true),  // Ping
        ("n", 0x2D, true),  // New Window
        ("a", 0x00, false), // Select All
        ("z", 0x06, false), // Undo
        ("c", 0x08, false), // Copy
    ];
    let taken = shortcuts
        .iter()
        .filter(|(key, key_code, shift)| {
            let mut flags = NSEventModifierFlags::Command;
            if *shift {
                flags |= NSEventModifierFlags::Shift;
            }
            let characters = NSString::from_str(key);
            NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                NSEventType::KeyDown,
                NSPoint::ZERO,
                flags,
                0.0,
                0,
                None,
                &characters,
                &characters,
                false,
                *key_code,
            )
            .is_some_and(|event| menu.performKeyEquivalent(&event))
        })
        .count();
    Some((taken, shortcuts.len()))
}

#[cfg(not(target_os = "macos"))]
fn press_menu_shortcuts(ctx: &egui::Context, trial: &mut Trial) -> Option<(usize, usize)> {
    trial.run(ctx, Command::Ping);
    trial.run(ctx, Command::NewWindow);
    None
}
