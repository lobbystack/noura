//! The editor's link lookups, answered from the note's links in
//! `app-core`: web addresses lead out of Noura, and every other target
//! resolves in the workspace or leads nowhere.

use app_core::{NoteEmbedState, NoteLinks, is_external_link};
use editor_egui::resolvers::{EmbeddedNote, LinkResolver, LinkState, ResolvedLink};

/// What a link that names no file in the workspace says.
const NOT_IN_WORKSPACE: &str = "No file in this workspace matches this link.";

/// Answers a note's editor about its links.
pub(crate) struct WorkspaceLinks {
    pub(crate) links: NoteLinks,
    /// Draws another frame when an embedded note's read ends.
    pub(crate) ctx: egui::Context,
}

impl LinkResolver for WorkspaceLinks {
    fn resolve_link(&self, url: &str) -> Option<ResolvedLink> {
        if is_external_link(url) {
            return Some(ResolvedLink::External(url.to_owned()));
        }
        self.links.resolve(url).map(ResolvedLink::File)
    }

    fn resolve_wikilink(&self, title: &str) -> Option<String> {
        self.links.resolve(title)
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

    /// An embedded note draws its title while its text loads.
    fn embed(&self, target: &str) -> Option<EmbeddedNote> {
        let ctx = self.ctx.clone();
        match self.links.embed(target, move || ctx.request_repaint())? {
            NoteEmbedState::Reading { title } => Some(EmbeddedNote {
                title,
                text: String::new(),
            }),
            NoteEmbedState::Read(note) => Some(EmbeddedNote {
                title: note.title,
                text: note.text,
            }),
        }
    }

    fn seq(&self) -> u64 {
        self.links.seq() ^ self.links.embeds_seq().rotate_left(32)
    }
}
