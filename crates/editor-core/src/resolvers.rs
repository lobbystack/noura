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

/// Answers the editor's link lookups.
pub trait LinkResolver {
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

/// The outcome of the user opening a link. The embedding app acts on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenedLink {
    /// Open this workspace file.
    File(String),
    /// Hand this address to the operating system.
    External(String),
    /// The link leads nowhere. Nothing was opened or created.
    NotFound { target: String },
}

/// Resolves a link the user opened. `wikilink` marks a `[[target]]` link.
pub fn open_link(resolver: &dyn LinkResolver, target: &str, wikilink: bool) -> OpenedLink {
    let resolved = if wikilink {
        resolver.resolve_wikilink(target).map(ResolvedLink::File)
    } else {
        resolver.resolve_link(target)
    };
    match resolved {
        Some(ResolvedLink::File(file)) => OpenedLink::File(file),
        Some(ResolvedLink::External(url)) => OpenedLink::External(url),
        None => OpenedLink::NotFound {
            target: target.into(),
        },
    }
}
