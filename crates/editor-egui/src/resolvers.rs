//! Lookups the embedding app answers: links, through `editor-core`'s
//! [`LinkResolver`], and embedded images, through [`EmbedResolver`].

use egui::{CornerRadius, Rect, Ui, Vec2};

pub use editor_core::resolvers::{
    LinkResolver, LinkState, NotFound, OpenedLink, ResolvedLink, open_link,
};

/// What the embedding app knows about the content a URL names.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EmbedState {
    /// Still loading. The editor reserves this size, in pixels, and `show`
    /// paints a placeholder in it.
    Loading(Vec2),
    /// Loaded, at this size in pixels.
    Loaded(Vec2),
    /// Nothing to show: the URL names nothing the app has, or loading it
    /// failed. The editor draws a broken-image marker in its place and
    /// never asks `show` to paint it.
    NotFound,
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
    /// start before it is time to show it.
    fn prefetch(&self, url: &str);

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
