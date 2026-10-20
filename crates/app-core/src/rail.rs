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

/// The pages registered so far, in the order they arrived.
#[derive(Debug)]
pub(crate) struct Rail {
    pages: Vec<Page>,
}

impl Default for Rail {
    fn default() -> Self {
        Self {
            pages: vec![Page {
                id: FILES_PAGE.to_owned(),
                title: "Files".to_owned(),
            }],
        }
    }
}

impl Rail {
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

    pub(crate) fn page(&self, id: &str) -> Option<&Page> {
        self.pages.iter().find(|page| page.id == id)
    }
}
