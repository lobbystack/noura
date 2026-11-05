//! The Plugins page and what plugins add: installing a plugin from a
//! folder, the first-run screen for the plugins that ship with Noura,
//! turning a plugin on or off in the open workspace, revoking and allowing
//! what it may use, approving or declining a new build, starting a plugin
//! that failed, the pages, commands, Inbox sections, Settings pages and
//! tools running plugins add, and the properties panel a plugin draws
//! beside an object of its own type. Every window shares the session's
//! plugins service and shows what the plugins it runs add.

use std::{ops::Range, path::PathBuf, sync::mpsc::TryRecvError};

use jiff::{civil::Date, tz::TimeZone};
use plugin_host::{Event, Naming, ViewKind, ViewNode, VirtualRow};

use super::{App, Change};
use crate::{
    AppError, Grant, NoticeKind, OpenAs, PluginInstall, PluginRow, PluginTool, PluginView,
    SettingsGroup, ShippedPlugin, TabId,
    plugins::{Opening, SETTINGS_PREFIX, is_plugin_page},
    settings::settings_section,
};

/// Where a plugin's view shows, for what you type in its name fields.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PluginPlace {
    /// The rail page with this ID.
    Page(String),
    /// The sidebar section of the rail page with this ID.
    Sidebar(String),
    /// The properties panel of the object open in this tab.
    Panel(TabId),
    /// A plugin's Inbox section.
    Inbox { plugin: String, section: String },
    /// A plugin's page in Settings, by its section ID.
    Settings(String),
}

/// What you typed in a plugin's name field, until its plugin took the
/// name and the field went away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameDraft {
    pub place: PluginPlace,
    /// The name field's node ID.
    pub node: String,
    /// The name the field started from.
    pub started: String,
    pub typed: String,
    /// The name went to the plugin, which hasn't answered yet.
    pub sent: bool,
}

impl App {
    /// The Plugins page's rows: every plugin installed on this device or
    /// enabled in the open workspace.
    pub fn plugins(&self) -> Vec<PluginRow> {
        self.session.plugins().rows()
    }

    /// Open the install dialog for the plugin in `folder`. The dialog lists
    /// the capabilities the plugin declares, or says why Noura won't
    /// install it. None of the plugin's code runs until you approve.
    pub fn review_plugin_folder(&mut self, folder: PathBuf) {
        let review = self.session.plugins().review(&folder);
        self.plugin_install = Some(review);
        self.plugin_download = None;
        self.listeners.emit(Change::Plugins);
    }

    /// Open the install dialog for the plugin in the release archive file
    /// `archive`, as `noura-plugin package` writes it, as
    /// [`Self::review_plugin_folder`] does for a folder. Noura compiles the
    /// component itself and never reads the archive again for new builds.
    pub fn review_plugin_archive(&mut self, archive: PathBuf) {
        let review = self.session.plugins().review_archive(&archive);
        self.plugin_install = Some(review);
        self.plugin_download = None;
        self.listeners.emit(Change::Plugins);
    }

    /// Open the install dialog for the release at `address` on GitHub. An
    /// address that isn't a release on GitHub is refused with a message,
    /// and nothing is fetched. Otherwise the dialog shows the release as
    /// downloading, then lists what the plugin declares, with a warning
    /// that nobody reviewed it, once [`App::poll`] takes the download.
    /// None of the plugin's code runs until you approve.
    pub fn review_plugin_release(&mut self, address: &str) {
        let (review, download) = self.session.plugins().review_release(address);
        self.plugin_install = Some(review);
        self.plugin_download = download;
        self.listeners.emit(Change::Plugins);
    }

    /// Show the release that finished downloading in the install dialog.
    fn poll_plugin_download(&mut self) -> bool {
        let Some(download) = &self.plugin_download else {
            return false;
        };
        let review = match download.try_recv() {
            Ok(review) => review,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                let source = self
                    .plugin_install
                    .as_ref()
                    .map(|install| install.source.clone())
                    .unwrap_or_default();
                PluginInstall::download_stopped(source)
            }
        };
        self.plugin_download = None;
        if self
            .plugin_install
            .as_ref()
            .is_some_and(|install| install.downloading)
        {
            self.plugin_install = Some(review);
        }
        self.listeners.emit(Change::Plugins);
        true
    }

    /// Check GitHub for a release of the plugin `id` newer than the one
    /// installed. Its row says how the check went; a newer release waits
    /// there until you install or decline it. Returns whether the check
    /// started.
    pub fn check_plugin_update(&mut self, id: &str) -> bool {
        let started = self.session.plugins().check_update(id);
        self.plugin_result(started, "Couldn't check for updates")
    }

    /// Download plugin releases through `transport` in place of the
    /// network, as tests do with a stand-in for GitHub.
    #[cfg(feature = "test-hooks")]
    pub fn set_plugin_release_transport(
        &mut self,
        transport: std::sync::Arc<dyn local_core::WebTransport>,
    ) {
        self.session.plugins().set_release_transport(transport);
    }

    /// The install dialog, while it is open.
    pub fn plugin_install(&self) -> Option<&PluginInstall> {
        self.plugin_install.as_ref()
    }

    /// Approve every capability the dialog lists and install the plugin on
    /// this device. Returns whether it installed; when it didn't, the
    /// dialog says why.
    pub fn approve_plugin_install(&mut self) -> bool {
        let Some(install) = self.plugin_install.clone() else {
            return false;
        };
        if install.problem.is_some() || install.downloading {
            return false;
        }
        let installed = self.session.plugins().install(&install);
        match installed {
            Ok(()) => {
                self.plugin_install = None;
                self.plugins_changed();
                true
            }
            Err(error) => {
                if let Some(install) = &mut self.plugin_install {
                    install.problem = Some(error.message);
                }
                self.listeners.emit(Change::Plugins);
                false
            }
        }
    }

    /// Close the install dialog without installing anything. A download
    /// still running is dropped when it ends.
    pub fn cancel_plugin_install(&mut self) {
        self.plugin_download = None;
        if self.plugin_install.take().is_some() {
            self.listeners.emit(Change::Plugins);
        }
    }

    /// Use the plugins in `folder`, one per subfolder, as the plugins that
    /// ship with Noura. A packaged copy of Noura calls this at launch.
    pub fn set_shipped_plugins(&mut self, folder: PathBuf) {
        self.session.plugins().set_shipped(folder);
        self.listeners.emit(Change::Plugins);
    }

    /// The first-run screen: the plugins that ship with Noura that this
    /// device hasn't shown you, with what each may use. Empty once you
    /// approved them. None of their code runs before you do.
    pub fn shipped_plugins(&self) -> Vec<ShippedPlugin> {
        self.session.plugins().shipped_review()
    }

    /// Approve every plugin the first-run screen lists, for this device,
    /// and install each one. The ones the open workspace enables start.
    pub fn approve_shipped_plugins(&mut self) -> bool {
        let approved = self.session.plugins().approve_shipped();
        self.plugins_changed();
        match approved {
            Ok(()) => true,
            Err(error) => {
                self.show_notice("Couldn't install the plugins", error, None);
                false
            }
        }
    }

    /// Turn the plugin `id` on or off in the open workspace. The
    /// workspace's manifest lists it as enabled or not once this returns
    /// `true`; a plugin installed and approved here then runs or stops.
    pub fn set_plugin_enabled(&mut self, id: &str, enabled: bool) -> bool {
        let changed = self.session.plugins().set_enabled(id, enabled);
        match changed {
            Ok(()) => {
                self.plugins_changed();
                true
            }
            Err(error) => {
                let title = if enabled {
                    "Couldn't turn the plugin on"
                } else {
                    "Couldn't turn the plugin off"
                };
                self.settings_notice(title, &error.message, NoticeKind::Problem);
                false
            }
        }
    }

    /// Remove the plugin `id` from this device: its code, its compiled code
    /// and your approvals. Workspace files stay as they are, the manifest's
    /// enabled plugins list included, for your other devices.
    pub fn uninstall_plugin(&mut self, id: &str) -> bool {
        let removed = self.session.plugins().uninstall(id);
        self.plugin_result(removed, "Couldn't uninstall the plugin")
    }

    /// Approve what a new build of the plugin `id` adds, and run that
    /// build.
    pub fn approve_plugin_update(&mut self, id: &str) -> bool {
        let approved = self.session.plugins().approve_update(id);
        self.plugin_result(approved, "Couldn't run the new build")
    }

    /// Keep running the installed build of the plugin `id` and drop the
    /// new build that waits for approval.
    pub fn decline_plugin_update(&mut self, id: &str) -> bool {
        let declined = self.session.plugins().decline_update(id);
        self.plugin_result(declined, "Couldn't keep the installed build")
    }

    /// Take `grant` away from the plugin `id`: a capability, a network host
    /// or a command target. The plugin's next call that needs it fails, and
    /// what it registered under a revoked capability goes.
    pub fn revoke_plugin_grant(&mut self, id: &str, grant: &Grant) -> bool {
        let revoked = self.session.plugins().revoke(id, grant);
        self.plugin_result(revoked, "Couldn't revoke the permission")
    }

    /// Give the plugin `id` back something it declares that you revoked.
    pub fn allow_plugin_grant(&mut self, id: &str, grant: &Grant) -> bool {
        let allowed = self.session.plugins().allow(id, grant);
        self.plugin_result(allowed, "Couldn't allow the permission")
    }

    /// Start the plugin `id` again after it failed too often. The
    /// workspace's enabled plugins list stays as it is.
    pub fn restart_plugin(&mut self, id: &str) -> bool {
        let restarted = self.session.plugins().restart(id);
        self.plugin_result(restarted, "Couldn't start the plugin")
    }

    fn plugin_result(&mut self, result: Result<(), AppError>, title: &str) -> bool {
        self.plugins_changed();
        match result {
            Ok(()) => true,
            Err(error) => {
                self.settings_notice(title, &error.message, NoticeKind::Problem);
                false
            }
        }
    }

    /// Whether running plugins have calls whose answers [`App::poll`]
    /// hasn't shown yet. Plugin calls run off the window's thread, so a
    /// slow or broken plugin never holds up a frame.
    pub fn is_plugin_work_pending(&self) -> bool {
        let plugins = self.session.plugins();
        self.plugin_download.is_some() || plugins.is_working() || plugins.is_checking()
    }

    /// What the plugin page `page_id` shows: the view its plugin drew, or
    /// why it couldn't be drawn. `None` when no running plugin adds the
    /// page.
    pub fn plugin_view(&self, page_id: &str) -> Option<PluginView> {
        self.session.plugins().view(page_id)
    }

    /// Whether the rail page `page_id` is a plugin's.
    pub fn is_plugin_page(&self, page_id: &str) -> bool {
        is_plugin_page(page_id)
    }

    /// This computer's time zone, where the days of a plugin's calendar
    /// grid start and end.
    pub fn time_zone(&self) -> TimeZone {
        self.system.time_zone()
    }

    /// Today's date on this computer's clock, which a calendar grid marks.
    pub fn today(&self) -> Date {
        self.system.now().to_zoned(self.system.time_zone()).date()
    }

    /// The language and region, as a BCP 47 tag such as `fr-CA`, that a
    /// calendar grid names months, weekdays and times in, when this
    /// computer says.
    pub fn locale(&self) -> Option<String> {
        self.system.locale()
    }

    /// Press the button `node_id` in the plugin page `page_id`. The plugin
    /// hears of it on its own thread and draws its views again; the window
    /// shows them, and open notes take what it wrote, once they arrive.
    pub fn press_plugin_button(&mut self, page_id: &str, node_id: &str) {
        self.send_plugin_event(page_id, &Event::Pressed(node_id.to_owned()));
    }

    /// Tell the plugin behind the page `page_id` about `event` in the
    /// page's view, such as a selection or a changed input. The plugin
    /// hears of it on its own thread and draws its views again, and open
    /// notes take what it wrote once its answer arrives. A date or a date
    /// and time of the wrong form never reaches it.
    pub fn send_plugin_event(&mut self, page_id: &str, event: &Event) {
        self.forget_name_draft(&PluginPlace::Page(page_id.to_owned()), event);
        self.session.plugins().send_event(page_id, event, self.key);
        self.plugins_changed();
    }

    /// What the section the plugin page `page_id` adds to the sidebar
    /// shows, while its plugin runs: the view the plugin drew, or why it
    /// couldn't be drawn. `None` when the page adds no section.
    pub fn plugin_sidebar(&self, page_id: &str) -> Option<PluginView> {
        self.session.plugins().sidebar_view(page_id)
    }

    /// Tell the plugin behind the page `page_id` about `event` in the
    /// page's sidebar section, as [`Self::send_plugin_event`] does for the
    /// page's view.
    pub fn send_plugin_sidebar_event(&mut self, page_id: &str, event: &Event) {
        self.forget_name_draft(&PluginPlace::Sidebar(page_id.to_owned()), event);
        self.session
            .plugins()
            .send_sidebar_event(page_id, event, self.key);
        self.plugins_changed();
    }

    /// Tell the plugin that draws the view at `place` about `event` there.
    pub fn send_plugin_event_at(&mut self, place: &PluginPlace, event: &Event) {
        match place {
            PluginPlace::Page(page_id) => self.send_plugin_event(page_id, event),
            PluginPlace::Sidebar(page_id) => self.send_plugin_sidebar_event(page_id, event),
            PluginPlace::Panel(tab) => self.send_panel_event(*tab, event),
            PluginPlace::Inbox { plugin, section } => {
                self.send_inbox_event(plugin, section, event);
            }
            PluginPlace::Settings(section_id) => {
                self.send_plugin_settings_event(section_id, event);
            }
        }
    }

    /// Keep what you typed in the name field `node_id` of the plugin view
    /// at `place`, which started from `started`, so that a name still in
    /// the field when the window closes reaches the plugin first.
    pub fn set_plugin_name_draft(
        &mut self,
        place: PluginPlace,
        node_id: &str,
        started: &str,
        typed: &str,
    ) {
        let draft = NameDraft {
            place,
            node: node_id.to_owned(),
            started: started.to_owned(),
            typed: typed.to_owned(),
            sent: false,
        };
        match self
            .name_drafts
            .iter_mut()
            .find(|held| held.place == draft.place && held.node == draft.node)
        {
            Some(held) => *held = draft,
            None => self.name_drafts.push(draft),
        }
    }

    /// Forget what you typed in the name field `node_id` at `place`, as
    /// once the field sent its name or you cancelled it.
    pub fn clear_plugin_name_draft(&mut self, place: &PluginPlace, node_id: &str) {
        self.name_drafts
            .retain(|draft| !(draft.place == *place && draft.node == node_id));
    }

    /// The name fields whose typing waits to reach their plugins.
    pub fn plugin_name_drafts(&self) -> &[NameDraft] {
        &self.name_drafts
    }

    /// A cancel from a name field ends what it held. A name it sent waits
    /// for the plugin's answer: the field may stay with the name in it, as
    /// after a create that failed.
    fn forget_name_draft(&mut self, place: &PluginPlace, event: &Event) {
        match event {
            Event::Named(naming) => {
                if let Some(draft) = self
                    .name_drafts
                    .iter_mut()
                    .find(|draft| draft.place == *place && draft.node == naming.node)
                {
                    draft.sent = true;
                }
            }
            Event::Cancelled(node) => {
                let node = node.clone();
                self.clear_plugin_name_draft(place, &node);
            }
            _ => {}
        }
    }

    /// Once the plugins answered, a name that reached its plugin is done
    /// when its field went away or starts from another name. When the
    /// field still shows as it did, the name in it waits again to reach
    /// the plugin if the window closes.
    fn settle_name_drafts(&mut self) {
        if !self.name_drafts.iter().any(|draft| draft.sent) {
            return;
        }
        let drafts = std::mem::take(&mut self.name_drafts);
        self.name_drafts = drafts
            .into_iter()
            .filter_map(|mut draft| {
                if !draft.sent {
                    return Some(draft);
                }
                let shows = self
                    .plugin_view_at(&draft.place)
                    .and_then(Result::ok)
                    .is_some_and(|view| shows_name_field(view.root(), &draft.node, &draft.started));
                draft.sent = false;
                shows.then_some(draft)
            })
            .collect();
    }

    /// The view at `place`, while its plugin draws one there.
    fn plugin_view_at(&self, place: &PluginPlace) -> Option<PluginView> {
        match place {
            PluginPlace::Page(page_id) => self.plugin_view(page_id),
            PluginPlace::Sidebar(page_id) => self.plugin_sidebar(page_id),
            PluginPlace::Panel(tab) => self.plugin_panel(*tab),
            PluginPlace::Inbox { plugin, section } => self
                .session
                .plugins()
                .inbox_sections()
                .into_iter()
                .find(|shown| shown.plugin_id == *plugin && shown.section_id == *section)
                .map(|shown| shown.content),
            PluginPlace::Settings(section_id) => self.plugin_settings_view(section_id),
        }
    }

    /// Send each name still typed in a name field to its plugin, as when
    /// the window closes. Returns whether any went, or one sent before
    /// waits for its plugin's answer.
    pub(super) fn commit_name_drafts(&mut self) -> bool {
        let drafts = std::mem::take(&mut self.name_drafts);
        let mut sent = false;
        for draft in drafts {
            if draft.sent {
                sent = true;
                continue;
            }
            let name = draft.typed.trim();
            if name.is_empty() || name == draft.started.trim() {
                continue;
            }
            let event = Event::Named(Naming {
                node: draft.node.clone(),
                name: name.to_owned(),
            });
            self.send_plugin_event_at(&draft.place, &event);
            sent = true;
        }
        sent
    }

    /// The rows in `range` of the virtual list `node_id` in the plugin
    /// page `page_id`. The plugin fetches rows on its own thread as they're
    /// first asked for, and a row is `None` until it arrives. The error
    /// says why the plugin's rows can't be shown.
    pub fn plugin_rows(
        &mut self,
        page_id: &str,
        node_id: &str,
        range: Range<u32>,
    ) -> Result<Vec<Option<VirtualRow>>, String> {
        self.session.plugins().virtual_rows(page_id, node_id, range)
    }

    /// Whether the Settings section `section_id` is a plugin's page.
    pub fn is_plugin_settings(&self, section_id: &str) -> bool {
        section_id.starts_with(SETTINGS_PREFIX)
    }

    /// What a plugin's page in Settings shows. Noura keeps no setting
    /// values for the plugin; the plugin keeps them in workspace files.
    pub fn plugin_settings_view(&self, section_id: &str) -> Option<PluginView> {
        self.session.plugins().settings_view(section_id)
    }

    /// Press the button `node_id` on a plugin's page in Settings.
    pub fn press_plugin_settings_button(&mut self, section_id: &str, node_id: &str) {
        self.send_plugin_settings_event(section_id, &Event::Pressed(node_id.to_owned()));
    }

    /// Tell the plugin behind its page `section_id` in Settings about
    /// `event` there.
    pub fn send_plugin_settings_event(&mut self, section_id: &str, event: &Event) {
        self.forget_name_draft(&PluginPlace::Settings(section_id.to_owned()), event);
        self.session
            .plugins()
            .send_settings(section_id, event, self.key);
        self.plugins_changed();
    }

    /// Press the button `node_id` in the Inbox section `section_id` of the
    /// plugin `plugin_id`.
    pub fn press_inbox_button(&mut self, plugin_id: &str, section_id: &str, node_id: &str) {
        let event = Event::Pressed(node_id.to_owned());
        self.send_inbox_event(plugin_id, section_id, &event);
    }

    /// Tell the plugin `plugin_id` about `event` in its Inbox section
    /// `section_id`.
    pub fn send_inbox_event(&mut self, plugin_id: &str, section_id: &str, event: &Event) {
        let place = PluginPlace::Inbox {
            plugin: plugin_id.to_owned(),
            section: section_id.to_owned(),
        };
        self.forget_name_draft(&place, event);
        self.session
            .plugins()
            .send_inbox(plugin_id, section_id, event, self.key);
        self.plugins_changed();
    }

    /// The tools running plugins offer to AI assistants, while each plugin
    /// is on and holds the `tools` capability.
    pub fn plugin_tools(&self) -> Vec<PluginTool> {
        self.session.plugins().tools()
    }

    /// Call the tool `name` of the plugin `plugin_id` with JSON
    /// `arguments`, and wait for its answer. Arguments that don't match
    /// the tool's schema never reach the plugin. Call this off the window's
    /// thread: the plugin may take up to its call deadline.
    pub fn call_plugin_tool(
        &self,
        plugin_id: &str,
        name: &str,
        arguments: &str,
    ) -> Result<String, AppError> {
        self.session.call_plugin_tool(plugin_id, name, arguments)
    }

    /// The properties panel the plugin that holds the type of the object
    /// open in `tab` draws beside its body, or why it can't be drawn.
    /// `None` when the tab shows no object of a running plugin's type, so
    /// the kernel's panel shows. The plugin draws on its own thread, so the
    /// panel it drew last shows until its new one arrives.
    pub fn plugin_panel(&self, tab: TabId) -> Option<PluginView> {
        let session = self.notes.sessions.get(&tab)?;
        let object_type = session.object_type()?;
        let object_id = session.object_id()?;
        self.session
            .plugins()
            .panel(object_type, object_id, session.revision())
    }

    /// Press the button `node_id` in the plugin's properties panel of the
    /// object open in `tab`. The plugin hears of it on its own thread and
    /// draws its panel again, and the note takes what the plugin wrote once
    /// the answer arrives: at once when it has nothing unsaved, and merged
    /// with your typing at its next save when it has.
    pub fn press_panel_button(&mut self, tab: TabId, node_id: &str) {
        self.send_panel_event(tab, &Event::Pressed(node_id.to_owned()));
    }

    /// Tell the plugin that draws the properties panel of the object open
    /// in `tab` about `event` in the panel, such as a press, a selection
    /// or a changed input. Its panel draws again and the note takes what
    /// it wrote, as after [`Self::press_panel_button`]. A date or a date
    /// and time of the wrong form never reaches it.
    pub fn send_panel_event(&mut self, tab: TabId, event: &Event) {
        self.forget_name_draft(&PluginPlace::Panel(tab), event);
        let Some(session) = self.notes.sessions.get(&tab) else {
            return;
        };
        let (Some(object_type), Some(object_id)) = (session.object_type(), session.object_id())
        else {
            return;
        };
        let (object_type, object_id) = (object_type.to_owned(), object_id.to_owned());
        self.session
            .plugins()
            .send_panel(&object_type, &object_id, event, self.key);
        self.plugins_changed();
    }

    /// Show what changed in the plugins service now, and wake the other
    /// windows to show it too.
    fn plugins_changed(&mut self) {
        self.poll_plugins();
        self.session.wake_all();
    }

    /// Take new plugin builds and the running plugins' answers, and show
    /// what the plugins add when it changed. Returns whether anything did.
    pub(super) fn poll_plugins(&mut self) -> bool {
        let downloaded = self.poll_plugin_download();
        let (version, pages, settings, acts, openings, working, types) = {
            let mut plugins = self.session.plugins();
            plugins.poll();
            (
                plugins.version(),
                plugins.pages(),
                plugins.settings_pages(),
                plugins.acts(),
                plugins.take_openings(self.key),
                plugins.is_working(),
                plugins.types_generation(),
            )
        };
        if types != self.types_seen {
            // A plugin registered or released a type: the files of that
            // type are checked against its schema again, or no longer.
            self.types_seen = types;
            self.refresh_tree(None);
        }
        if !working {
            self.settle_name_drafts();
            self.names_sent_while_leaving();
        }
        let acted = acts != self.plugin_acts_seen;
        if acted {
            // Open notes take what the plugins wrote.
            self.plugin_acts_seen = acts;
            self.check_notes_on_disk();
        }
        let opened = !openings.is_empty();
        if version == self.plugins_seen {
            self.open_for_plugins(openings);
            return downloaded || acted || opened;
        }
        self.plugins_seen = version;
        self.show_plugin_pages(&pages);
        self.open_for_plugins(openings);
        self.show_plugin_settings(&settings);
        self.listeners.emit(Change::Plugins);
        self.listeners.emit(Change::Commands);
        self.listeners.emit(Change::Inbox);
        true
    }

    /// Do what running plugins asked of this window, oldest first: show
    /// objects, and pages once they are in the rail, and move objects to
    /// the trash. An object Noura can't find says so; a page the plugin
    /// doesn't add is passed over.
    fn open_for_plugins(&mut self, openings: Vec<Opening>) {
        if self.closed || self.is_leaving_window() {
            return;
        }
        for opening in openings {
            match opening {
                Opening::Object { id, how } => self.open_object(&id, how),
                Opening::Page(page_id) => {
                    if self.rail.page(&page_id).is_some() {
                        self.open_page(&page_id, OpenAs::Preview);
                    }
                }
                Opening::Trash(id) => self.trash_object(&id),
            }
        }
    }

    /// Move the object with the stable ID `id` to the trash, wherever its
    /// file is now, as Move to Trash in the file tree does, but leaving the
    /// tree's current row where it is.
    fn trash_object(&mut self, id: &str) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        match workspace.engine().read_object(id) {
            Ok(object) => self.trash_item(&object.relative_path, false),
            Err(error) => {
                tracing::warn!(code = %error.code, "a plugin asked to trash an object Noura can't read");
                self.show_notice(
                    "Couldn't move it to the trash",
                    AppError::new(
                        "object_not_found",
                        "Noura couldn't find it in the workspace.",
                    ),
                    None,
                );
            }
        }
    }

    /// Open the object with the stable ID `id` as `how` says, wherever its
    /// file is now.
    fn open_object(&mut self, id: &str, how: OpenAs) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        match workspace.engine().read_object(id) {
            Ok(object) => self.open_file(&object.relative_path, how),
            Err(error) => {
                tracing::warn!(code = %error.code, "a plugin asked to open an object Noura can't read");
                self.show_notice(
                    "Couldn't open it",
                    AppError::new(
                        "object_not_found",
                        "Noura couldn't find it in the workspace.",
                    ),
                    None,
                );
            }
        }
    }

    /// Make the rail's plugin pages `pages`, as (page ID, title).
    pub(super) fn show_plugin_pages(&mut self, pages: &[(String, String)]) {
        let gone: Vec<String> = self
            .rail
            .ordered()
            .iter()
            .filter(|page| is_plugin_page(&page.id))
            .filter(|page| !pages.iter().any(|(id, _)| *id == page.id))
            .map(|page| page.id.clone())
            .collect();
        for id in gone {
            self.unregister_page(&id);
        }
        for (id, title) in pages {
            let shown = self.rail.page(id).map(|page| page.title.as_str());
            if shown != Some(title.as_str()) {
                self.register_page(id, title);
            }
        }
    }

    /// Make Settings' plugin pages `pages`, as (section ID, title), in the
    /// Workspace group after the kernel's sections. Settings open on a
    /// page that went shows Plugins.
    fn show_plugin_settings(&mut self, pages: &[(String, String)]) {
        let before = self.settings_sections.len();
        self.settings_sections.retain(|section| {
            !section.id.starts_with(SETTINGS_PREFIX)
                || pages.iter().any(|(id, _)| *id == section.id)
        });
        let mut changed = before != self.settings_sections.len();
        for (id, title) in pages {
            let shown = self
                .settings_sections
                .iter()
                .find(|section| section.id == *id)
                .map(|section| section.title.as_str());
            if shown != Some(title.as_str()) {
                self.register_settings_section(id, title, SettingsGroup::Workspace);
                changed = true;
            }
        }
        let showing_gone = self.settings.as_ref().is_some_and(|settings| {
            settings.section.starts_with(SETTINGS_PREFIX)
                && !pages.iter().any(|(id, _)| *id == settings.section)
        });
        if showing_gone {
            self.select_settings_section(settings_section::PLUGINS);
        }
        if changed {
            self.listeners.emit(Change::Settings);
        }
    }

    /// Take the plugin pages out of the rail, without touching the tabs,
    /// before a workspace's saved tabs come back: a tab on a plugin page
    /// waits until its plugin adds the page again.
    pub(super) fn forget_plugin_pages(&mut self) {
        let pages: Vec<String> = self
            .rail
            .ordered()
            .iter()
            .filter(|page| is_plugin_page(&page.id))
            .map(|page| page.id.clone())
            .collect();
        for id in pages {
            self.rail.unregister(&id);
        }
    }
}

/// Whether `node` or a node under it is the name field `id`, starting from
/// `started`.
fn shows_name_field(node: &ViewNode, id: &str, started: &str) -> bool {
    if node.id == id {
        return matches!(&node.kind, ViewKind::NameField { value, .. } if value == started);
    }
    node.children
        .iter()
        .any(|child| shows_name_field(child, id, started))
}
