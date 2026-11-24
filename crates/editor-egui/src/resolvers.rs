//! Lookups the embedding app answers: links, through `editor-core`'s
//! [`LinkResolver`], and embedded images, through [`EmbedResolver`].

use egui::{CornerRadius, Rect, Ui, Vec2};

pub use editor_core::resolvers::{
    EmbeddedNote, LinkResolver, LinkState, LinkSuggestion, NotFound, OpenedLink, ResolvedLink,
};

/// What the embedding app knows about the content a URL names.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EmbedState {
    /// Still loading. The editor reserves this size, in points, and `show`
    /// paints a placeholder in it.
    Loading(Vec2),
    /// Loaded, showing at this size in points at its own size. Points
    /// keep an image's size beside the text when the zoom or the screen's
    /// density changes.
    Loaded(Vec2),
    /// A document's page, at this size in points. A page no taller than
    /// an A4 page for its width lays out as an image of that size does. A
    /// taller one lays out as wide as an A4 page would, and its height
    /// follows its shape past the bottom of the view, so a web page saved
    /// as one long page reads at the width of a paper one.
    Page(Vec2),
    /// Nothing to show: the URL names nothing the app has, or loading it
    /// failed. The editor draws a broken-image marker in its place and
    /// never asks `show` to paint it.
    NotFound,
    /// A remote image nobody asked for yet. The editor draws a Load image
    /// button in its place, and asks [`EmbedResolver::load`] when it's
    /// pressed. Nothing loads before that.
    NotLoaded,
    /// A remote image whose load failed. The editor draws a button that
    /// tries again through [`EmbedResolver::load`].
    Failed,
}

/// Loads and paints embedded content such as images. The editor asks by the
/// URL written in the note; the embedding app decides what it names and
/// whether to load it. It is `Send` so an editor can live in a window that
/// draws on its own schedule.
pub trait EmbedResolver: Send {
    /// What the app knows about the content at `url`.
    fn state(&self, url: &str) -> EmbedState;

    /// Shows the content in the provided Ui at the provided Rect, with the
    /// given corner rounding.
    fn show(&self, ui: &mut Ui, url: &str, rect: Rect, rounding: CornerRadius);

    /// Called each frame while content may be shown soon, so loading can
    /// start before it is time to show it. Remote images never load here.
    fn prefetch(&self, url: &str);

    /// Load the remote image at `url`, as its Load image button asks.
    fn load(&self, _url: &str) {}

    /// Changes whenever any return value could change.
    fn seq(&self) -> u64;
}

/// Finds nothing: every embed draws as not found.
impl EmbedResolver for () {
    fn state(&self, _url: &str) -> EmbedState {
        EmbedState::NotFound
    }
    fn show(&self, _ui: &mut Ui, _url: &str, _rect: Rect, _rounding: CornerRadius) {}
    fn prefetch(&self, _url: &str) {}
    fn seq(&self) -> u64 {
        0
    }
}
