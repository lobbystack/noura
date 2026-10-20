//! Lookups the embedding app answers: links, through `editor-core`'s
//! [`LinkResolver`], and embedded images, through [`EmbedResolver`].

use egui::{CornerRadius, Rect, Ui, Vec2};

pub use editor_core::resolvers::{
    LinkResolver, LinkState, NotFound, OpenedLink, ResolvedLink, open_link,
};

/// Loads and paints embedded content such as images. The editor asks by the
/// URL written in the note; the embedding app decides what it names and
/// whether to load it.
pub trait EmbedResolver {
    /// Size of the content at the url.
    fn size(&self, url: &str) -> Vec2;

    /// Whether `url` has finished loading, so `show` paints it rather than a
    /// placeholder.
    fn is_loaded(&self, url: &str) -> bool;

    /// Shows the content in the provided Ui at the provided Rect, with the
    /// given corner rounding.
    fn show(&self, ui: &mut Ui, url: &str, rect: Rect, rounding: CornerRadius);

    /// Called each frame while content may be shown soon, so loading can
    /// start before it is time to show it.
    fn prefetch(&self, url: &str);

    /// Changes whenever any return value could change.
    fn seq(&self) -> u64;
}

/// Loads nothing: every embed shows as a placeholder.
impl EmbedResolver for () {
    fn size(&self, _url: &str) -> Vec2 {
        Vec2::splat(200.)
    }
    fn is_loaded(&self, _url: &str) -> bool {
        false
    }
    fn show(&self, _ui: &mut Ui, _url: &str, _rect: Rect, _rounding: CornerRadius) {}
    fn prefetch(&self, _url: &str) {}
    fn seq(&self) -> u64 {
        0
    }
}
