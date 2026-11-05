//! Link lookups the embedding app answers. The editor never reads the
//! workspace, opens files or contacts a website itself: it asks a
//! [`LinkResolver`] where a link leads and reports what the user opened.

/// Where a link leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedLink {
    /// A file in the workspace, named however the embedding app names files.
    File(String),
    /// An address outside the workspace, such as a web page.
    External(String),
}

/// How a link draws, and why when it doesn't lead anywhere useful.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkState {
    Normal,
    Warning { message: String },
    Broken { message: String },
}

/// Answers the editor's link lookups. It is `Send` so a document can move
/// with the window that shows it.
pub trait LinkResolver: Send {
    /// Where a Markdown link's destination leads, or `None` if it leads
    /// nowhere.
    fn resolve_link(&self, url: &str) -> Option<ResolvedLink>;

    /// The file a wikilink target such as `[[Notes]]` names, or `None` if no
    /// file matches.
    fn resolve_wikilink(&self, title: &str) -> Option<String>;

    /// How a Markdown link draws.
    fn link_state(&self, url: &str) -> LinkState;

    /// How a wikilink draws.
    fn wikilink_state(&self, title: &str) -> LinkState;

    /// The note an `![[target]]` embed shows, or `None` when the target
    /// names no note. Answering reads the note and writes nothing.
    fn embed(&self, _target: &str) -> Option<EmbeddedNote> {
        None
    }

    /// The files to offer when `[[typed` is being typed, best first. The
    /// editor lists them and puts the picked one's
    /// [`link`](LinkSuggestion::link) in place of what's typed.
    fn suggest(&self, _typed: &str) -> Vec<LinkSuggestion> {
        Vec::new()
    }

    /// Changes whenever an answer could change, as when files appear,
    /// move or change.
    fn seq(&self) -> u64 {
        0
    }
}

/// A file typing `[[` offers to link to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSuggestion {
    /// What the list shows for the file.
    pub name: String,
    /// The folder the list shows beside the name, when it has to tell
    /// files of the same name apart. `""` is the top of the workspace.
    pub folder: Option<String>,
    /// What picking the file puts between `[[` and `]]`.
    pub link: String,
}

/// What an embedded note shows: its title and the start of its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedNote {
    pub title: String,
    pub text: String,
}

/// A resolver that finds nothing: every link and wikilink leads nowhere.
#[derive(Clone, Copy, Debug, Default)]
pub struct NotFound;

/// What [`NotFound`] says about every link.
pub const NOT_FOUND_MESSAGE: &str = "Nothing matches this link.";

impl LinkResolver for NotFound {
    fn resolve_link(&self, _url: &str) -> Option<ResolvedLink> {
        None
    }

    fn resolve_wikilink(&self, _title: &str) -> Option<String> {
        None
    }

    fn link_state(&self, _url: &str) -> LinkState {
        LinkState::Broken {
            message: NOT_FOUND_MESSAGE.into(),
        }
    }

    fn wikilink_state(&self, _title: &str) -> LinkState {
        LinkState::Broken {
            message: NOT_FOUND_MESSAGE.into(),
        }
    }
}

/// A link the user opened. The embedding app finds where it leads and
/// follows it; the editor opens and creates nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenedLink {
    /// The link's target as the note writes it: a wikilink's target, or a
    /// Markdown link's destination.
    pub target: String,
    /// Whether it's a `[[target]]` link.
    pub wikilink: bool,
    /// Whether to open it in a new tab and keep the current one, as
    /// Cmd-click, Ctrl-click and a middle-click ask.
    pub new_tab: bool,
}
