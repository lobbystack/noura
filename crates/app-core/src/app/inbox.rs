//! The Inbox page's view state, which follows the files through the
//! change stream, and the slot where enabled plugins add their sections.

use std::sync::Arc;

use super::{App, Change};
use crate::{
    FileLink, Inbox, InboxSectionProvider, LinkAction, OpenAs,
    inbox::{Attention, RecentNote, greeting},
};

impl App {
    /// What the Inbox shows now.
    pub fn inbox(&self) -> Inbox {
        let platform = self.platform();
        Inbox {
            greeting: greeting(self.system.local_hour()),
            needs_attention: self
                .problems
                .iter()
                .map(|problem| Attention::of(problem, platform))
                .collect(),
            sections: self.inbox_sections.ordered(&self.rail.ordered()),
            recent: RecentNote::list(&self.recent),
        }
    }

    /// Follow a link from the Inbox: open its file as `how` says, the way
    /// the file tree opens one, or show it in the file manager.
    pub fn follow_link(&mut self, link: &FileLink, how: OpenAs) {
        match link.action {
            LinkAction::Open => self.open_file(&link.relative_path, how),
            LinkAction::Reveal => self.reveal(&link.relative_path),
        }
    }

    /// Add the Inbox section of the plugin `plugin_id`, headed `title`,
    /// while the plugin is on, or replace the one it has. `provider` fills
    /// it now and again whenever the files change. The section takes the
    /// place of the plugin's rail page `page_id` among the sections, and a
    /// plugin without a page puts its section after the others. A section
    /// that fails shows why in its own place.
    pub fn add_inbox_section(
        &mut self,
        plugin_id: &str,
        page_id: Option<&str>,
        title: &str,
        provider: Arc<dyn InboxSectionProvider>,
    ) {
        self.inbox_sections.add(plugin_id, page_id, title, provider);
        self.listeners.emit(Change::Inbox);
    }

    /// Take the plugin `plugin_id`'s section out of the Inbox, as when you
    /// turn the plugin off.
    pub fn remove_inbox_section(&mut self, plugin_id: &str) {
        if self.inbox_sections.remove(plugin_id) {
            self.listeners.emit(Change::Inbox);
        }
    }

    /// Fill the plugin sections again after the files changed.
    pub(super) fn reload_inbox_sections(&mut self) {
        if self.inbox_sections.reload() {
            self.listeners.emit(Change::Inbox);
        }
    }

    /// Show the notes changed last in a new listing of the files.
    pub(super) fn set_recent(&mut self, recent: Vec<String>) {
        if self.recent != recent {
            self.recent = recent;
            self.listeners.emit(Change::Inbox);
        }
    }
}
