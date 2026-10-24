//! Following the links in a note: a click, a Cmd-click or a key on a link
//! opens what it leads to.

use super::App;
use crate::{
    AppError, OpenAs, TabId,
    links::{is_external_link, link_path},
};

impl App {
    /// Follow the link to `target` in the note in `tab`, as a click on it
    /// does. A workspace file opens the way the file tree opens it, as
    /// `how` says. An `http`, `https` or `www.` address opens in the
    /// browser, and an address with any other scheme does nothing. A link
    /// to a file the workspace doesn't hold says so and creates nothing,
    /// and a `#heading` or `#^block` part is ignored.
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

/// The web page a link to `target` opens: `target` itself when it starts
/// with `http://` or `https://`, and `https://` before it when it starts
/// with `www.`.
fn web_address(target: &str) -> Option<String> {
    let starts_with = |prefix: &str| {
        target
            .get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
    };
    if starts_with("http://") || starts_with("https://") {
        Some(target.to_owned())
    } else if starts_with("www.") {
        Some(format!("https://{target}"))
    } else {
        None
    }
}
