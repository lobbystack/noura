//! The trial's state and its main window.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::mutex::Mutex;
use egui::{Event, Key, Modifiers, ViewportId};

use crate::command::Command;
use crate::drag_out::DragOutcome;
use crate::files::DropList;
use crate::second_window::{self, SecondWindow};

/// What the platform under the trial can do.
#[derive(Clone, Copy)]
pub struct Platform {
    /// The app shows a native menu bar.
    pub native_menu: bool,
    /// Files can be dragged out of the window.
    pub drag_out: bool,
}

impl Platform {
    /// macOS and Windows have both. On Linux muda's menus need a GTK window
    /// and the `drag` crate drags out through GTK, and winit has neither.
    pub fn current() -> Self {
        let native = cfg!(any(target_os = "macos", target_os = "windows"));
        Self {
            native_menu: native,
            drag_out: native,
        }
    }
}

/// Where the latest drag out of the window stands.
enum DragStatus {
    Idle,
    Dragging(String),
    Ended(String, DragOutcome),
}

/// Everything the trial window shows.
pub struct Trial {
    platform: Platform,
    pings: u32,
    single_line: String,
    multi_line: String,
    files: DropList,
    drag_out_request: Option<PathBuf>,
    drag_status: DragStatus,
    second_window: Arc<Mutex<SecondWindow>>,
    /// Key presses that menu items stand in for, per window, waiting for that
    /// window's next frame.
    pending_input: HashMap<ViewportId, Vec<Event>>,
}

impl Trial {
    pub fn new(platform: Platform) -> Self {
        Self {
            platform,
            pings: 0,
            single_line: String::new(),
            multi_line: String::new(),
            files: DropList::default(),
            drag_out_request: None,
            drag_status: DragStatus::Idle,
            second_window: Arc::default(),
            pending_input: HashMap::new(),
        }
    }

    /// Runs a command chosen from the menu bar or by its shortcut.
    pub fn run(&mut self, ctx: &egui::Context, command: Command) {
        match command {
            Command::Ping => self.pings += 1,
            Command::NewWindow => {
                self.second_window.lock().open();
                ctx.send_viewport_cmd_to(
                    second_window::viewport_id(),
                    egui::ViewportCommand::Focus,
                );
                ctx.request_repaint_of(ViewportId::ROOT);
            }
            Command::Quit => {
                ctx.send_viewport_cmd_to(ViewportId::ROOT, egui::ViewportCommand::Close);
            }
            Command::Undo => self.press(ctx, Modifiers::NONE, Key::Z),
            Command::Redo => self.press(ctx, Modifiers::SHIFT, Key::Z),
            Command::SelectAll => self.press(ctx, Modifiers::NONE, Key::A),
            Command::Cut => request_clipboard_event(ctx, egui::ViewportCommand::RequestCut),
            Command::Copy => request_clipboard_event(ctx, egui::ViewportCommand::RequestCopy),
            Command::Paste => request_clipboard_event(ctx, egui::ViewportCommand::RequestPaste),
            // muda's predefined About item opens the system About panel and
            // sends no menu event.
            Command::About => {}
        }
    }

    /// How many times Ping ran.
    pub fn pings(&self) -> u32 {
        self.pings
    }

    /// Adds the input that menu items stand in for to a window's next frame.
    /// Call it from `eframe::App::raw_input_hook`.
    pub fn inject_input(&mut self, input: &mut egui::RawInput) {
        if let Some(events) = self.pending_input.remove(&input.viewport_id) {
            input.events.extend(events);
        }
    }

    /// The file whose row started a drag out of the main window, if any. The
    /// caller starts the native drag and reports how it ended through
    /// [`Self::drag_out_finished`].
    pub fn take_drag_out_request(&mut self) -> Option<PathBuf> {
        let path = self.drag_out_request.take()?;
        self.drag_status = DragStatus::Dragging(file_name(&path));
        Some(path)
    }

    /// Shows how the native drag ended. The drag swallowed the mouse-up, so
    /// this also tells egui the button is up again.
    pub fn drag_out_finished(&mut self, ctx: &egui::Context, outcome: DragOutcome) {
        let name = match &self.drag_status {
            DragStatus::Dragging(name) => name.clone(),
            _ => String::new(),
        };
        let started = !matches!(outcome, DragOutcome::Failed(_));
        self.drag_status = DragStatus::Ended(name, outcome);
        if !started {
            return;
        }
        let pos = ctx
            .input(|input| input.pointer.latest_pos())
            .unwrap_or_default();
        self.pending_input
            .entry(ViewportId::ROOT)
            .or_default()
            .push(Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            });
        ctx.request_repaint_of(ViewportId::ROOT);
    }

    fn drag_status_text(&self) -> String {
        match &self.drag_status {
            DragStatus::Idle if self.platform.drag_out => {
                "Drag a listed file out of the window to copy it.".to_owned()
            }
            DragStatus::Idle => "Dragging files out isn't available on this platform.".to_owned(),
            DragStatus::Dragging(name) => format!("Dragging {name} out of the window."),
            DragStatus::Ended(name, DragOutcome::Dropped) => {
                format!("Dropped {name}. The other app got a copy.")
            }
            DragStatus::Ended(name, DragOutcome::Cancelled) => {
                format!("Cancelled. No app got a copy of {name}.")
            }
            DragStatus::Ended(name, DragOutcome::Failed(error)) => {
                format!("Couldn't drag {name} out: {error}")
            }
        }
    }

    /// Queues the key press a shortcut would send to the focused window.
    fn press(&mut self, ctx: &egui::Context, extra: Modifiers, key: Key) {
        let viewport = focused_viewport(ctx);
        let modifiers = command_modifiers() | extra;
        let events = self.pending_input.entry(viewport).or_default();
        for pressed in [true, false] {
            events.push(Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            });
        }
        ctx.request_repaint_of(viewport);
    }

    /// Draws the main window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Menu bar");
                if !self.platform.native_menu {
                    self.menu_stand_in(ui);
                }
                ui.label(match self.pings {
                    0 => "Ping hasn't run yet.".to_owned(),
                    1 => "Ping ran 1 time.".to_owned(),
                    pings => format!("Ping ran {pings} times."),
                });

                ui.add_space(12.0);
                ui.heading("Text");
                text_fields(ui, &mut self.single_line, &mut self.multi_line);

                ui.add_space(12.0);
                ui.heading("Files");
                if let Some(path) = self.files.ui(ui, self.platform.drag_out) {
                    self.drag_out_request = Some(path);
                }
                ui.label(self.drag_status_text());

                ui.add_space(12.0);
                ui.heading("Windows");
                ui.label(if self.second_window.lock().is_open() {
                    "The second window is open."
                } else {
                    "The second window is closed."
                });
            });
        });
        second_window::show(ui.ctx(), &self.second_window);
    }

    /// Buttons and shortcuts for the trial's own commands where no menu bar
    /// shows. Text fields already handle the Edit shortcuts themselves.
    fn menu_stand_in(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        for command in [Command::Ping, Command::NewWindow, Command::Quit] {
            let Some(shortcut) = command.shortcut() else {
                continue;
            };
            if ui.input_mut(|input| input.consume_shortcut(&shortcut.to_egui())) {
                self.run(&ctx, command);
            }
        }
        ui.horizontal(|ui| {
            ui.label("No menu bar on this platform:");
            for command in [Command::Ping, Command::NewWindow] {
                if ui.button(command.label()).clicked() {
                    self.run(&ctx, command);
                }
            }
        });
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// A single-line and a multi-line field, for the Edit menu and IME checks.
pub fn text_fields(ui: &mut egui::Ui, single_line: &mut String, multi_line: &mut String) {
    let label = ui.label("Single-line text");
    ui.text_edit_singleline(single_line).labelled_by(label.id);
    let label = ui.label("Multi-line text");
    ui.text_edit_multiline(multi_line).labelled_by(label.id);
}

/// Asks the focused window for a clipboard event. eframe answers with the
/// event egui-winit makes from the matching shortcut, from the same
/// clipboard.
fn request_clipboard_event(ctx: &egui::Context, command: egui::ViewportCommand) {
    ctx.send_viewport_cmd_to(focused_viewport(ctx), command);
}

/// The window with keyboard focus, or the main window when none has it.
fn focused_viewport(ctx: &egui::Context) -> ViewportId {
    ctx.input(|input| {
        input
            .raw
            .viewports
            .iter()
            .find(|(_, info)| info.focused == Some(true))
            .map(|(id, _)| *id)
    })
    .unwrap_or(ViewportId::ROOT)
}

/// The modifiers egui-winit reports for the platform's command key.
fn command_modifiers() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;
    use egui_kittest::{Harness, kittest::Queryable};

    /// The main window on macOS or Windows.
    fn harness() -> Harness<'static, Trial> {
        build(harness_builder())
    }

    /// Half a second per frame, so a few frames let a text field's undo
    /// history settle.
    fn harness_builder() -> egui_kittest::HarnessBuilder<Trial> {
        Harness::builder().with_step_dt(0.5)
    }

    fn build(builder: egui_kittest::HarnessBuilder<Trial>) -> Harness<'static, Trial> {
        let platform = Platform {
            native_menu: true,
            drag_out: true,
        };
        builder.build_ui_state(|ui, trial: &mut Trial| trial.ui(ui), Trial::new(platform))
    }

    /// Runs enough frames for egui to record an undo point.
    fn settle(harness: &mut Harness<'static, Trial>) {
        harness.run_steps(3);
    }

    #[test]
    fn running_the_custom_item_updates_the_window() {
        let mut harness = harness();
        harness.run();
        harness.get_by_label("Ping hasn't run yet.");

        let ctx = harness.ctx.clone();
        harness.state_mut().run(&ctx, Command::Ping);
        harness.run();
        harness.get_by_label("Ping ran 1 time.");

        harness.state_mut().run(&ctx, Command::Ping);
        harness.run();
        harness.get_by_label("Ping ran 2 times.");
    }

    /// Runs a menu command and feeds the window the input it asks for, the
    /// way `eframe::App::raw_input_hook` does.
    fn choose(harness: &mut Harness<'static, Trial>, command: Command) {
        let ctx = harness.ctx.clone();
        harness.state_mut().run(&ctx, command);
        let mut input = egui::RawInput::default();
        harness.state_mut().inject_input(&mut input);
        harness.input_mut().events.extend(input.events);
        harness.run();
    }

    fn single_line_value(harness: &Harness<'static, Trial>) -> String {
        harness
            .get_by_label("Single-line text")
            .value()
            .unwrap_or_default()
    }

    #[test]
    fn edit_menu_select_all_undo_and_redo_match_their_shortcuts() {
        let mut by_menu = harness();
        let mut by_keys = harness();
        for harness in [&mut by_menu, &mut by_keys] {
            harness.run();
            harness.get_by_label("Single-line text").focus();
            harness.run();
            harness
                .get_by_label("Single-line text")
                .type_text("hello world");
            settle(harness);
        }

        choose(&mut by_menu, Command::SelectAll);
        by_keys.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        by_keys.run();
        for harness in [&mut by_menu, &mut by_keys] {
            harness.get_by_label("Single-line text").type_text("x");
            settle(harness);
        }
        assert_eq!(single_line_value(&by_menu), "x");
        assert_eq!(single_line_value(&by_keys), "x");

        choose(&mut by_menu, Command::Undo);
        by_keys.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
        by_keys.run();
        assert_eq!(single_line_value(&by_menu), "hello world");
        assert_eq!(single_line_value(&by_keys), "hello world");

        choose(&mut by_menu, Command::Redo);
        by_keys.key_press_modifiers(
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            egui::Key::Z,
        );
        by_keys.run();
        assert_eq!(single_line_value(&by_menu), "x");
        assert_eq!(single_line_value(&by_keys), "x");
    }

    /// eframe answers these requests with the same `Cut`, `Copy` and
    /// `Paste` events egui-winit makes from the shortcuts, using the same
    /// clipboard.
    #[test]
    fn edit_menu_cut_copy_and_paste_ask_the_focused_window_for_its_clipboard_events() {
        for (command, request) in [
            (Command::Cut, egui::ViewportCommand::RequestCut),
            (Command::Copy, egui::ViewportCommand::RequestCopy),
            (Command::Paste, egui::ViewportCommand::RequestPaste),
        ] {
            let mut harness = harness();
            harness.run();
            let ctx = harness.ctx.clone();
            harness.state_mut().run(&ctx, command);
            harness.step();
            let commands = &harness.output().viewport_output[&ViewportId::ROOT].commands;
            assert_eq!(commands, &[request], "{command:?}");
        }
    }

    #[test]
    fn quit_closes_the_main_window_which_ends_the_app() {
        let mut harness = harness();
        harness.run();
        let ctx = harness.ctx.clone();
        harness.state_mut().run(&ctx, Command::Quit);
        harness.step();
        let commands = &harness.output().viewport_output[&ViewportId::ROOT].commands;
        assert_eq!(commands, &[egui::ViewportCommand::Close]);
    }

    /// A file dropped from Finder, as egui-winit hands it over.
    #[derive(Debug)]
    struct Dropped(std::path::PathBuf);

    impl egui::DroppedFile for Dropped {
        fn path(&self) -> &std::path::Path {
            &self.0
        }

        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|error| error.to_string())
        }
    }

    fn hovered(path: &std::path::Path) -> egui::HoveredFile {
        egui::HoveredFile {
            path: Some(path.to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn files_dragged_over_the_window_light_up_the_drop_area_until_dropped() {
        let folder = tempfile::tempdir().unwrap();
        let note = folder.path().join("Plan.md");
        std::fs::write(&note, "# Plan\n").unwrap();
        let mut harness = harness();
        harness.run();
        harness.get_by_label("Drop files or folders here.");

        harness.input_mut().hovered_files = vec![hovered(&note), hovered(folder.path())];
        // egui keeps repainting while files hover, so `run` would never settle.
        harness.run_steps(2);
        harness.get_by_label("Release to list 2 items.");

        harness.input_mut().hovered_files.clear();
        harness.run();
        harness.get_by_label("Drop files or folders here.");
    }

    #[test]
    fn dropping_files_and_a_folder_lists_each_path() {
        let folder = tempfile::tempdir().unwrap();
        let note = folder.path().join("Plan.md");
        std::fs::write(&note, "# Plan\n").unwrap();
        let projects = folder.path().join("Projects");
        std::fs::create_dir(&projects).unwrap();
        let mut harness = harness();
        harness.run();

        harness.input_mut().dropped_files = vec![
            std::sync::Arc::new(Dropped(note.clone())),
            std::sync::Arc::new(Dropped(projects.clone())),
        ];
        harness.run();

        harness.get_by_label(&format!("File: {}", note.display()));
        harness.get_by_label(&format!("Folder: {}", projects.display()));
    }

    /// Presses the mouse on a node and moves it far enough to start a drag.
    fn start_dragging(harness: &mut Harness<'static, Trial>, label: &str) {
        let from = harness.get_by_label(label).rect().center();
        harness.hover_at(from);
        harness.drag_at(from);
        harness.hover_at(from + egui::vec2(40.0, 0.0));
        harness.run();
    }

    /// Ends a native drag the way the shell does, then feeds the window the
    /// input the trial queues for it.
    fn finish_drag(harness: &mut Harness<'static, Trial>, outcome: DragOutcome) {
        let ctx = harness.ctx.clone();
        harness.state_mut().drag_out_finished(&ctx, outcome);
        let mut input = egui::RawInput::default();
        harness.state_mut().inject_input(&mut input);
        harness.input_mut().events.extend(input.events);
        harness.run();
    }

    #[test]
    fn dragging_a_listed_file_out_starts_a_native_drag_and_shows_how_it_ended() {
        let folder = tempfile::tempdir().unwrap();
        let note = folder.path().join("Plan.md");
        std::fs::write(&note, "# Plan\n").unwrap();
        let mut harness = harness();
        harness.run();
        harness.input_mut().dropped_files = vec![std::sync::Arc::new(Dropped(note.clone()))];
        harness.run();
        let row = format!("File: {}", note.display());

        start_dragging(&mut harness, &row);
        assert_eq!(
            harness.state_mut().take_drag_out_request(),
            Some(note.clone())
        );
        harness.run();
        harness.get_by_label("Dragging Plan.md out of the window.");

        finish_drag(&mut harness, DragOutcome::Dropped);
        harness.get_by_label("Dropped Plan.md. The other app got a copy.");
        assert_eq!(harness.ctx.dragged_id(), None, "the mouse button is up");

        start_dragging(&mut harness, &row);
        assert_eq!(
            harness.state_mut().take_drag_out_request(),
            Some(note.clone())
        );
        finish_drag(&mut harness, DragOutcome::Cancelled);
        harness.get_by_label("Cancelled. No app got a copy of Plan.md.");
    }

    /// kittest has one native window, so egui draws the second window inside
    /// the first as an embedded window.
    #[test]
    fn new_window_opens_a_second_window_that_closes_on_its_own() {
        let mut harness = harness();
        harness.run();
        harness.get_by_label("The second window is closed.");
        assert_eq!(harness.get_all_by_label("Single-line text").count(), 1);

        let ctx = harness.ctx.clone();
        harness.state_mut().run(&ctx, Command::NewWindow);
        harness.run();
        harness.get_by_label("The second window is open.");
        harness.get_by_label("This is the second window.");
        let fields: Vec<_> = harness.get_all_by_label("Single-line text").collect();
        assert_eq!(fields.len(), 2);
        fields[1].focus();
        harness.run();
        harness
            .get_all_by_label("Single-line text")
            .nth(1)
            .unwrap()
            .type_text("second");
        harness.run();
        let values: Vec<_> = harness
            .get_all_by_label("Single-line text")
            .map(|field| field.value().unwrap_or_default())
            .collect();
        assert_eq!(values, ["", "second"]);

        harness.get_by_label("Close this window").click();
        harness.run();
        harness.get_by_label("The second window is closed.");
        assert_eq!(harness.get_all_by_label("Single-line text").count(), 1);
        harness.get_by_label("Ping hasn't run yet.");
    }

    fn ime(event: egui::ImeEvent) -> egui::Event {
        egui::Event::Ime(event)
    }

    fn preedit(text: &str) -> egui::Event {
        ime(egui::ImeEvent::Preedit {
            text: text.to_owned(),
            active_range_chars: Some(0..text.chars().count()),
        })
    }

    /// The egui half of the IME check: what egui-winit hands over while
    /// someone types にほん and picks 日本. The owner checks the macOS half.
    #[test]
    fn japanese_input_shows_marked_text_inline_and_commits_it_in_both_fields() {
        for label in ["Single-line text", "Multi-line text"] {
            // egui's bundled fonts have no Japanese. The app falls back to
            // the system's fonts, which kittest doesn't load.
            let mut harness = build(harness_builder().allow_missing_glyphs());
            harness.run();
            harness.get_by_label(label).focus();
            harness.run();
            harness.get_by_label(label).type_text("A");
            harness.run();

            for (event, shown) in [
                (preedit("に"), "Aに"),
                (preedit("にほん"), "Aにほん"),
                (preedit("日本"), "A日本"),
                (ime(egui::ImeEvent::Commit("日本".to_owned())), "A日本"),
            ] {
                harness.event(event);
                harness.run();
                assert_eq!(harness.get_by_label(label).value().as_deref(), Some(shown));
            }
            harness.get_by_label(label).type_text("!");
            harness.run();
            assert_eq!(
                harness.get_by_label(label).value().as_deref(),
                Some("A日本!"),
                "{label}: the caret follows the committed text"
            );

            let field = harness.get_by_label(label).rect();
            let ime_area = harness
                .output()
                .platform_output
                .ime
                .expect("egui asks the platform to place the IME window");
            assert!(
                field.intersects(ime_area.cursor_rect),
                "{label}: the candidate window goes at the caret"
            );
        }
    }

    /// Linux: muda can't show a menu bar on a winit window, and the `drag`
    /// crate can't drag out of one.
    #[test]
    fn without_a_native_menu_bar_the_window_runs_the_trial_commands_itself() {
        let mut harness = Harness::builder().build_ui_state(
            |ui, trial: &mut Trial| trial.ui(ui),
            Trial::new(Platform {
                native_menu: false,
                drag_out: false,
            }),
        );
        harness.run();
        harness.get_by_label("Dragging files out isn't available on this platform.");

        harness.get_by_label("Ping").click();
        harness.run();
        harness.get_by_label("Ping ran 1 time.");

        harness.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::P);
        harness.run();
        harness.get_by_label("Ping ran 2 times.");

        harness.get_by_label("New Window").click();
        harness.run();
        harness.get_by_label("The second window is open.");
    }
}
