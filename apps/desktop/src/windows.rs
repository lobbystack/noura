//! Noura's windows. The root viewport shows the first window, and every
//! other window is a deferred egui viewport that draws on its own. All of
//! them share one `app-core` session.
//!
//! eframe quits when the root viewport closes, so closing the first
//! window while others stay open hides the root viewport instead, and it
//! keeps hosting the others. Closing the last window quits, and so does
//! Quit, which keeps every window for the next launch. A window closes
//! once its notes are saved, and Quit waits for every window's notes.
//!
//! The shell around the windows lives here too: the native menu bar,
//! whose choices go to the window that had focus last, the Edit items'
//! input, file drags, and a later launch's request to come forward.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        mpsc::{Receiver, Sender, channel},
    },
    time::{Duration, Instant},
};

use app_core::App;
use app_ui::{FolderPicker, NouraUi};
use eframe::egui::{
    self, Context, Pos2, ViewportBuilder, ViewportCommand, ViewportId, ViewportInfo, mutex::Mutex,
    vec2,
};

use crate::{
    file_drag::{DragSource, FileDrag},
    instance::Instance,
    menu::{Choice, EditInput},
};

/// How far each new window sits from the window that opened it.
const CASCADE: f32 = 28.0;

/// How often Noura lets the windows that wait for notes to save catch up,
/// in case nothing else wakes it.
const WAITING_CHECK: Duration = Duration::from_millis(100);

/// How long Noura keeps bringing a window forward after a later launch
/// asks, and how often it tries again. A minimized window animates back
/// before it can take focus, and the system may refuse focus at first.
const FORWARD_PATIENCE: Duration = Duration::from_secs(3);
const FORWARD_RETRY: Duration = Duration::from_millis(200);

/// How often Noura looks for a new window's native window to give it the
/// menu bar, on Windows, until it has one.
#[cfg(target_os = "windows")]
const MENU_RETRY: Duration = Duration::from_millis(100);

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
    /// The root viewport hides once the first window closed while others
    /// stay open.
    root_hidden: bool,
    /// Quit ran: the root viewport may close with every window open.
    quitting: bool,
    /// Quit ran, the root viewport's close went through, or the last
    /// window closed: eframe ends the app, so this copy answers no later
    /// launch.
    ending: bool,
    /// The window that had focus last, which menu choices go to and a
    /// later launch brings forward.
    recent: Arc<Mutex<ViewportId>>,
    instance: Option<Instance>,
    /// A later launch asked for a window to come forward, and it hasn't
    /// come yet.
    forward: Option<Forward>,
    /// The window that came forward to ask about a note that couldn't
    /// save before it, or every window, closed.
    asking: Option<ViewportId>,
    picker: Box<dyn Fn() -> Box<dyn FolderPicker>>,
    next_id: u64,
    /// Menu choices, sent from muda's event handler.
    choices: Receiver<Choice>,
    choice_sender: Sender<Choice>,
    edit: EditInput,
    /// Each window's file drags.
    file_drags: Arc<Mutex<HashMap<ViewportId, FileDrag>>>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    native_menu: Option<crate::menu::native::NativeMenu>,
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
        let (choice_sender, choices) = channel();
        Self {
            first: Some(NouraUi::new(first, picker())),
            others: Vec::new(),
            root_hidden: false,
            quitting: false,
            ending: false,
            recent: Arc::new(Mutex::new(ViewportId::ROOT)),
            instance,
            forward: None,
            asking: None,
            picker: Box::new(picker),
            next_id: 0,
            choices,
            choice_sender,
            edit: EditInput::default(),
            file_drags: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            native_menu: None,
        }
    }

    /// Send menu choices to the windows, and on macOS and Windows show the
    /// menu bar.
    pub fn install_menu_bar(&mut self, cc: &eframe::CreationContext<'_>) {
        let sender = self.choice_sender.clone();
        let ctx = cc.egui_ctx.clone();
        muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
            if let Some(choice) = crate::menu::choice_for(event.id())
                && sender.send(choice).is_ok()
            {
                ctx.request_repaint_of(ViewportId::ROOT);
            }
        }));
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(menus) = self.menu_bar() {
            self.native_menu = match crate::menu::native::install(cc, &menus) {
                Ok(native_menu) => Some(native_menu),
                Err(error) => {
                    tracing::error!(%error, "the native menu bar failed to install");
                    None
                }
            };
        }
    }

    /// Draw the first window in the root viewport, and keep the other
    /// windows on screen.
    fn show(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(first) = &mut self.first {
            first.ui(ui);
            note_focus(&ctx, ViewportId::ROOT, &self.recent);
            self.file_drags
                .lock()
                .entry(ViewportId::ROOT)
                .or_default()
                .after_frame(first, &ctx, DragSource::Main(frame));
        }
        if !self.settle(&ctx) {
            return;
        }
        for window in &self.others {
            window.show(&ctx, &self.recent, &self.file_drags);
        }
    }

    /// Show the windows the open ones opened, forget the ones that closed,
    /// and hide the root viewport once the first window closed. Returns
    /// false once the last window closed: Noura quits, and that window
    /// comes back at the next launch.
    fn settle(&mut self, ctx: &Context) -> bool {
        let mut opened = Vec::new();
        if let Some(first) = &mut self.first {
            let new = first.take_new_windows();
            if !new.is_empty() {
                let origin = ctx.input_for(ViewportId::ROOT, |input| {
                    input.viewport().outer_rect.map(|rect| rect.min)
                });
                opened.extend(new.into_iter().map(|app| (app, origin)));
            }
            if first.is_closed() {
                self.first = None;
                self.file_drags.lock().remove(&ViewportId::ROOT);
            }
        }
        for window in &self.others {
            let mut state = window.state.lock();
            let origin = state.origin;
            opened.extend(state.opened.drain(..).map(|app| (app, origin)));
        }
        let file_drags = &self.file_drags;
        self.others.retain(|window| {
            let open = !window.is_closed();
            if !open {
                file_drags.lock().remove(&window.id);
            }
            open
        });
        let mut cascade = 0.0;
        for (app, origin) in opened {
            cascade += CASCADE;
            let position = origin.map(|origin| origin + vec2(cascade, cascade));
            self.add(app, position);
        }
        if self.first.is_none() && self.others.is_empty() {
            self.ending = true;
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            return false;
        }
        if self.first.is_none() && !self.root_hidden {
            // The other windows stay, so the root viewport hides and
            // keeps hosting them.
            self.root_hidden = true;
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Visible(false));
        }
        true
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

    /// Close the windows whose close button was pressed, each once its
    /// notes are saved. The root viewport's close ends the app unless
    /// other windows stay open, or the first window has notes to save:
    /// then the first window closes once they are, which ends the app if
    /// it was the last. Quit lets it end the app with every window open.
    ///
    /// This runs in every pass, including the passes where every window
    /// is minimized or covered and eframe skips the ui, as when the first
    /// window closes from the taskbar.
    fn close_requested(&mut self, ctx: &Context) {
        let requested = |id: ViewportId| {
            ctx.input(|input| {
                input
                    .raw
                    .viewports
                    .get(&id)
                    .is_some_and(ViewportInfo::close_requested)
            })
        };
        if requested(ViewportId::ROOT) {
            let first_waits = self
                .first
                .as_ref()
                .is_some_and(NouraUi::must_save_before_closing);
            if !self.quitting && !self.ending && (!self.others.is_empty() || first_waits) {
                ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::CancelClose);
                if let Some(first) = &mut self.first {
                    first.close_window();
                }
            } else {
                // eframe ends the app after this pass.
                self.ending = true;
            }
        }
        for window in &self.others {
            if requested(window.id)
                && let Some(ui) = &mut window.state.lock().ui
            {
                ui.close_window();
            }
        }
    }

    /// Keep the windows that wait for notes to save going, including
    /// windows that draw no frame, such as minimized ones: a window
    /// closing, or every window during a quit or a workspace switch. Each
    /// one saves its own notes, and the window that asked for a quit or a
    /// switch goes ahead once all have. A window that asks about a note
    /// that couldn't save comes forward.
    fn keep_up_waiting(&mut self, ctx: &Context) {
        let mut waiting = false;
        let mut asking = None;
        if let Some(first) = &mut self.first
            && first.is_waiting()
        {
            waiting = true;
            first.keep_up(ctx);
            if first.is_asking() {
                asking = Some(ViewportId::ROOT);
            }
        }
        for window in &self.others {
            if let Some(ui) = &mut window.state.lock().ui
                && ui.is_waiting()
            {
                waiting = true;
                ui.keep_up(ctx);
                if ui.is_asking() {
                    asking = asking.or(Some(window.id));
                }
            }
        }
        if let Some(id) = asking
            && self.asking != Some(id)
        {
            ctx.send_viewport_cmd_to(id, ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd_to(id, ViewportCommand::Focus);
        }
        self.asking = asking;
        if waiting {
            ctx.request_repaint_after_for(WAITING_CHECK, ViewportId::ROOT);
        }
    }

    /// Start every window's note saves as they come due, including in
    /// windows that draw no frame, such as minimized, hidden or covered
    /// ones: eframe runs no ui for them, only this logic.
    fn keep_saving(&mut self, ctx: &Context) {
        if let Some(first) = &mut self.first {
            first.keep_saving(ctx);
        }
        for window in &self.others {
            if let Some(ui) = &mut window.state.lock().ui {
                ui.keep_saving(ctx);
            }
        }
    }

    /// Save every window's notes and wait for the saves, as Noura ends
    /// without its own Quit, as when the system quits it at logout. No
    /// window can ask about a save that fails then.
    fn save_before_exit(&mut self) {
        let mut saved = self.first.as_mut().is_none_or(NouraUi::save_before_exit);
        for window in &self.others {
            if let Some(ui) = &mut window.state.lock().ui {
                saved &= ui.save_before_exit();
            }
        }
        if !saved {
            tracing::error!("Noura ended with a note it couldn't save");
        }
    }

    /// Whether Quit ran in any window.
    fn quit_requested(&self) -> bool {
        self.first.as_ref().is_some_and(NouraUi::is_quitting)
            || self.others.iter().any(|window| {
                window
                    .state
                    .lock()
                    .ui
                    .as_ref()
                    .is_some_and(NouraUi::is_quitting)
            })
    }

    /// Run the menu choices made since the last pass: commands in the
    /// window that had focus last, and the Edit items in the focused text
    /// field.
    fn run_menu_choices(&mut self, ctx: &Context) {
        while let Ok(choice) = self.choices.try_recv() {
            match choice {
                Choice::Command(id) => self.in_recent_window(|window| {
                    window.run_menu_command(&id, Instant::now());
                    // Quit from the menu works while the windows are
                    // hidden too.
                    window.close_requested_windows(ctx);
                }),
                Choice::System(item) => self.edit.run(ctx, item),
            }
        }
    }

    /// Run `run` on the window that had focus last, or on another open
    /// window when that one closed.
    fn in_recent_window(&mut self, run: impl FnOnce(&mut NouraUi)) {
        let Some(id) = self.recent_window() else {
            return;
        };
        if id == ViewportId::ROOT {
            if let Some(first) = &mut self.first {
                run(first);
            }
        } else if let Some(window) = self.others.iter().find(|window| window.id == id)
            && let Some(ui) = &mut window.state.lock().ui
        {
            run(ui);
        }
    }

    /// The menu bar of the window that had focus last. Every window lists
    /// the same commands.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn menu_bar(&self) -> Option<Vec<app_core::Menu>> {
        let id = self.recent_window()?;
        if id == ViewportId::ROOT {
            return self.first.as_ref().map(NouraUi::menu_bar);
        }
        let window = self.others.iter().find(|window| window.id == id)?;
        window.state.lock().ui.as_ref().map(NouraUi::menu_bar)
    }

    /// Show the menus the registry lists now, as when a command registered
    /// or a page moved, and on Windows give each new window the menu bar.
    fn update_menu_bar(&mut self, ctx: &Context) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let menus = self.menu_bar();
            let Some(native_menu) = &mut self.native_menu else {
                return;
            };
            if let Some(menus) = menus
                && let Err(error) = native_menu.update(&menus)
            {
                tracing::error!(%error, "the native menu bar failed to update");
            }
            #[cfg(target_os = "windows")]
            match native_menu.cover_windows() {
                // A new window's native window comes a pass or two after
                // its viewport.
                Ok(covered) if covered < self.others.len() => {
                    ctx.request_repaint_after_for(MENU_RETRY, ViewportId::ROOT);
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!(%error, "a window's menu bar failed to install");
                }
            }
        }
        let _ = ctx;
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
        let Some(id) = self.recent_window() else {
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

    /// Stop answering later launches once this copy is ending. A launch
    /// that heard this copy came forward would exit with nothing left
    /// running, so a request waiting now gets no answer, and a launch from
    /// now on finds no one listening. Either one waits for the lock and
    /// runs once this copy is gone.
    fn stop_answering(&mut self) {
        self.forward = None;
        if let Some(instance) = &mut self.instance {
            instance.stop_listening();
        }
    }

    /// The window that had focus last, or another open window when that
    /// one closed.
    fn recent_window(&self) -> Option<ViewportId> {
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

    fn show(
        &self,
        ctx: &Context,
        recent: &Arc<Mutex<ViewportId>>,
        file_drags: &Arc<Mutex<HashMap<ViewportId, FileDrag>>>,
    ) {
        let state = self.state.clone();
        let recent = recent.clone();
        let file_drags = file_drags.clone();
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
            file_drags
                .lock()
                .entry(id)
                .or_default()
                .after_frame(window, ctx, DragSource::Focused);
            let opened = window.take_new_windows();
            if !opened.is_empty() {
                state.origin = ctx.input(|input| input.viewport().outer_rect.map(|rect| rect.min));
                state.opened.extend(opened);
            }
            if window.is_closed() || window.is_quitting() || !state.opened.is_empty() {
                // The root viewport opens and closes windows, and quits.
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
        self.run_menu_choices(ctx);
        self.keep_saving(ctx);
        self.keep_up_waiting(ctx);
        if self.quit_requested() {
            self.quitting = true;
            self.ending = true;
        }
        self.close_requested(ctx);
        if !self.settle(ctx) || self.ending {
            self.stop_answering();
            return;
        }
        self.come_forward(ctx);
        self.update_menu_bar(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.show(ui, frame);
    }

    fn raw_input_hook(&mut self, ctx: &Context, raw_input: &mut egui::RawInput) {
        self.file_drags
            .lock()
            .entry(raw_input.viewport_id)
            .or_default()
            .complete_input(ctx, raw_input);
        self.edit.inject(raw_input);
    }

    /// eframe calls this as Noura ends: after Quit, which saved every
    /// note first, or when the system quits Noura, as with Quit in the
    /// Dock or at logout. winit ends Noura then without asking, and Noura
    /// doesn't take the system's quit request itself yet, so the notes
    /// save here, and a note that can't save loses the edits that aren't
    /// on disk. On macOS that request is the quit Apple event, which
    /// `NSAppleEventManager` could hand to Noura's own Quit, which asks.
    fn on_exit(&mut self) {
        // A later launch from now on finds no copy to bring forward. It
        // waits for the lock, which goes once every window has let go of
        // the workspace, and then runs.
        if let Some(instance) = &mut self.instance {
            instance.stop_listening();
        }
        self.save_before_exit();
        self.quit();
        self.instance = None;
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        thread::JoinHandle,
        time::{Duration, Instant},
    };

    use app_core::command_id;
    use egui::{Key, Modifiers, ViewportEvent, ViewportIdMap, ViewportInfo};
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
            // The palette draws ⌘ and ⇧, which egui's bundled fonts lack;
            // the desktop takes them from the system fonts.
            .allow_missing_glyphs()
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

    /// Press `key` with Cmd (Ctrl off macOS) and `modifiers` in the first
    /// window, and return what the root viewport was told in that frame.
    /// kittest runs a frame per event, so the key comes down alone.
    fn press(
        harness: &mut Harness<'static, Windows>,
        modifiers: Modifiers,
        key: Key,
    ) -> Vec<ViewportCommand> {
        harness.event(egui::Event::Key {
            key,
            pressed: true,
            modifiers: Modifiers::COMMAND | modifiers,
            repeat: false,
            physical_key: None,
        });
        harness.step();
        root_commands(harness)
    }

    /// Press Cmd-Shift-W (Ctrl-Shift-W off macOS) in the first window.
    fn close_first_window(harness: &mut Harness<'static, Windows>) -> Vec<ViewportCommand> {
        press(harness, Modifiers::SHIFT, Key::W)
    }

    /// Run New Window from the first window's command palette.
    fn open_new_window(harness: &mut Harness<'static, Windows>) {
        harness.key_press_modifiers(Modifiers::COMMAND, Key::K);
        harness.run();
        harness
            .get_by_label("Search commands")
            .type_text("new window");
        harness.run();
        harness.key_press(Key::Enter);
        harness.run();
    }

    /// Choose the menu item of the command `id`, as muda reports it.
    fn choose(harness: &mut Harness<'static, Windows>, id: &str) {
        let choice = Choice::Command(id.to_owned());
        harness.state().choice_sender.send(choice).unwrap();
        harness.run();
    }

    fn second_window(harness: &Harness<'static, Windows>) -> Arc<Mutex<WindowState>> {
        harness.state().others[0].state.clone()
    }

    #[test]
    fn new_window_from_the_palette_opens_another_viewport() {
        let app_data = workspace();
        let mut harness = harness(app_data.path());
        assert_eq!(other_viewports(&harness), 0);

        open_new_window(&mut harness);

        assert_eq!(other_viewports(&harness), 1);
        assert_eq!(harness.state().others.len(), 1);
    }

    #[test]
    fn new_window_from_the_menu_opens_another_viewport() {
        let app_data = workspace();
        let mut harness = harness(app_data.path());

        choose(&mut harness, command_id::NEW_WINDOW);

        assert_eq!(other_viewports(&harness), 1);
    }

    /// The menu bar is the app's on macOS, so a menu choice goes to the
    /// window that had focus last.
    #[test]
    fn a_menu_choice_runs_in_the_window_that_had_focus_last() {
        let app_data = workspace();
        let mut harness = harness(app_data.path());
        open_new_window(&mut harness);
        let second = harness.state().others[0].id;
        *harness.state().recent.lock() = second;

        choose(&mut harness, command_id::CLOSE_WINDOW);

        assert!(harness.state().others.is_empty());
        assert!(harness.state().first.is_some());
        assert_eq!(other_viewports(&harness), 0);
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

        second_window(&harness)
            .lock()
            .ui
            .as_mut()
            .unwrap()
            .close_window();
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

    /// A minimized window that its taskbar button or Dock menu asks to
    /// close.
    fn closing() -> ViewportInfo {
        ViewportInfo {
            events: vec![ViewportEvent::Close],
            ..window(true, false)
        }
    }

    fn cancels_close(commands: &ViewportIdMap<Vec<ViewportCommand>>) -> bool {
        commands
            .get(&ViewportId::ROOT)
            .is_some_and(|commands| commands.contains(&ViewportCommand::CancelClose))
    }

    /// eframe runs no ui while every window is minimized, so the root
    /// viewport's close request must be answered in `App::logic`, or the
    /// whole app quits.
    #[test]
    fn closing_the_minimized_first_window_keeps_the_others_with_no_ui_running() {
        let app_data = workspace();
        let mut harness = harness(app_data.path());
        open_new_window(&mut harness);
        let second = harness.state().others[0].id;

        let commands = logic_pass(
            &mut harness,
            &[(ViewportId::ROOT, closing()), (second, window(true, false))],
        );

        assert!(cancels_close(&commands), "the app keeps running");
        assert!(harness.state().first.is_none(), "the first window closed");
        assert_eq!(harness.state().others.len(), 1);
        assert!(
            !second_window(&harness)
                .lock()
                .ui
                .as_ref()
                .unwrap()
                .is_closed()
        );
    }

    #[test]
    fn closing_a_minimized_other_window_closes_it_with_no_ui_running() {
        let app_data = workspace();
        let mut harness = harness(app_data.path());
        open_new_window(&mut harness);
        let second = harness.state().others[0].id;

        let commands = logic_pass(
            &mut harness,
            &[(ViewportId::ROOT, window(true, false)), (second, closing())],
        );

        assert!(!cancels_close(&commands));
        assert!(harness.state().others.is_empty());
        assert!(harness.state().first.is_some());
    }

    /// Quit closes the root viewport, which ends the app, even with other
    /// windows open, and every window comes back at the next launch.
    #[test]
    fn quitting_with_two_windows_open_ends_the_app_and_keeps_both() {
        let app_data = workspace();
        let mut harness = harness(app_data.path());
        open_new_window(&mut harness);
        let second = harness.state().others[0].id;
        *harness.state().recent.lock() = second;

        // Quit from the second window's menu: the root viewport closes.
        choose(&mut harness, command_id::QUIT);
        assert!(harness.state().quitting);
        let commands = logic_pass(
            &mut harness,
            &[
                (
                    ViewportId::ROOT,
                    ViewportInfo {
                        events: vec![ViewportEvent::Close],
                        ..window(false, false)
                    },
                ),
                (second, window(false, true)),
            ],
        );
        assert!(!cancels_close(&commands), "Quit ends the app");
        assert!(harness.state().first.is_some(), "no window closed first");
        assert_eq!(harness.state().others.len(), 1);

        eframe::App::on_exit(harness.state_mut());
        drop(harness);

        let mut next = App::launch(app_data.path());
        let deadline = Instant::now() + Duration::from_secs(20);
        while next.workspace().is_none() {
            assert!(Instant::now() < deadline, "the workspace never reopened");
            next.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            next.take_opened_windows().len(),
            1,
            "both windows came back"
        );
    }

    #[test]
    fn quit_from_the_first_window_s_keys_lets_the_root_viewport_close() {
        let app_data = workspace();
        let mut harness = harness(app_data.path());
        open_new_window(&mut harness);
        let second = harness.state().others[0].id;

        let commands = press(&mut harness, Modifiers::NONE, Key::Q);
        assert!(commands.contains(&ViewportCommand::Close));
        let commands = logic_pass(
            &mut harness,
            &[
                (
                    ViewportId::ROOT,
                    ViewportInfo {
                        events: vec![ViewportEvent::Close],
                        ..window(false, true)
                    },
                ),
                (second, window(false, false)),
            ],
        );

        assert!(!cancels_close(&commands));
        assert!(harness.state().first.is_some());
    }

    /// Launch again on another thread, as a second process would.
    fn launch(app_data: &std::path::Path) -> JoinHandle<std::io::Result<Claim>> {
        let app_data = app_data.to_owned();
        std::thread::spawn(move || crate::instance::claim(&app_data))
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

        let later = launch(app_data.path());

        // Both windows are minimized: the window comes back first.
        let root = (ViewportId::ROOT, window(true, false));
        let deadline = Instant::now() + Duration::from_secs(10);
        let sent = loop {
            let commands = logic_pass(&mut harness, &[root.clone(), (second, window(true, false))]);
            assert!(!commands.contains_key(&ViewportId::ROOT));
            if let Some(sent) = commands.get(&second) {
                break sent.clone();
            }
            assert!(Instant::now() < deadline, "no launch asked to come forward");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(sent.contains(&ViewportCommand::Minimized(false)));
        assert!(sent.contains(&ViewportCommand::Focus));
        // The running copy answered once it took the request.
        assert!(matches!(later.join().unwrap(), Ok(Claim::Running)));

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

    /// Wait until a later launch's request reaches the running copy.
    fn wait_until_asked(harness: &Harness<'static, Windows>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !harness.state().instance.as_ref().unwrap().is_asked() {
            assert!(Instant::now() < deadline, "no launch asked to come forward");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The root viewport's close button, pressed or sent by Quit.
    fn root_closing(focused: bool) -> (ViewportId, ViewportInfo) {
        (
            ViewportId::ROOT,
            ViewportInfo {
                events: vec![ViewportEvent::Close],
                ..window(false, focused)
            },
        )
    }

    /// A launch while Noura quits hears nothing from it, whether its
    /// request came before Quit or after. It waits for the lock and runs
    /// once the copy is gone, instead of exiting with no copy running.
    #[test]
    fn a_launch_while_noura_quits_runs_once_it_is_gone() {
        let app_data = workspace();
        let Ok(Claim::First(instance)) = crate::instance::claim(app_data.path()) else {
            panic!("nothing else runs on this app data folder");
        };
        let mut harness = harness_with(app_data.path(), Some(instance));
        open_new_window(&mut harness);
        let second = harness.state().others[0].id;
        let before = launch(app_data.path());
        wait_until_asked(&harness);

        choose(&mut harness, command_id::QUIT);
        let after = launch(app_data.path());
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            let commands = logic_pass(
                &mut harness,
                &[root_closing(true), (second, window(false, false))],
            );
            assert!(!cancels_close(&commands), "Quit ends the app");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!before.is_finished(), "the launch heard a quitting copy");
        assert!(!after.is_finished(), "the launch heard a quitting copy");
        eframe::App::on_exit(harness.state_mut());
        drop(harness);

        // One launch runs, and the other asks it to come forward.
        let (Ok(before), Ok(after)) = (before.join().unwrap(), after.join().unwrap()) else {
            panic!("a launch failed");
        };
        let running: Vec<_> = [before, after]
            .into_iter()
            .filter_map(|claim| match claim {
                Claim::First(instance) => Some(instance),
                Claim::Running => None,
            })
            .collect();
        assert_eq!(running.len(), 1, "one launch runs");
    }

    /// Closing the only window ends the app, so a request that waits then
    /// gets no answer, and its launch runs once the copy is gone.
    #[test]
    fn a_launch_while_the_only_window_closes_runs_once_it_is_gone() {
        let app_data = workspace();
        let Ok(Claim::First(instance)) = crate::instance::claim(app_data.path()) else {
            panic!("nothing else runs on this app data folder");
        };
        let mut harness = harness_with(app_data.path(), Some(instance));
        let later = launch(app_data.path());
        wait_until_asked(&harness);

        let commands = logic_pass(&mut harness, &[root_closing(true)]);

        assert!(
            !cancels_close(&commands),
            "closing the only window ends the app"
        );
        eframe::App::on_exit(harness.state_mut());
        drop(harness);
        assert!(matches!(later.join().unwrap(), Ok(Claim::First(_))));
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

        let later = launch(app_data.path());

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
        assert!(matches!(later.join().unwrap(), Ok(Claim::Running)));
    }

    /// A workspace with a note at Docs/Plan.md.
    fn workspace_with_note() -> tempfile::TempDir {
        let app_data = workspace();
        let docs = app_data.path().join("Current").join("Docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("Plan.md"), "Plan\n").unwrap();
        app_data
    }

    fn plan(app_data: &std::path::Path) -> String {
        std::fs::read_to_string(app_data.join("Current/Docs/Plan.md")).unwrap()
    }

    /// Show Docs/Plan.md in `window` and type in it, without saving yet.
    fn type_in_plan(window: &mut NouraUi) {
        type_in(window, "Docs/Plan.md");
    }

    /// Show the note at `relative_path` in `window` and type in it,
    /// without saving yet.
    fn type_in(window: &mut NouraUi, relative_path: &str) {
        let app = window.app_mut();
        app.open_file(relative_path, app_core::OpenAs::Kept);
        let tab = app.active_tab().unwrap().id();
        let text = format!("{}typed\n", app.note(tab).unwrap().text());
        app.edit_note(tab, 1, &text, Instant::now());
    }

    /// Type in the second window's note.
    fn type_in_second_window(harness: &Harness<'static, Windows>) {
        type_in_plan(second_window(harness).lock().ui.as_mut().unwrap());
    }

    /// Hold the workspace's write lock, as another Noura process writing
    /// would, until the returned file is unlocked or dropped. Saves wait
    /// meanwhile.
    fn hold_writes(
        harness: &Harness<'static, Windows>,
        app_data: &std::path::Path,
    ) -> std::fs::File {
        let id = harness
            .state()
            .first
            .as_ref()
            .unwrap()
            .app()
            .workspace()
            .unwrap()
            .id()
            .to_owned();
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(app_data.join("workspaces").join(id).join("workspace.lock"))
            .unwrap();
        lock.lock().unwrap();
        lock
    }

    /// Run passes as eframe does while both windows are minimized, until
    /// `done`.
    fn pass_until(
        harness: &mut Harness<'static, Windows>,
        second: ViewportId,
        done: impl Fn(&Windows) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(harness.state()) {
            assert!(Instant::now() < deadline, "the windows never settled");
            logic_pass(
                harness,
                &[
                    (ViewportId::ROOT, window(true, false)),
                    (second, window(true, false)),
                ],
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Quit from the first window waits for the note typed in the second,
    /// even with both windows minimized and no ui running, then ends the
    /// app.
    #[test]
    fn quit_waits_for_a_note_in_another_window() {
        let app_data = workspace_with_note();
        let mut harness = harness(app_data.path());
        open_new_window(&mut harness);
        let second = harness.state().others[0].id;
        type_in_second_window(&harness);
        let lock = hold_writes(&harness, app_data.path());

        choose(&mut harness, command_id::QUIT);
        for _ in 0..10 {
            let commands = logic_pass(
                &mut harness,
                &[
                    (ViewportId::ROOT, window(true, false)),
                    (second, window(true, false)),
                ],
            );
            assert!(!commands.contains_key(&ViewportId::ROOT));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!harness.state().quitting, "Quit waits for the save");
        assert_eq!(harness.state().others.len(), 1);
        assert_eq!(plan(app_data.path()), "Plan\n");

        lock.unlock().unwrap();
        pass_until(&mut harness, second, |windows| windows.quitting);

        assert!(harness.state().ending);
        assert_eq!(plan(app_data.path()), "Plan\ntyped\n");
    }

    /// A secondary window's close button closes it once its note is
    /// saved, with no ui running, and the app goes on.
    #[test]
    fn closing_a_second_window_waits_for_its_note_and_keeps_the_app() {
        let app_data = workspace_with_note();
        let mut harness = harness(app_data.path());
        open_new_window(&mut harness);
        let second = harness.state().others[0].id;
        type_in_second_window(&harness);
        let lock = hold_writes(&harness, app_data.path());

        let commands = logic_pass(
            &mut harness,
            &[(ViewportId::ROOT, window(true, false)), (second, closing())],
        );

        assert!(!commands.contains_key(&ViewportId::ROOT), "Noura goes on");
        assert_eq!(harness.state().others.len(), 1, "the window waits");
        assert!(!harness.state().quitting && !harness.state().ending);

        lock.unlock().unwrap();
        pass_until(&mut harness, second, |windows| windows.others.is_empty());

        assert!(harness.state().first.is_some());
        assert!(!harness.state().quitting && !harness.state().ending);
        assert_eq!(plan(app_data.path()), "Plan\ntyped\n");
    }

    /// The only window's close button waits for its note, then ends the
    /// app.
    #[test]
    fn closing_the_only_window_waits_for_its_note() {
        let app_data = workspace_with_note();
        let mut harness = harness(app_data.path());
        type_in_plan(harness.state_mut().first.as_mut().unwrap());
        let lock = hold_writes(&harness, app_data.path());

        let commands = logic_pass(&mut harness, &[root_closing(true)]);

        assert!(cancels_close(&commands), "the window waits for the save");
        assert!(harness.state().first.is_some());
        lock.unlock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !harness.state().ending {
            assert!(Instant::now() < deadline, "the window never closed");
            logic_pass(&mut harness, &[(ViewportId::ROOT, window(false, true))]);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(harness.state().first.is_none());
        assert_eq!(plan(app_data.path()), "Plan\ntyped\n");
    }

    /// A note that can't save in the second window stops Quit: that window
    /// comes forward to ask, and nothing closes until you choose.
    #[cfg(unix)]
    #[test]
    fn a_note_that_cant_save_in_another_window_asks_there_before_quitting() {
        use std::os::unix::fs::PermissionsExt;

        let app_data = workspace_with_note();
        let docs = app_data.path().join("Current/Docs");
        let mut harness = harness(app_data.path());
        open_new_window(&mut harness);
        let second = harness.state().others[0].id;
        type_in_second_window(&harness);
        std::fs::set_permissions(&docs, std::fs::Permissions::from_mode(0o555)).unwrap();

        // Quit from the menu while the second window is minimized.
        let quit = Choice::Command(command_id::QUIT.to_owned());
        harness.state().choice_sender.send(quit).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let sent = loop {
            let commands = logic_pass(
                &mut harness,
                &[
                    (ViewportId::ROOT, window(false, true)),
                    (second, window(true, false)),
                ],
            );
            if let Some(sent) = commands.get(&second) {
                break sent.clone();
            }
            assert!(Instant::now() < deadline, "the window never asked");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(sent.contains(&ViewportCommand::Minimized(false)));
        assert!(sent.contains(&ViewportCommand::Focus));
        assert!(!harness.state().quitting, "nothing closes until you choose");
        let state = second_window(&harness);
        assert!(state.lock().ui.as_ref().unwrap().is_asking());

        state
            .lock()
            .ui
            .as_mut()
            .unwrap()
            .app_mut()
            .discard_and_leave();
        pass_until(&mut harness, second, |windows| windows.quitting);

        std::fs::set_permissions(&docs, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(plan(app_data.path()), "Plan\n");
    }

    /// Typing saves after its pause even when every window is minimized
    /// and eframe runs `App::logic` alone, in the first window and in a
    /// deferred one.
    #[test]
    fn notes_save_after_typing_pauses_with_every_window_minimized() {
        let app_data = workspace_with_note();
        std::fs::write(app_data.path().join("Current/Docs/Other.md"), "Other\n").unwrap();
        let mut harness = harness(app_data.path());
        open_new_window(&mut harness);
        let second = harness.state().others[0].id;
        type_in_plan(harness.state_mut().first.as_mut().unwrap());
        type_in(
            second_window(&harness).lock().ui.as_mut().unwrap(),
            "Docs/Other.md",
        );

        let other = app_data.path().join("Current/Docs/Other.md");
        let folder = app_data.path().to_owned();
        pass_until(&mut harness, second, |_| {
            plan(&folder) == "Plan\ntyped\n"
                && std::fs::read_to_string(&other).unwrap() == "Other\ntyped\n"
        });

        assert!(!harness.state().quitting && !harness.state().ending);
    }

    /// When the system quits Noura, as at logout, eframe calls `on_exit`
    /// with no Quit before it: every window's notes save then.
    #[test]
    fn an_exit_noura_didn_t_ask_for_saves_every_window_s_notes() {
        let app_data = workspace_with_note();
        std::fs::write(app_data.path().join("Current/Docs/Other.md"), "Other\n").unwrap();
        let mut harness = harness(app_data.path());
        open_new_window(&mut harness);
        type_in_plan(harness.state_mut().first.as_mut().unwrap());
        type_in(
            second_window(&harness).lock().ui.as_mut().unwrap(),
            "Docs/Other.md",
        );
        assert_eq!(plan(app_data.path()), "Plan\n");

        eframe::App::on_exit(harness.state_mut());

        assert_eq!(plan(app_data.path()), "Plan\ntyped\n");
        assert_eq!(
            std::fs::read_to_string(app_data.path().join("Current/Docs/Other.md")).unwrap(),
            "Other\ntyped\n"
        );
        assert!(harness.state().first.is_none());
    }
}
