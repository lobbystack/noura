//! The editor's link lookups, answered from the note's links in
//! `app-core`: web addresses lead out of Noura, and every other target
//! resolves in the workspace or leads nowhere.

use app_core::NoteLinks;
use editor_egui::resolvers::{EmbeddedNote, LinkResolver, LinkState, ResolvedLink};

/// What a link that names no file in the workspace says.
const NOT_IN_WORKSPACE: &str = "No file in this workspace matches this link.";

/// Answers a note's editor about its links.
pub(crate) struct WorkspaceLinks(pub(crate) NoteLinks);

/// Whether `url` leaves the workspace: a web address, or any address with
/// a scheme such as `mailto:`.
fn is_web(url: &str) -> bool {
    let url = url.trim_start();
    if url.to_ascii_lowercase().starts_with("www.") {
        return true;
    }
    url.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme.starts_with(|ch: char| ch.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '.' | '-'))
    })
}

impl LinkResolver for WorkspaceLinks {
    fn resolve_link(&self, url: &str) -> Option<ResolvedLink> {
        if is_web(url) {
            return Some(ResolvedLink::External(url.to_owned()));
        }
        self.0.resolve(url).map(ResolvedLink::File)
    }

    fn resolve_wikilink(&self, title: &str) -> Option<String> {
        self.0.resolve(title)
    }

    fn link_state(&self, url: &str) -> LinkState {
        match self.resolve_link(url) {
            Some(_) => LinkState::Normal,
            None => LinkState::Broken {
                message: NOT_IN_WORKSPACE.into(),
            },
        }
    }

    fn wikilink_state(&self, title: &str) -> LinkState {
        match self.resolve_wikilink(title) {
            Some(_) => LinkState::Normal,
            None => LinkState::Broken {
                message: NOT_IN_WORKSPACE.into(),
            },
        }
    }

    fn embed(&self, target: &str) -> Option<EmbeddedNote> {
        self.0.embed(target).map(|note| EmbeddedNote {
            title: note.title,
            text: note.text,
        })
    }

    fn seq(&self) -> u64 {
        self.0.seq()
    }
}
