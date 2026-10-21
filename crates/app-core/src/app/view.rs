//! Tabs, the rail, the sidebar's layout and the file tree's open folders:
//! what the window shows and how you move between it. A refresh of the
//! tree after Noura's own change runs through `App::replace_tree`, and one
//! from the watch thread through `App::poll`. Both make the tabs follow
//! their files.

use std::time::Instant;

use super::{App, Change};
use crate::{
    AppError, DropSpot, FileTree, Layout, OpenAs, Page, Tab, TabId, TabPlace, TreeAction,
    error::Access,
    files::{remap, shown_name},
    layout::sidebar_width,
    rail::FILES_PAGE,
    tabs::{Restored, TabContent},
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
    /// pages register here; registering a page again renames it. A saved
    /// tab that was waiting for the page opens in its place, and shows if
    /// it was the active tab and you haven't picked another since.
    pub fn register_page(&mut self, id: &str, title: &str) {
        self.rail.register(id, title);
        self.listeners.emit(Change::Rail);
        let Some(page) = self.rail.page(id) else {
            return;
        };
        let content = TabContent::Page {
            page_id: page.id.clone(),
            title: page.title.clone(),
        };
        if let Some(active_changed) = self.tabs.page_registered(content) {
            self.tabs_changed(active_changed);
        }
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

    /// The rail's pages, top to bottom.
    pub fn rail(&self) -> Vec<&Page> {
        self.rail.ordered()
    }

    /// Put the page `page_id` at `index` in the rail, as dragging it does.
    /// The open workspace keeps the order on this device.
    pub fn move_page(&mut self, page_id: &str, index: usize) {
        if self.rail.move_page(page_id, index) {
            self.listeners.emit(Change::Rail);
            self.save_view();
        }
    }

    /// Move the page `page_id` up (`-1`) or down (`1`) the rail, as
    /// Alt-Up and Alt-Down do.
    pub fn move_page_by(&mut self, page_id: &str, offset: isize) {
        let Some(index) = self
            .rail
            .ordered()
            .iter()
            .position(|page| page.id == page_id)
        else {
            return;
        };
        if let Some(index) = index.checked_add_signed(offset) {
            self.move_page(page_id, index);
        }
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

    /// The layout this device remembers.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Show or hide the sidebar.
    pub fn toggle_sidebar(&mut self) {
        self.layout.sidebar_shown = !self.layout.sidebar_shown;
        self.layout_changed();
    }

    /// Resize the sidebar, within [`SIDEBAR_MIN_WIDTH`](crate::SIDEBAR_MIN_WIDTH)
    /// and [`SIDEBAR_MAX_WIDTH`](crate::SIDEBAR_MAX_WIDTH).
    pub fn set_sidebar_width(&mut self, width: f32) {
        if let Some(width) = sidebar_width(width)
            && width != self.layout.sidebar_width
        {
            self.layout.sidebar_width = width;
            self.layout_changed();
        }
    }

    /// Whether you folded the sidebar section `key`.
    pub fn is_section_collapsed(&self, key: &str) -> bool {
        self.layout
            .collapsed_sections
            .iter()
            .any(|section| section == key)
    }

    /// Fold or unfold the sidebar section `key`.
    pub fn toggle_section(&mut self, key: &str) {
        let sections = &mut self.layout.collapsed_sections;
        match sections.iter().position(|section| section == key) {
            Some(index) => {
                sections.remove(index);
            }
            None => sections.push(key.to_owned()),
        }
        self.layout_changed();
    }

    fn layout_changed(&mut self) {
        self.listeners.emit(Change::Layout);
        self.layouts.save(&self.layout);
    }

    /// A click on the tree's `row`: open a file as `how` says, or open or
    /// close a folder.
    pub fn activate_tree_row(&mut self, row: usize, how: OpenAs) {
        if let Some(TreeAction::OpenFile { relative_path }) =
            self.change_tree(|tree| tree.activate(row))
        {
            self.open_file(&relative_path, how);
        }
    }

    /// Start dragging the tree's row at `row`.
    pub fn start_tree_drag(&mut self, row: usize) {
        if let Some(tree) = &mut self.tree {
            tree.start_drag(row);
        }
    }

    /// The pointer is at `spot` at time `at` while a tree row is dragged.
    /// See [`FileTree::drag_over`]: a closed folder the pointer rests on
    /// opens, and the workspace remembers it open.
    pub fn drag_tree_over(&mut self, spot: DropSpot, at: Instant) {
        self.change_tree(|tree| {
            tree.drag_over(spot, at);
            Some(())
        });
    }

    /// Drop the dragged tree row, and move it when the drop landed
    /// somewhere it can go. Folders the drag opened close again unless
    /// they hold the drop.
    pub fn finish_tree_drag(&mut self) {
        if let Some(action) = self.change_tree(FileTree::finish_drag) {
            self.apply(action);
        }
    }

    /// Stop dragging the tree row without moving it, and close the folders
    /// the drag opened.
    pub fn cancel_tree_drag(&mut self) {
        self.change_tree(|tree| {
            tree.cancel_drag();
            Some(())
        });
    }

    /// Change the file tree, and save its open folders when they changed.
    pub(super) fn change_tree<R>(
        &mut self,
        change: impl FnOnce(&mut FileTree) -> Option<R>,
    ) -> Option<R> {
        let tree = self.tree.as_mut()?;
        let before = tree.expanded_folders();
        let result = change(tree);
        if tree.expanded_folders() != before {
            self.listeners.emit(Change::Tree);
            self.save_view();
        }
        result
    }

    /// Read the workspace folder again, every file included, and bring the
    /// tree and the tabs in line with it. A managed note's tab follows its
    /// stable ID, even when another file now sits at its old path; a tab
    /// whose file is gone, or moved without an ID to follow, closes. The
    /// watch thread does the same on its own after outside changes, and
    /// [`App::poll`] applies it; this does it now.
    pub fn rescan(&mut self) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        // Every file, not only the ones the watcher reported: the plain
        // listing trusts the index for paths it already knows, so a file
        // that another app replaced would keep the old file's stable ID.
        if let Err(error) = workspace.engine().reconcile() {
            let error = AppError::from_core(error, workspace.root(), Access::ReadWorkspace);
            self.show_notice("Couldn't read the files", error, None);
            return;
        }
        self.refresh_tree(None);
    }

    /// Show a tree read again: it keeps the old tree's open folders,
    /// current row and drag, which follow the item that moved from
    /// `moved.0` to `moved.1`, and the tabs follow their files into it. Every refresh
    /// of the tree comes through here, so the open folders it carries over
    /// are saved.
    pub(super) fn replace_tree(
        &mut self,
        tree: Result<FileTree, AppError>,
        moved: Option<(&str, &str)>,
    ) {
        match tree {
            Ok(mut tree) => {
                if let Some(old) = &self.tree {
                    tree.carry_over(old, moved);
                }
                self.tree = Some(tree);
                self.listeners.emit(Change::Tree);
                self.follow_files();
                self.save_view();
            }
            Err(error) => self.show_notice("Couldn't read the files", error, None),
        }
    }

    /// Close the tabs of the item at `relative_path`, a file or a folder,
    /// and of everything inside it, as when it goes to the trash.
    pub(super) fn close_tabs_inside(&mut self, relative_path: &str) {
        let (changed, active_changed) = self.tabs.follow_files(|path, id| {
            remap(path, relative_path, relative_path)
                .is_none()
                .then(|| (path.to_owned(), id.map(str::to_owned)))
        });
        if changed {
            self.tabs_changed(active_changed);
        }
    }

    /// The item at `from`, a file or a folder, moved to `to`: point its
    /// tabs, and the tabs of everything inside it, at the new path.
    pub(super) fn move_tabs(&mut self, from: &str, to: &str) {
        let (changed, active_changed) = self.tabs.follow_files(|path, id| {
            let path = remap(path, from, to).unwrap_or_else(|| path.to_owned());
            Some((path, id.map(str::to_owned)))
        });
        if changed {
            self.tabs_changed(active_changed);
        }
    }

    /// Point each file tab at its file in the tree: a managed object's
    /// file by its stable ID, any other file by its path. Tabs whose file
    /// is gone close.
    pub(super) fn follow_files(&mut self) {
        let Some(tree) = &self.tree else {
            return;
        };
        let (changed, active_changed) = self.tabs.follow_files(|path, id| {
            let path = tree.follow(path, id)?;
            let id = tree.object_id(&path).map(str::to_owned);
            Some((path, id))
        });
        if changed {
            self.tabs_changed(active_changed);
        }
    }

    /// Announce a change to the tabs and save them. `document` says
    /// whether what the window shows changed too.
    pub(super) fn tabs_changed(&mut self, document: bool) {
        self.listeners.emit(Change::Tabs);
        if document {
            self.listeners.emit(Change::Document);
        }
        self.save_view();
    }

    /// Save the open workspace's view state to this device's app data,
    /// in the background.
    pub(super) fn save_view(&mut self) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        let (tabs, active) = self.tabs.stored();
        let expanded_folders = self
            .tree
            .as_ref()
            .map(FileTree::expanded_folders)
            .unwrap_or_default();
        let view = crate::view_state::StoredView {
            tabs,
            active,
            expanded_folders,
            rail_order: self.rail.saved_order().to_vec(),
        };
        self.views.save(workspace.id(), view);
    }

    /// Bring back the open workspace's saved view state: the tabs whose
    /// file or page is still there, and the active tab. Tabs for pages
    /// that haven't registered yet wait for them.
    pub(super) fn restore_view(&mut self) {
        let Some(workspace) = &self.workspace else {
            self.tabs.restore(Vec::new(), None);
            self.rail.set_saved_order(Vec::new());
            return;
        };
        let stored = self.views.load(workspace.id());
        self.rail.set_saved_order(stored.rail_order);
        self.listeners.emit(Change::Rail);
        if let Some(tree) = &mut self.tree {
            tree.expand_folders(stored.expanded_folders.iter().map(String::as_str));
        }
        let restored = stored
            .tabs
            .into_iter()
            .map(|tab| match (tab.file, tab.page) {
                (Some(path), None) => {
                    let file = self.tree.as_ref().and_then(|tree| {
                        // A managed note moved while Noura was closed
                        // comes back at its new path.
                        let path = tree.follow(&path, tab.object_id.as_deref())?;
                        let file = workspace.open_file(&path).ok()?;
                        let object_id = tree.object_id(&path).map(str::to_owned);
                        Some(TabContent::File { file, object_id })
                    });
                    file.map_or(Restored::Gone, |file| Restored::Open(file, tab.preview))
                }
                (None, Some(page_id)) if page_id != FILES_PAGE && !page_id.is_empty() => {
                    match self.rail.page(&page_id) {
                        Some(page) => {
                            let content = TabContent::Page {
                                page_id: page.id.clone(),
                                title: page.title.clone(),
                            };
                            Restored::Open(content, tab.preview)
                        }
                        // A page that registers later, such as a plugin's,
                        // gets its tab back then.
                        None => Restored::Waiting {
                            page_id,
                            preview: tab.preview,
                        },
                    }
                }
                _ => Restored::Gone,
            })
            .collect();
        self.tabs.restore(restored, stored.active);
    }
}
