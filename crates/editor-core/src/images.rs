//! Inline images as atoms: the cursor steps past a whole image, a backspace
//! against one selects it, and `|WxH` in the alt text sets its size.

use comrak::nodes::{AstNode, NodeValue};

use crate::doc::MdDoc;
use crate::input::{Advance, Bound, Increment, Region};
use crate::offset_types::RangeExt as _;
use crate::operation_types::Operation;

/// Explicit image dimensions in logical points; `None` means "use the
/// natural / fit-to-width size for that axis."
#[derive(Clone, Copy, Default)]
pub struct ImageDims {
    pub width: Option<f32>,
    pub height: Option<f32>,
}

/// Parse Obsidian's `|W` / `|WxH` size suffix off an image's alt text. Only a
/// trailing segment of positive integers (optionally split by a single `x`)
/// counts — anything else leaves a literal `|` in the alt and yields no dims.
fn parse_image_dims(alt: &str) -> ImageDims {
    let Some(bar) = alt.rfind('|') else {
        return ImageDims::default();
    };
    let mut parts = alt[bar + 1..].split('x');
    let w = parts.next().unwrap_or("");
    let h = parts.next();
    if parts.next().is_some() {
        return ImageDims::default(); // more than one `x`
    }

    let dim = |s: &str| -> Option<f32> {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse::<u32>().ok())
            .flatten()
            .filter(|n| *n > 0)
            .map(|n| n as f32)
    };

    match h {
        None => match dim(w) {
            Some(width) => ImageDims {
                width: Some(width),
                height: None,
            },
            None => ImageDims::default(),
        },
        // `WxH`: both must be valid, else the `|…` isn't a size spec
        Some(h) => match (dim(w), dim(h)) {
            (Some(width), Some(height)) => ImageDims {
                width: Some(width),
                height: Some(height),
            },
            _ => ImageDims::default(),
        },
    }
}

impl<'ast> MdDoc {
    /// Obsidian-style explicit dimensions for an image, parsed from the
    /// trailing `|W` / `|WxH` of its alt text (`![alt|100x100](url)`).
    pub fn image_dims(&self, node: &'ast AstNode<'ast>) -> ImageDims {
        match self.infix_range(node) {
            Some(range) => parse_image_dims(&self.buffer[range]),
            None => ImageDims::default(),
        }
    }

    /// Recompute `super::super::super::bounds::Bounds::images` — the source
    /// range of every inline image. Images are empty when they render raw
    /// (disabled), since then the source is plain editable text. Depends on
    /// text only.
    pub fn calc_atom_bounds<'a>(&mut self, root: &'a AstNode<'a>) {
        let mut images = Vec::new();
        for node in root.descendants() {
            // A plain-text file shows an image's source as text.
            if let NodeValue::Image(_) = node.data.borrow().value
                && !self.disable_images
                && !self.plaintext
            {
                images.push(self.node_range(node));
            }
        }
        images.sort_unstable();
        self.bounds.images = images;
    }

    /// If the selection exactly covers a collapsed image (i.e. a tap-selected
    /// atom), select its URL ([`Self::atom_edit_selection`]) — the selection
    /// endpoints inside the syntax reveal the source, and the likeliest edit
    /// becomes type-to-replace. Touch platforms trigger this from the edit
    /// menu's "Edit" action — with no arrow keys, there's otherwise no way to
    /// move the cursor into the syntax. True if handled (op pushed).
    pub fn enter_at_image(
        &self,
        root: &'ast AstNode<'ast>,
        operations: &mut Vec<Operation>,
    ) -> bool {
        let selection = self.buffer.current.selection;
        for node in root.descendants() {
            if !matches!(node.data.borrow().value, NodeValue::Image(_)) {
                continue;
            }
            let img = self.node_range(node);
            if selection != img || self.range_revealed_interior(img) {
                continue;
            }
            operations.push(Operation::Select(self.atom_edit_selection(node).0));
            return true;
        }
        false
    }

    /// Backspace/forward-delete against a collapsed image selects it rather
    /// than erasing a character of its source; a second press then deletes the
    /// selection. Mirrors [`Self::delete_at_fold`]'s non-destructive edit. True
    /// if handled (ops pushed).
    pub fn delete_at_image(&self, region: Region, operations: &mut Vec<Operation>) -> bool {
        let Region::SelectionOrAdvance {
            advance: Advance::Next(Bound::Word) | Advance::By(Increment::Char),
            backwards,
        } = region
        else {
            return false;
        };
        // selection must be empty
        let Some(offset) = self.selection_offset() else {
            return false;
        };
        for &img in &self.bounds.images {
            if self.range_revealed_interior(img) {
                continue;
            }
            let against = if backwards {
                offset == img.end()
            } else {
                offset == img.start()
            };
            if against {
                operations.push(Operation::Select(img));
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::parse_image_dims;

    #[test]
    fn obsidian_image_dims() {
        let wh = |s: &str| {
            let d = parse_image_dims(s);
            (d.width, d.height)
        };
        // valid specs
        assert_eq!(wh("alt|100x40"), (Some(100.0), Some(40.0)));
        assert_eq!(wh("alt|100"), (Some(100.0), None));
        assert_eq!(wh("|100x40"), (Some(100.0), Some(40.0))); // empty alt
        assert_eq!(wh("a|b|100"), (Some(100.0), None)); // only the last `|` segment
        // not a size: the `|` stays part of the alt
        assert_eq!(wh("alt"), (None, None));
        assert_eq!(wh("alt|caption"), (None, None));
        assert_eq!(wh("alt|100x"), (None, None)); // missing height
        assert_eq!(wh("alt|100xx40"), (None, None)); // two `x`
        assert_eq!(wh("alt|100X40"), (None, None)); // uppercase X isn't the separator
        assert_eq!(wh("alt|0"), (None, None)); // zero rejected
        assert_eq!(wh("alt|100x0"), (None, None)); // zero dim rejected
    }
}
