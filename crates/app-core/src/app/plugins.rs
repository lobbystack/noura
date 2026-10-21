//! The Plugins page and plugin pages: installing a plugin from a folder,
//! turning it on or off in the open workspace, uninstalling it, approving
//! a new build, drawing and pressing a plugin page's view, and the
//! properties panel a plugin draws beside an object of its own type. Every
//! window shares the session's plugins service and shows the pages of the
//! plugins it runs.

use std::path::PathBuf;

use super::{App, Change};
use crate::{NoticeKind, PluginInstall, PluginRow, PluginView, TabId, plugins::is_plugin_page};

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
        self.listeners.emit(Change::Plugins);
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
        if install.problem.is_some() {
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

    /// Close the install dialog without installing anything.
    pub fn cancel_plugin_install(&mut self) {
        if self.plugin_install.take().is_some() {
            self.listeners.emit(Change::Plugins);
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
        match removed {
            Ok(()) => {
                self.plugins_changed();
                true
            }
            Err(error) => {
                self.plugins_changed();
                self.settings_notice(
                    "Couldn't uninstall the plugin",
                    &error.message,
                    NoticeKind::Problem,
                );
                false
            }
        }
    }

    /// Approve the capabilities a new build of the plugin `id` adds, and
    /// run that build.
    pub fn approve_plugin_update(&mut self, id: &str) -> bool {
        let approved = self.session.plugins().approve_update(id);
        match approved {
            Ok(()) => {
                self.plugins_changed();
                true
            }
            Err(error) => {
                self.plugins_changed();
                self.settings_notice(
                    "Couldn't run the new build",
                    &error.message,
                    NoticeKind::Problem,
                );
                false
            }
        }
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

    /// Press the button `node_id` in the plugin page `page_id`. The plugin
    /// hears of it and draws its pages again, and open notes take what it
    /// wrote.
    pub fn press_plugin_button(&mut self, page_id: &str, node_id: &str) {
        self.session.plugins().press(page_id, node_id);
        self.check_notes_on_disk();
        self.plugins_changed();
    }

    /// The properties panel the plugin that holds the type of the object
    /// open in `tab` draws beside its body, or why it can't be drawn.
    /// `None` when the tab shows no object of a running plugin's type, so
    /// the kernel's panel shows.
    pub fn plugin_panel(&self, tab: TabId) -> Option<PluginView> {
        let session = self.notes.sessions.get(&tab)?;
        let object_type = session.object_type()?;
        let object_id = session.object_id()?;
        self.session
            .plugins()
            .panel(object_type, object_id, session.revision())
    }

    /// Press the button `node_id` in the plugin's properties panel of the
    /// object open in `tab`. The plugin hears of it, its panel draws
    /// again, and the note takes what the plugin wrote: at once when it
    /// has nothing unsaved, and merged with your typing at its next save
    /// when it has.
    pub fn press_panel_button(&mut self, tab: TabId, node_id: &str) {
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
            .press_panel(&object_type, &object_id, node_id);
        self.check_notes_on_disk();
        self.plugins_changed();
        self.listeners.emit(Change::Note);
    }

    /// Show what changed in the plugins service now, and wake the other
    /// windows to show it too.
    fn plugins_changed(&mut self) {
        self.poll_plugins();
        self.session.wake_all();
    }

    /// Take new plugin builds, and show the pages the running plugins add
    /// and their views when they changed. Returns whether anything did.
    pub(super) fn poll_plugins(&mut self) -> bool {
        let (version, pages) = {
            let mut plugins = self.session.plugins();
            plugins.poll();
            (plugins.version(), plugins.pages())
        };
        if version == self.plugins_seen {
            return false;
        }
        self.plugins_seen = version;
        self.show_plugin_pages(&pages);
        self.listeners.emit(Change::Plugins);
        true
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
