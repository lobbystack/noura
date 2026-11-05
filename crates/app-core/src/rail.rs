//! The rail: the pages a window can show, such as Files and the Inbox.

/// The rail page that shows the file tree's documents. It has no tab of
/// its own: it shows the active document tab.
pub const FILES_PAGE: &str = "files";

/// The Inbox's page ID. The rail puts it first until you move it.
pub const INBOX_PAGE: &str = "inbox";

/// A page in the rail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub id: String,
    pub title: String,
}

/// The pages registered so far, in the order they arrived, and the order
/// you gave the open workspace's rail.
#[derive(Debug, Clone)]
pub(crate) struct Rail {
    pages: Vec<Page>,
    /// Page IDs in the order you left them, including pages that aren't
    /// registered now, such as a disabled plugin's, so they come back to
    /// their place. Empty until you move a page.
    saved: Vec<String>,
}

impl Default for Rail {
    /// The kernel's pages, which every window has whatever plugins are on:
    /// the Inbox and Files.
    fn default() -> Self {
        Self {
            pages: vec![
                Page {
                    id: INBOX_PAGE.to_owned(),
                    title: "Inbox".to_owned(),
                },
                Page {
                    id: FILES_PAGE.to_owned(),
                    title: "Files".to_owned(),
                },
            ],
            saved: Vec::new(),
        }
    }
}

impl Rail {
    /// The saved order, as the view state keeps it.
    pub(crate) fn saved_order(&self) -> &[String] {
        &self.saved
    }

    /// Use a saved order, such as the open workspace's.
    pub(crate) fn set_saved_order(&mut self, order: Vec<String>) {
        let mut saved: Vec<String> = Vec::with_capacity(order.len());
        for id in order {
            if !saved.contains(&id) {
                saved.push(id);
            }
        }
        self.saved = saved;
    }

    /// Every page ID in rail order, registered or not: the Inbox first
    /// until you move it, then the saved order, then pages that arrived
    /// since, in the order they arrived.
    fn full_order(&self) -> Vec<&str> {
        let mut order: Vec<&str> = Vec::new();
        if !self.saved.iter().any(|id| id == INBOX_PAGE) && self.page(INBOX_PAGE).is_some() {
            order.push(INBOX_PAGE);
        }
        let ids = self
            .saved
            .iter()
            .map(String::as_str)
            .chain(self.pages.iter().map(|page| page.id.as_str()));
        for id in ids {
            if !order.contains(&id) {
                order.push(id);
            }
        }
        order
    }

    /// The registered pages in rail order.
    pub(crate) fn ordered(&self) -> Vec<&Page> {
        self.full_order()
            .into_iter()
            .filter_map(|id| self.page(id))
            .collect()
    }

    /// Put the page `id` at `index` in the rail. Returns whether the
    /// order changed.
    pub(crate) fn move_page(&mut self, id: &str, index: usize) -> bool {
        let mut shown: Vec<String> = self.ordered().iter().map(|page| page.id.clone()).collect();
        let Some(from) = shown.iter().position(|shown| shown == id) else {
            return false;
        };
        let page = shown.remove(from);
        let to = index.min(shown.len());
        if to == from {
            return false;
        }
        shown.insert(to, page);
        // Pages that aren't registered keep their slots; the registered
        // ones fill theirs in the new order.
        let mut moved = shown.into_iter();
        let saved: Vec<String> = self
            .full_order()
            .into_iter()
            .map(|slot| match self.page(slot) {
                Some(_) => moved.next().unwrap_or_default(),
                None => slot.to_owned(),
            })
            .collect();
        self.saved = saved;
        true
    }

    /// Add a page, or rename one that is already there.
    pub(crate) fn register(&mut self, id: &str, title: &str) {
        match self.pages.iter_mut().find(|page| page.id == id) {
            Some(page) => title.clone_into(&mut page.title),
            None => self.pages.push(Page {
                id: id.to_owned(),
                title: title.to_owned(),
            }),
        }
    }

    /// Take a page out, as when its plugin stops. Its place in the saved
    /// order stays, so it comes back there. Returns whether it was there.
    pub(crate) fn unregister(&mut self, id: &str) -> bool {
        let before = self.pages.len();
        self.pages.retain(|page| page.id != id);
        self.pages.len() != before
    }

    pub(crate) fn page(&self, id: &str) -> Option<&Page> {
        self.pages.iter().find(|page| page.id == id)
    }
}
