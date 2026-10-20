//! Tabs and the rail: what the window shows and how you move between it.

use super::{App, Change};
use crate::{
    OpenAs, Tab, TabId, TabPlace,
    files::{remap, shown_name},
    rail::FILES_PAGE,
    tabs::TabContent,
};

impl App {
    /// Show a file in a tab: its own tab when it has one, otherwise as
    /// `how` says.
    pub fn open_file(&mut self, relative_path: &str, how: OpenAs) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        match workspace.open_file(relative_path) {
            Ok(file) => {
                let object_id = self
                    .tree
                    .as_ref()
                    .and_then(|tree| tree.object_id(relative_path))
                    .map(str::to_owned);
                let content = TabContent::File { file, object_id };
                self.tabs.open(content, how);
                self.tabs_changed(true);
            }
            Err(error) => {
                let title = format!(
                    "Couldn't open \u{201c}{}\u{201d}",
                    shown_name(
                        relative_path.rsplit('/').next().unwrap_or(relative_path),
                        crate::EntryKind::File
                    )
                );
                self.show_notice(&title, error, None);
            }
        }
    }

    /// Add a page to the rail. Kernel pages such as the Inbox and plugin
    /// pages register here; registering a page again renames it.
    pub fn register_page(&mut self, id: &str, title: &str) {
        self.rail.register(id, title);
        self.listeners.emit(Change::Rail);
    }

    /// Show a rail page. Files shows a document tab; any other page shows
    /// in its own tab, or opens as `how` says.
    pub fn open_page(&mut self, page_id: &str, how: OpenAs) {
        if page_id == FILES_PAGE {
            if self.tabs.show_files() {
                self.tabs_changed(true);
            }
            return;
        }
        let Some(page) = self.rail.page(page_id) else {
            return;
        };
        let content = TabContent::Page {
            page_id: page.id.clone(),
            title: page.title.clone(),
        };
        self.tabs.open(content, how);
        self.tabs_changed(true);
    }

    /// The rail page the window shows: the active tab's page, or Files.
    pub fn current_page(&self) -> &str {
        self.tabs
            .active()
            .and_then(Tab::page_id)
            .unwrap_or(FILES_PAGE)
    }

    /// Show the tab `id`.
    pub fn activate_tab(&mut self, id: TabId) {
        if self.tabs.activate(id) {
            self.tabs_changed(true);
        }
    }

    /// Where you were in a tab: the default for a tab that isn't open.
    pub fn tab_place(&self, id: TabId) -> TabPlace {
        self.tabs
            .get(id)
            .map(|tab| tab.place().clone())
            .unwrap_or_default()
    }

    /// Remember where you are in a tab, as its view reports it: the
    /// scroll position, and a position such as the editor's selection.
    pub fn set_tab_place(&mut self, id: TabId, place: TabPlace) {
        self.tabs.set_place(id, place);
    }

    /// Keep a preview tab, so the next preview opens beside it. A
    /// double-click on the tab does this, and so does the first edit in
    /// the tab's view.
    pub fn keep_tab(&mut self, id: TabId) {
        if self.tabs.keep(id) {
            self.tabs_changed(false);
        }
    }

    /// Close a tab. Closing the active tab shows the tab that takes its
    /// place; closing the last one leaves nothing open.
    pub fn close_tab(&mut self, id: TabId) {
        let was_active = self.tabs.active().map(Tab::id) == Some(id);
        if self.tabs.close(id) {
            self.tabs_changed(was_active);
        }
    }

    /// Read the workspace folder again and bring the tree and the tabs in
    /// line with it. A tab whose managed note moved follows its stable
    /// ID; a tab whose file is gone, or moved without an ID to follow,
    /// closes. The watcher (ticket 10) runs this after outside changes.
    pub fn rescan(&mut self) {
        self.reread_tree(None);
    }

    /// The item at `from`, a file or a folder, moved to `to` inside the
    /// workspace: its tabs and open folders follow it. The tree's rename
    /// and move call this once the move is on disk.
    pub fn item_moved(&mut self, from: &str, to: &str) {
        let (changed, active_changed) = self.tabs.follow_files(|path, id| {
            let path = remap(path, from, to).unwrap_or_else(|| path.to_owned());
            Some((path, id.map(str::to_owned)))
        });
        if changed {
            self.tabs_changed(active_changed);
        }
        self.reread_tree(Some((from, to)));
    }

    /// Read the tree again, keeping open folders and the focused row.
    /// `moved` names an item that moved, so they follow it.
    fn reread_tree(&mut self, moved: Option<(&str, &str)>) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        let mut tree = match workspace.file_tree() {
            Ok(tree) => tree,
            Err(error) => {
                self.show_notice("Couldn't read the files", error, None);
                return;
            }
        };
        let follow = |path: &str| match moved {
            Some((from, to)) => remap(path, from, to).unwrap_or_else(|| path.to_owned()),
            None => path.to_owned(),
        };
        if let Some(old) = &self.tree {
            let expanded: Vec<String> = old
                .expanded_folders()
                .iter()
                .map(|path| follow(path))
                .collect();
            tree.expand_folders(expanded.iter().map(String::as_str));
            if let Some(focused) = old.focused_path().map(follow)
                && let Some(row) = tree.find_row(&focused)
            {
                tree.set_focus(row);
            }
        }
        self.tree = Some(tree);
        self.listeners.emit(Change::Tree);
        self.follow_files();
    }

    /// Point each file tab at its file in the tree: the same path, or the
    /// one file with the tab's stable ID. Tabs whose file is gone close.
    fn follow_files(&mut self) {
        let Some(tree) = &self.tree else {
            return;
        };
        let (changed, active_changed) = self.tabs.follow_files(|path, id| {
            if tree.is_file(path) {
                return Some((path.to_owned(), tree.object_id(path).map(str::to_owned)));
            }
            let moved = tree.only_file_with_object(id?)?;
            Some((moved.to_owned(), id.map(str::to_owned)))
        });
        if changed {
            self.tabs_changed(active_changed);
        }
    }

    /// Announce a change to the tabs and save them. `document` says
    /// whether what the window shows changed too.
    fn tabs_changed(&mut self, document: bool) {
        self.listeners.emit(Change::Tabs);
        if document {
            self.listeners.emit(Change::Document);
        }
        self.save_view();
    }

    /// Save the open workspace's view state to this device's app data.
    pub(super) fn save_view(&self) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        let (tabs, active) = self.tabs.stored();
        self.views.save(
            workspace.id(),
            &crate::view_state::StoredView { tabs, active },
        );
    }

    /// Bring back the open workspace's saved view state: the tabs whose
    /// file or page is still there, and the active tab.
    pub(super) fn restore_view(&mut self) {
        let Some(workspace) = &self.workspace else {
            self.tabs.restore(Vec::new(), None);
            return;
        };
        let stored = self.views.load(workspace.id());
        let restored = stored
            .tabs
            .into_iter()
            .map(|tab| match (tab.file, tab.page) {
                (Some(path), None) => {
                    let tree = self.tree.as_ref()?;
                    // A managed note moved while Noura was closed comes
                    // back at its new path.
                    let path = if tree.is_file(&path) {
                        path
                    } else {
                        tree.only_file_with_object(tab.object_id.as_deref()?)?
                            .to_owned()
                    };
                    let file = workspace.open_file(&path).ok()?;
                    let object_id = tree.object_id(&path).map(str::to_owned);
                    Some((TabContent::File { file, object_id }, tab.preview))
                }
                (None, Some(page_id)) if page_id != FILES_PAGE => {
                    self.rail.page(&page_id).map(|page| {
                        let content = TabContent::Page {
                            page_id: page.id.clone(),
                            title: page.title.clone(),
                        };
                        (content, tab.preview)
                    })
                }
                _ => None,
            })
            .collect();
        self.tabs.restore(restored, stored.active);
    }
}
