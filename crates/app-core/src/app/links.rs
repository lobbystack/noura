//! Following the links in a note: a click, a Cmd-click or a key on a link
//! opens what it leads to.

use super::App;
use crate::{
    AppError, OpenAs, TabId,
    links::{is_external_link, link_path, web_address},
    pdf::{is_pdf, page_in_link},
};

impl App {
    /// Follow the link to `target` in the note in `tab`, as a click on it
    /// does. A workspace file opens the way the file tree opens it, as
    /// `how` says, and a PDF opens at the page its `#page=` fragment
    /// names: page 1 for none or an invalid page, the last page for one
    /// past the end. An `http`, `https` or `www.` address opens in the
    /// browser, and an address with any other scheme does nothing. A link
    /// to a file the workspace doesn't hold says so and creates nothing,
    /// and a `#heading` or `#^block` part is ignored. Following a link
    /// writes nothing.
    pub fn follow_note_link(&mut self, tab: TabId, target: &str, how: OpenAs) {
        let target = target.trim();
        if let Some(url) = web_address(target) {
            self.open_link(&url);
            return;
        }
        if is_external_link(target) {
            tracing::debug!("a link with a scheme Noura doesn't open was not followed");
            return;
        }
        let Some(links) = self.note_links(tab) else {
            return;
        };
        let path = link_path(target);
        // A fragment alone leads to a heading or block of the note itself.
        if path.is_empty() {
            return;
        }
        match links.resolve(target) {
            Some(found) if is_pdf(&found) => self.open_pdf_page(&found, page_in_link(target), how),
            Some(found) => self.open_file(&found, how),
            None => {
                let message = format!("\u{201c}{path}\u{201d} isn't in this workspace yet.");
                self.show_notice(
                    "Couldn't open the link",
                    AppError::new("link_target_missing", message),
                    None,
                );
            }
        }
    }
}
