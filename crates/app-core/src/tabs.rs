//! Tabs: the files and pages open in a window, which one shows, and
//! which tab is the preview that the next preview replaces.

use crate::{OpenedFile, view_state::StoredTab};

/// How to open a file or a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAs {
    /// A single click: switch to the item's tab, or show the item in the
    /// preview tab, which the next preview replaces.
    Preview,
    /// A double-click, Open in new tab, a new note or a duplicate: switch
    /// to the item's tab and keep it, or open a kept tab after the others
    /// and leave the current tab in place.
    Kept,
}

/// Where you were in a tab: kept while you look at another tab, for the
/// rest of the session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TabPlace {
    /// How far the tab's view was scrolled, in points: across, then down.
    pub scroll: [f32; 2],
    /// The position the tab's view reports, if it reports one.
    pub view: Option<ViewPlace>,
}

/// A position a view reports for its tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewPlace {
    /// A text view's selection in characters: where it started and where
    /// the caret is. They're equal when nothing is selected.
    Selection { anchor: usize, caret: usize },
}

/// A tab's identity for this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(u64);

/// What a tab shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabContent {
    /// A file in the workspace. `object_id` is the stable ID of a managed
    /// object, which the tab follows when the file moves.
    File {
        file: OpenedFile,
        object_id: Option<String>,
    },
    /// A rail page such as the Inbox.
    Page { page_id: String, title: String },
}

/// One open tab.
#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    id: TabId,
    content: TabContent,
    preview: bool,
    place: TabPlace,
}

impl Tab {
    pub fn id(&self) -> TabId {
        self.id
    }

    /// The name the tab shows.
    pub fn title(&self) -> &str {
        match &self.content {
            TabContent::File { file, .. } => &file.name,
            TabContent::Page { title, .. } => title,
        }
    }

    /// Whether this is the preview tab, which the next preview replaces.
    pub fn is_preview(&self) -> bool {
        self.preview
    }

    /// Where you were in the tab.
    pub fn place(&self) -> &TabPlace {
        &self.place
    }

    pub fn content(&self) -> &TabContent {
        &self.content
    }

    /// The file the tab shows, if it shows one.
    pub fn file(&self) -> Option<&OpenedFile> {
        match &self.content {
            TabContent::File { file, .. } => Some(file),
            TabContent::Page { .. } => None,
        }
    }

    /// The rail page the tab shows, if it shows one.
    pub fn page_id(&self) -> Option<&str> {
        match &self.content {
            TabContent::Page { page_id, .. } => Some(page_id),
            TabContent::File { .. } => None,
        }
    }

    fn shows(&self, content: &TabContent) -> bool {
        match (&self.content, content) {
            (TabContent::File { file, .. }, TabContent::File { file: other, .. }) => {
                file.relative_path == other.relative_path
            }
            (TabContent::Page { page_id, .. }, TabContent::Page { page_id: other, .. }) => {
                page_id == other
            }
            _ => false,
        }
    }
}

/// When the tabs where `kept` is false close, the new index of the tab at
/// `index`, or of the tab that takes its place: the next tab that stays,
/// or else the last one before it. `None` when no tab stays or `index` is
/// past the end.
pub(crate) fn takes_place(kept: &[bool], index: usize) -> Option<usize> {
    if index >= kept.len() {
        return None;
    }
    let before = kept[..index].iter().filter(|kept| **kept).count();
    let stays_after = kept[index..].iter().any(|kept| *kept);
    if stays_after {
        Some(before)
    } else {
        before.checked_sub(1)
    }
}

/// A saved tab as it comes back with its workspace.
#[derive(Debug)]
pub(crate) enum Restored {
    /// The tab opens: what it shows, and whether it's the preview.
    Open(TabContent, bool),
    /// A page tab whose page isn't in the rail yet, such as a plugin's
    /// page that registers after the workspace opens.
    Waiting { page_id: String, preview: bool },
    /// The tab's file or page is gone: it drops out as if closed.
    Gone,
}

/// A saved page tab waiting for its page to register. It keeps its place
/// in the saved tabs meanwhile, as the rail keeps a missing page's place.
#[derive(Debug)]
struct WaitingPage {
    page_id: String,
    preview: bool,
    /// Its place among the saved tabs.
    index: usize,
}

/// A window's tabs, in order, and the active one.
#[derive(Debug, Default)]
pub(crate) struct Tabs {
    tabs: Vec<Tab>,
    active: Option<TabId>,
    /// The document tab that was active before a page tab took over.
    last_document: Option<TabId>,
    /// Saved page tabs waiting for their page, in saved order.
    waiting: Vec<WaitingPage>,
    /// The waiting page whose tab was active when the tabs were saved. It
    /// shows once its page registers, unless you pick a tab first.
    active_waiting: Option<String>,
    next_id: u64,
}

impl Tabs {
    pub(crate) fn list(&self) -> &[Tab] {
        &self.tabs
    }

    pub(crate) fn active(&self) -> Option<&Tab> {
        let active = self.active?;
        self.tabs.iter().find(|tab| tab.id == active)
    }

    /// Show `content`: switch to its tab, or open one.
    pub(crate) fn open(&mut self, content: TabContent, how: OpenAs) -> TabId {
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.shows(&content)) {
            if how == OpenAs::Kept {
                tab.preview = false;
            }
            let id = tab.id;
            self.set_active(Some(id));
            return id;
        }
        let id = self.new_id();
        let tab = Tab {
            id,
            content,
            preview: how == OpenAs::Preview,
            place: TabPlace::default(),
        };
        let preview = self.tabs.iter().position(|tab| tab.preview);
        match preview {
            Some(index) if how == OpenAs::Preview => self.tabs[index] = tab,
            _ => self.tabs.push(tab),
        }
        if how == OpenAs::Preview {
            // The new preview replaces a waiting one too.
            self.waiting.retain(|waiting| !waiting.preview);
        }
        self.set_active(Some(id));
        id
    }

    fn new_id(&mut self) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Show the tab `id`. Returns whether it changed the active tab.
    pub(crate) fn activate(&mut self, id: TabId) -> bool {
        if !self.tabs.iter().any(|tab| tab.id == id) {
            return false;
        }
        if self.active == Some(id) {
            // Picking the tab that shows keeps it over a waiting page.
            self.active_waiting = None;
            return false;
        }
        self.set_active(Some(id));
        true
    }

    /// Show the Files page: the active document tab, or the document tab
    /// that was active before a page tab took over, or the last document
    /// tab. With no document tab, nothing is active. Returns whether it
    /// changed the active tab.
    pub(crate) fn show_files(&mut self) -> bool {
        if self.active().is_some_and(|tab| tab.file().is_some()) {
            return false;
        }
        let documents = || self.tabs.iter().filter(|tab| tab.file().is_some());
        let target = documents()
            .find(|tab| Some(tab.id) == self.last_document)
            .or_else(|| documents().next_back())
            .map(|tab| tab.id);
        if target == self.active {
            return false;
        }
        self.set_active(target);
        true
    }

    /// Make `id` active, remembering the document tab a page tab takes
    /// over from.
    fn set_active(&mut self, id: Option<TabId>) {
        if let Some(current) = self.active()
            && current.file().is_some()
        {
            self.last_document = Some(current.id);
        }
        self.active = id;
        self.active_waiting = None;
    }

    pub(crate) fn get(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id == id)
    }

    pub(crate) fn set_place(&mut self, id: TabId, place: TabPlace) {
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.place = place;
        }
    }

    /// Keep the preview tab `id` open when the next preview opens.
    /// Returns whether it was the preview tab.
    pub(crate) fn keep(&mut self, id: TabId) -> bool {
        match self.tabs.iter_mut().find(|tab| tab.id == id && tab.preview) {
            Some(tab) => {
                tab.preview = false;
                true
            }
            None => false,
        }
    }

    /// Replace every tab with the saved tabs as they come back, in order.
    /// A tab that is gone drops out as if closed; when it, or a waiting
    /// page tab, was the active one, the tab that takes its place becomes
    /// active.
    pub(crate) fn restore(&mut self, restored: Vec<Restored>, active: Option<usize>) {
        self.tabs.clear();
        self.waiting.clear();
        self.active_waiting = None;
        self.last_document = None;
        // Which saved tabs open now.
        let mut opened = Vec::with_capacity(restored.len());
        // Only one tab may be the preview.
        let mut has_preview = false;
        for (index, entry) in restored.into_iter().enumerate() {
            let place = self.tabs.len() + self.waiting.len();
            // A saved list can name a file or page twice; keep the first.
            match entry {
                Restored::Open(content, preview)
                    if !self.tabs.iter().any(|tab| tab.shows(&content)) =>
                {
                    let preview = preview && !has_preview;
                    has_preview |= preview;
                    let id = self.new_id();
                    self.tabs.push(Tab {
                        id,
                        content,
                        preview,
                        place: TabPlace::default(),
                    });
                    opened.push(true);
                }
                Restored::Waiting { page_id, preview }
                    if !self
                        .waiting
                        .iter()
                        .any(|waiting| waiting.page_id == page_id) =>
                {
                    let preview = preview && !has_preview;
                    has_preview |= preview;
                    if active == Some(index) {
                        self.active_waiting = Some(page_id.clone());
                    }
                    self.waiting.push(WaitingPage {
                        page_id,
                        preview,
                        index: place,
                    });
                    opened.push(false);
                }
                _ => opened.push(false),
            }
        }
        self.active = active
            .and_then(|active| takes_place(&opened, active))
            .and_then(|index| self.tabs.get(index))
            .map(|tab| tab.id);
    }

    /// The page in `content` just joined the rail: open the tab that was
    /// waiting for it, in its saved place, and show it if it was the
    /// active tab. Returns `None` when no tab was waiting, or else whether
    /// the active tab changed.
    pub(crate) fn page_registered(&mut self, content: TabContent) -> Option<bool> {
        let TabContent::Page { page_id, .. } = &content else {
            return None;
        };
        let page_id = page_id.clone();
        let position = self
            .waiting
            .iter()
            .position(|waiting| waiting.page_id == page_id)?;
        let waiting = self.waiting.remove(position);
        if self.tabs.iter().any(|tab| tab.shows(&content)) {
            return Some(false);
        }
        // Its saved place, less the tabs before it that still wait.
        let waiting_before = self
            .waiting
            .iter()
            .filter(|other| other.index < waiting.index)
            .count();
        let at = waiting
            .index
            .saturating_sub(waiting_before)
            .min(self.tabs.len());
        let id = self.new_id();
        let preview = waiting.preview && !self.tabs.iter().any(|tab| tab.preview);
        self.tabs.insert(
            at,
            Tab {
                id,
                content,
                preview,
                place: TabPlace::default(),
            },
        );
        if self.active_waiting.as_deref() == Some(page_id.as_str()) {
            self.set_active(Some(id));
            return Some(true);
        }
        Some(false)
    }

    /// The tabs as saved in the view state, with the page tabs that are
    /// still waiting in their places.
    pub(crate) fn stored(&self) -> (Vec<StoredTab>, Option<usize>) {
        let mut tabs: Vec<StoredTab> = self
            .tabs
            .iter()
            .map(|tab| match &tab.content {
                TabContent::File { file, object_id } => StoredTab {
                    file: Some(file.relative_path.clone()),
                    object_id: object_id.clone(),
                    page: None,
                    preview: tab.preview,
                },
                TabContent::Page { page_id, .. } => StoredTab::page(page_id, tab.preview),
            })
            .collect();
        let mut active = self
            .active
            .and_then(|active| self.tabs.iter().position(|tab| tab.id == active));
        for waiting in &self.waiting {
            let at = waiting.index.min(tabs.len());
            tabs.insert(at, StoredTab::page(&waiting.page_id, waiting.preview));
            if self.active_waiting.as_deref() == Some(waiting.page_id.as_str()) {
                active = Some(at);
            } else if let Some(shifted) = active.filter(|active| *active >= at) {
                active = Some(shifted + 1);
            }
        }
        (tabs, active)
    }

    /// Point each file tab at where its file is now, or close it.
    /// `follow` gets a file tab's path and stable ID and answers with the
    /// file's path and stable ID now, or `None` when the file is gone.
    /// Returns whether any tab changed, and whether the active tab's file
    /// did.
    pub(crate) fn follow_files(
        &mut self,
        mut follow: impl FnMut(&str, Option<&str>) -> Option<(String, Option<String>)>,
    ) -> (bool, bool) {
        let active = self.active;
        let mut changed = false;
        let mut active_changed = false;
        let mut gone = Vec::new();
        for tab in &mut self.tabs {
            let TabContent::File { file, object_id } = &mut tab.content else {
                continue;
            };
            match follow(&file.relative_path, object_id.as_deref()) {
                Some((path, id)) => {
                    if path != file.relative_path {
                        *file = OpenedFile::at(&path);
                        changed = true;
                        active_changed |= Some(tab.id) == active;
                    }
                    *object_id = id;
                }
                None => gone.push(tab.id),
            }
        }
        // Two tabs that now show one file become one: the first stays, in
        // place of the later one.
        let mut index = 0;
        while index < self.tabs.len() {
            let tab = &self.tabs[index];
            let survivor = self.tabs[..index]
                .iter()
                .position(|earlier| !gone.contains(&earlier.id) && earlier.shows(&tab.content));
            match survivor {
                Some(survivor) if !gone.contains(&tab.id) => {
                    changed = true;
                    active_changed |= self.active == Some(tab.id);
                    let merged = self.tabs.remove(index);
                    self.merge_into(merged, survivor);
                }
                _ => index += 1,
            }
        }
        for id in gone {
            changed = true;
            active_changed |= self.active == Some(id);
            self.close(id);
        }
        (changed, active_changed)
    }

    /// Fold the tab `merged`, already out of the list, into the tab at
    /// `survivor`, which shows the same file: the survivor stays kept if
    /// either was, and shows when `merged` did.
    fn merge_into(&mut self, merged: Tab, survivor: usize) {
        let survivor = &mut self.tabs[survivor];
        survivor.preview &= merged.preview;
        let survivor = survivor.id;
        if self.active == Some(merged.id) {
            self.active = Some(survivor);
            self.active_waiting = None;
        }
        if self.last_document == Some(merged.id) {
            self.last_document = Some(survivor);
        }
    }

    /// Close a tab. Closing the active tab activates the tab that takes
    /// its place, or the one before it at the end; closing the last tab
    /// leaves nothing active. Returns whether the tab was open.
    /// Show the page `page_id` under its new title in the tab that shows
    /// it. Returns whether a tab changed.
    pub(crate) fn rename_page(&mut self, page_id: &str, title: &str) -> bool {
        let mut renamed = false;
        for tab in &mut self.tabs {
            if let TabContent::Page {
                page_id: shown,
                title: shown_title,
            } = &mut tab.content
                && shown == page_id
                && shown_title != title
            {
                title.clone_into(shown_title);
                renamed = true;
            }
        }
        renamed
    }

    /// The IDs of the tabs that show the page `page_id`.
    pub(crate) fn showing_page(&self, page_id: &str) -> Vec<TabId> {
        self.tabs
            .iter()
            .filter(|tab| tab.page_id() == Some(page_id))
            .map(|tab| tab.id)
            .collect()
    }

    pub(crate) fn close(&mut self, id: TabId) -> bool {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return false;
        };
        self.tabs.remove(index);
        if self.active == Some(id) {
            self.active_waiting = None;
            self.active = self
                .tabs
                .get(index.min(self.tabs.len().saturating_sub(1)))
                .map(|tab| tab.id);
        }
        true
    }
}
