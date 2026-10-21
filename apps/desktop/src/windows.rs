//! Noura's windows. The root viewport shows the first window, and every
//! other window is a deferred egui viewport that draws on its own. All of
//! them share one `app-core` session.
//!
//! eframe quits when the root viewport closes, so closing the first
//! window while others stay open hides the root viewport instead, and it
//! keeps hosting the others. Closing the last window quits.

use std::sync::Arc;

use app_core::App;
use app_ui::{FolderPicker, NouraUi};
use eframe::egui::{
    self, Context, Pos2, ViewportBuilder, ViewportCommand, ViewportId, mutex::Mutex, vec2,
};

use crate::instance::Instance;

/// How far each new window sits from the window that opened it.
const CASCADE: f32 = 28.0;

/// A window's frame: its title, size and, for a new window, its place.
pub fn builder(position: Option<Pos2>) -> ViewportBuilder {
    let builder = ViewportBuilder::default()
        .with_title("Noura")
        .with_inner_size([1100.0, 720.0])
        .with_min_inner_size([640.0, 420.0]);
    match position {
        Some(position) => builder.with_position(position),
        None => builder,
    }
}

pub struct Windows {
    /// The window the root viewport shows, until it closes while other
    /// windows stay open.
    first: Option<NouraUi>,
    others: Vec<Window>,
    /// The window that had focus last, which a later launch brings
    /// forward.
    recent: Arc<Mutex<ViewportId>>,
    instance: Option<Instance>,
    picker: Box<dyn Fn() -> Box<dyn FolderPicker>>,
    next_id: u64,
}

/// A window in a deferred viewport.
struct Window {
    id: ViewportId,
    builder: ViewportBuilder,
    /// eframe draws a deferred viewport on its own schedule, so the state
    /// sits behind a lock.
    state: Arc<Mutex<WindowState>>,
}

struct WindowState {
    ui: Option<NouraUi>,
    /// Windows this one opened, and where it sat then, until the root
    /// viewport shows them.
    opened: Vec<App>,
    origin: Option<Pos2>,
}

impl Windows {
    /// Show `first` in the root viewport. `picker` makes each window's
    /// folder picker.
    pub fn new(
        first: App,
        instance: Option<Instance>,
        picker: impl Fn() -> Box<dyn FolderPicker> + 'static,
    ) -> Self {
        Self {
            first: Some(NouraUi::new(first, picker())),
            others: Vec::new(),
            recent: Arc::new(Mutex::new(ViewportId::ROOT)),
            instance,
            picker: Box::new(picker),
            next_id: 0,
        }
    }

    /// Draw the first window in the root viewport, and keep the other
    /// windows on screen.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut opened = Vec::new();
        if let Some(first) = &mut self.first {
            if ui.input(|input| input.viewport().close_requested()) && !self.others.is_empty() {
                // The other windows stay, so the root viewport hides
                // instead of quitting.
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                first.close_window();
            }
            first.ui(ui);
            note_focus(&ctx, ViewportId::ROOT, &self.recent);
            let origin = ctx.input(|input| input.viewport().outer_rect.map(|rect| rect.min));
            opened.extend(
                first
                    .take_new_windows()
                    .into_iter()
                    .map(|app| (app, origin)),
            );
            if first.is_closed() {
                self.first = None;
                if !self.others.is_empty() || !opened.is_empty() {
                    ctx.send_viewport_cmd(ViewportCommand::Visible(false));
                }
            }
        }
        for window in &self.others {
            let mut state = window.state.lock();
            let origin = state.origin;
            opened.extend(state.opened.drain(..).map(|app| (app, origin)));
        }
        self.others.retain(|window| !window.is_closed());
        let mut cascade = 0.0;
        for (app, origin) in opened {
            cascade += CASCADE;
            let position = origin.map(|origin| origin + vec2(cascade, cascade));
            self.add(app, position);
        }
        if self.first.is_none() && self.others.is_empty() {
            // The last window closed: Noura quits, and the window comes
            // back at the next launch.
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        for window in &self.others {
            window.show(&ctx, &self.recent);
        }
        if self
            .instance
            .as_ref()
            .is_some_and(Instance::take_activation)
        {
            self.bring_forward(&ctx);
        }
    }

    fn add(&mut self, app: App, position: Option<Pos2>) {
        let id = ViewportId::from_hash_of(("noura window", self.next_id));
        self.next_id += 1;
        self.others.push(Window {
            id,
            builder: builder(position),
            state: Arc::new(Mutex::new(WindowState {
                ui: Some(NouraUi::new(app, (self.picker)())),
                opened: Vec::new(),
                origin: None,
            })),
        });
    }

    /// Bring the window that had focus last to the front, as a later
    /// launch asks.
    fn bring_forward(&self, ctx: &Context) {
        let recent = *self.recent.lock();
        let open = |id: ViewportId| {
            (id == ViewportId::ROOT && self.first.is_some())
                || self.others.iter().any(|window| window.id == id)
        };
        let id = if open(recent) {
            recent
        } else if self.first.is_some() {
            ViewportId::ROOT
        } else if let Some(last) = self.others.last() {
            last.id
        } else {
            return;
        };
        ctx.send_viewport_cmd_to(id, ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd_to(id, ViewportCommand::Focus);
    }

    /// Let every window go, as Noura quits: each one's tabs stay saved,
    /// and the last to go writes them.
    fn quit(&mut self) {
        self.first = None;
        for window in self.others.drain(..) {
            let mut state = window.state.lock();
            state.ui = None;
            state.opened.clear();
        }
    }
}

impl Window {
    fn is_closed(&self) -> bool {
        self.state.lock().ui.as_ref().is_none_or(NouraUi::is_closed)
    }

    fn show(&self, ctx: &Context, recent: &Arc<Mutex<ViewportId>>) {
        let state = self.state.clone();
        let recent = recent.clone();
        let id = self.id;
        ctx.show_viewport_deferred(id, self.builder.clone(), move |ui, _class| {
            let mut state = state.lock();
            let state = &mut *state;
            let Some(window) = &mut state.ui else {
                return;
            };
            if ui.input(|input| input.viewport().close_requested()) {
                window.close_window();
            }
            window.ui(ui);
            let ctx = ui.ctx();
            note_focus(ctx, id, &recent);
            let opened = window.take_new_windows();
            if !opened.is_empty() {
                state.origin = ctx.input(|input| input.viewport().outer_rect.map(|rect| rect.min));
                state.opened.extend(opened);
            }
            if window.is_closed() || !state.opened.is_empty() {
                // The root viewport opens and closes windows.
                ctx.request_repaint_of(ViewportId::ROOT);
            }
        });
    }
}

/// Remember the viewport `id` as the window that had focus last when it
/// has focus now.
fn note_focus(ctx: &Context, id: ViewportId, recent: &Mutex<ViewportId>) {
    if ctx.input(|input| input.viewport().focused == Some(true)) {
        *recent.lock() = id;
    }
}

impl eframe::App for Windows {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn on_exit(&mut self) {
        self.quit();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        time::{Duration, Instant},
    };

    use egui::{Key, Modifiers, accesskit::Role};
    use egui_kittest::{Harness, kittest::Queryable};

    use super::*;
    use crate::instance::Claim;

    const SWITCHER: &str = "Switch workspace. Current workspace: Current";

    struct NoFolder;

    impl FolderPicker for NoFolder {
        fn pick_folder(&mut self, _title: &str) -> Option<PathBuf> {
            None
        }
    }

    /// kittest has one native window and draws only the first. The other
    /// windows stay separate viewports, as on the desktop, and the tests
    /// read what the desktop would show from their output.
    fn harness(app_data: &std::path::Path) -> Harness<'static, Windows> {
        harness_with(app_data, None)
    }

    fn harness_with(
        app_data: &std::path::Path,
        instance: Option<Instance>,
    ) -> Harness<'static, Windows> {
        let windows = Windows::new(App::launch(app_data), instance, || Box::new(NoFolder));
        let mut harness = Harness::builder()
            .with_size([1400.0, 900.0])
            .build_ui_state(|ui, windows: &mut Windows| windows.show(ui), windows);
        harness.ctx.set_embed_viewports(false);
        let deadline = Instant::now() + Duration::from_secs(20);
        while harness.query_by_label(SWITCHER).is_none() {
            assert!(Instant::now() < deadline, "the workspace never opened");
            harness.step();
            std::thread::sleep(Duration::from_millis(5));
        }
        harness.run();
        harness
    }

    fn workspace() -> tempfile::TempDir {
        let app_data = tempfile::tempdir().unwrap();
        let folder = app_data.path().join("Current");
        std::fs::create_dir(&folder).unwrap();
        app_core::WorkspaceService::new(app_data.path())
            .create("Current", &folder)
            .unwrap();
        app_data
    }

    fn root_commands(harness: &Harness<'static, Windows>) -> Vec<ViewportCommand> {
        harness
            .output()
            .viewport_output
            .get(&ViewportId::ROOT)
            .map(|output| output.commands.clone())
            .unwrap_or_default()
    }

    /// The windows the desktop shows besides the root viewport's.
    fn other_viewports(harness: &Harness<'static, Windows>) -> usize {
        harness
            .output()
            .viewport_output
            .keys()
            .filter(|id| **id != ViewportId::ROOT)
            .count()
    }

    /// Press Cmd-Shift-W (Ctrl-Shift-W off macOS) in the first window, and
    /// return what the root viewport was told in that frame. kittest runs
    /// a frame per event, so the key comes down alone.
    fn close_first_window(harness: &mut Harness<'static, Windows>) -> Vec<ViewportCommand> {
        harness.event(egui::Event::Key {
            key: Key::W,
            pressed: true,
            modifiers: Modifiers::COMMAND | Modifiers::SHIFT,
            repeat: false,
            physical_key: None,
        });
        harness.step();
        root_commands(harness)
    }

    fn open_new_window(harness: &mut Harness<'static, Windows>) {
        harness
            .get_by_role_and_label(Role::Button, SWITCHER)
            .click();
        harness.run();
        harness
            .get_by_role_and_label(Role::Button, "New window")
            .click();
        harness.run();
    }

    #[test]
    fn new_window_opens_another_viewport() {
        let app_data = workspace();
        let mut harness = harness(app_data.path());
        assert_eq!(other_viewports(&harness), 0);

        open_new_window(&mut harness);

        assert_eq!(other_viewports(&harness), 1);
        assert_eq!(harness.state().others.len(), 1);
    }

    #[test]
    fn closing_the_first_window_hides_it_and_keeps_the_others_until_the_last_closes() {
        let app_data = workspace();
        let mut harness = harness(app_data.path());
        open_new_window(&mut harness);

        let commands = close_first_window(&mut harness);
        assert!(commands.contains(&ViewportCommand::Visible(false)));
        assert!(!commands.contains(&ViewportCommand::Close));
        harness.run();
        assert_eq!(other_viewports(&harness), 1);
        assert!(harness.state().first.is_none());

        let second = harness.state().others[0].state.clone();
        second.lock().ui.as_mut().unwrap().close_window();
        harness.step();
        assert!(root_commands(&harness).contains(&ViewportCommand::Close));
    }

    #[test]
    fn closing_the_only_window_quits() {
        let app_data = workspace();
        let mut harness = harness(app_data.path());

        let commands = close_first_window(&mut harness);

        assert!(commands.contains(&ViewportCommand::Close));
        assert!(!commands.contains(&ViewportCommand::Visible(false)));
    }

    #[test]
    fn a_later_launch_brings_the_window_that_had_focus_last_to_the_front() {
        let app_data = workspace();
        let Ok(Claim::First(instance)) = crate::instance::claim(app_data.path()) else {
            panic!("nothing else runs on this app data folder");
        };
        let mut harness = harness_with(app_data.path(), Some(instance));
        open_new_window(&mut harness);
        let second = harness.state().others[0].id;
        *harness.state().recent.lock() = second;

        assert!(matches!(
            crate::instance::claim(app_data.path()),
            Ok(Claim::Running)
        ));

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            harness.step();
            let commands = harness
                .output()
                .viewport_output
                .get(&second)
                .map(|output| output.commands.clone())
                .unwrap_or_default();
            if commands.contains(&ViewportCommand::Focus) {
                assert!(commands.contains(&ViewportCommand::Minimized(false)));
                break;
            }
            assert!(Instant::now() < deadline, "no window came forward");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
