//! Noura's windows. The root viewport shows the first window, and every
//! other window is a deferred egui viewport that draws on its own. All of
//! them share one `app-core` session.
//!
//! eframe quits when the root viewport closes, so closing the first
//! window while others stay open hides the root viewport instead, and it
//! keeps hosting the others. Closing the last window quits.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use app_core::App;
use app_ui::{FolderPicker, NouraUi};
use eframe::egui::{
    self, Context, Pos2, ViewportBuilder, ViewportCommand, ViewportId, mutex::Mutex, vec2,
};

use crate::instance::Instance;

/// How far each new window sits from the window that opened it.
const CASCADE: f32 = 28.0;

/// How long Noura keeps bringing a window forward after a later launch
/// asks, and how often it tries again. A minimized window animates back
/// before it can take focus, and the system may refuse focus at first.
const FORWARD_PATIENCE: Duration = Duration::from_secs(3);
const FORWARD_RETRY: Duration = Duration::from_millis(200);

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
    /// A later launch asked for a window to come forward, and it hasn't
    /// come yet.
    forward: Option<Forward>,
    picker: Box<dyn Fn() -> Box<dyn FolderPicker>>,
    next_id: u64,
}

/// When a later launch asked, and when Noura last tried.
struct Forward {
    asked: Instant,
    tried: Option<Instant>,
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
            forward: None,
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

    /// Bring the window that had focus last to the front when a later
    /// launch asks, and keep at it until that window has focus.
    ///
    /// This runs in every pass, including the passes where every window is
    /// minimized or covered and eframe skips the ui. The system focuses
    /// only a window that is no longer minimized, so a minimized window is
    /// restored first and focused once it is back.
    fn come_forward(&mut self, ctx: &Context) {
        let now = Instant::now();
        if self
            .instance
            .as_ref()
            .is_some_and(Instance::take_activation)
        {
            self.forward = Some(Forward {
                asked: now,
                tried: None,
            });
        }
        if self.forward.is_none() {
            return;
        }
        let Some(id) = self.forward_target() else {
            self.forward = None;
            return;
        };
        let info = ctx.input(|input| input.raw.viewports.get(&id).cloned());
        let Some(forward) = &mut self.forward else {
            return;
        };
        if info.is_some_and(|info| info.focused == Some(true) && info.minimized != Some(true)) {
            self.forward = None;
            return;
        }
        if now.duration_since(forward.asked) > FORWARD_PATIENCE {
            tracing::debug!("the window a later launch asked for never took focus");
            self.forward = None;
            return;
        }
        if forward
            .tried
            .is_none_or(|tried| now.duration_since(tried) >= FORWARD_RETRY)
        {
            forward.tried = Some(now);
            ctx.send_viewport_cmd_to(id, ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd_to(id, ViewportCommand::Focus);
        }
        ctx.request_repaint_after_for(FORWARD_RETRY, ViewportId::ROOT);
    }

    /// The window that had focus last, or another open window when that
    /// one closed.
    fn forward_target(&self) -> Option<ViewportId> {
        let recent = *self.recent.lock();
        let open = |id: ViewportId| {
            (id == ViewportId::ROOT && self.first.is_some())
                || self.others.iter().any(|window| window.id == id)
        };
        if open(recent) {
            Some(recent)
        } else if self.first.is_some() {
            Some(ViewportId::ROOT)
        } else {
            self.others.last().map(|window| window.id)
        }
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
    /// eframe calls this before every `ui`, and also alone while every
    /// window is minimized or covered.
    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.come_forward(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn on_exit(&mut self) {
        // A later launch from now on finds no copy to bring forward. It
        // waits for the lock, which goes once every window has let go of
        // the workspace, and then runs.
        if let Some(instance) = &mut self.instance {
            instance.stop_listening();
        }
        self.quit();
        self.instance = None;
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        time::{Duration, Instant},
    };

    use egui::{Key, Modifiers, ViewportIdMap, ViewportInfo, accesskit::Role};
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
    /// read what the desktop would show from their output. Each step runs
    /// `App::logic` and then `App::ui`, as eframe does.
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
            .build_eframe(|_creation| windows);
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

    /// Run a pass as eframe does while every window is minimized or
    /// covered: `App::logic` alone, which no ui follows. Returns the
    /// commands each window was sent.
    fn logic_pass(
        harness: &mut Harness<'static, Windows>,
        viewports: &[(ViewportId, ViewportInfo)],
    ) -> ViewportIdMap<Vec<ViewportCommand>> {
        let input = egui::RawInput {
            viewports: viewports.iter().cloned().collect(),
            ..Default::default()
        };
        let ctx = harness.ctx.clone();
        let mut frame = eframe::Frame::_new_kittest();
        let output = ctx.run_logic(&input, |ctx| {
            eframe::App::logic(harness.state_mut(), ctx, &mut frame);
        });
        output.viewport_commands.into_iter().collect()
    }

    fn window(minimized: bool, focused: bool) -> ViewportInfo {
        ViewportInfo {
            minimized: Some(minimized),
            occluded: Some(minimized),
            focused: Some(focused),
            ..Default::default()
        }
    }

    #[test]
    fn a_later_launch_brings_a_minimized_window_back_with_no_ui_running() {
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

        // Both windows are minimized: the window comes back first.
        let root = (ViewportId::ROOT, window(true, false));
        let commands = logic_pass(&mut harness, &[root.clone(), (second, window(true, false))]);
        let sent = commands.get(&second).cloned().unwrap_or_default();
        assert!(sent.contains(&ViewportCommand::Minimized(false)));
        assert!(sent.contains(&ViewportCommand::Focus));
        assert!(!commands.contains_key(&ViewportId::ROOT));

        // It is back but the system didn't focus it yet: Noura asks again.
        std::thread::sleep(FORWARD_RETRY);
        let commands = logic_pass(
            &mut harness,
            &[root.clone(), (second, window(false, false))],
        );
        let sent = commands.get(&second).cloned().unwrap_or_default();
        assert!(sent.contains(&ViewportCommand::Focus));

        // Once it has focus, Noura stops asking.
        logic_pass(&mut harness, &[root.clone(), (second, window(false, true))]);
        assert!(harness.state().forward.is_none());
        std::thread::sleep(FORWARD_RETRY);
        let commands = logic_pass(&mut harness, &[root, (second, window(false, true))]);
        assert!(!commands.contains_key(&second));
    }

    /// Quitting lets go of the lock once every window has let go of the
    /// workspace, so the next launch runs.
    #[test]
    fn quitting_lets_the_next_launch_run() {
        let app_data = workspace();
        let Ok(Claim::First(instance)) = crate::instance::claim(app_data.path()) else {
            panic!("nothing else runs on this app data folder");
        };
        let mut harness = harness_with(app_data.path(), Some(instance));
        open_new_window(&mut harness);

        eframe::App::on_exit(harness.state_mut());

        assert!(harness.state().first.is_none());
        assert!(harness.state().others.is_empty());
        assert!(matches!(
            crate::instance::claim(app_data.path()),
            Ok(Claim::First(_))
        ));
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
