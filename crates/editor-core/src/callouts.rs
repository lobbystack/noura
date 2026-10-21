//! Obsidian-style callouts: a quote whose first line starts with
//! `[!type]`, optionally followed by `+` or `-`. Any type counts. The
//! callout draws labeled with its type and hides its `>` markers, and shows
//! its whole source while the caret is anywhere inside it.

use comrak::nodes::{AstNode, NodeValue};

use crate::doc::MdDoc;
use crate::offset_types::{Grapheme, RangeExt as _, RangeIterExt as _};

/// A callout in the note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Callout {
    /// The type as written, such as `note` in `[!note]`.
    pub kind: String,
    /// `+` or `-` after the marker, which Obsidian uses to fold callouts.
    pub fold: Option<char>,
    /// The `[!type]` marker with its fold sign.
    pub marker: (Grapheme, Grapheme),
    /// The rest of the first line after the marker, without the spaces
    /// around it. Empty when the callout has no title.
    pub title: (Grapheme, Grapheme),
}

impl Callout {
    /// What the editor draws in place of the marker: the type with each
    /// word capitalized, as today's editor shows it.
    pub fn label(&self) -> String {
        capitalized(&self.kind)
    }

    /// The color family the callout draws in, by Obsidian's type names.
    pub fn family(&self) -> CalloutFamily {
        match self.kind.to_lowercase().as_str() {
            "tip" | "hint" | "important" => CalloutFamily::Tip,
            "success" | "check" | "done" => CalloutFamily::Success,
            "question" | "help" | "faq" => CalloutFamily::Question,
            "warning" | "caution" | "attention" => CalloutFamily::Warning,
            "failure" | "fail" | "missing" | "danger" | "error" | "bug" => CalloutFamily::Danger,
            "example" => CalloutFamily::Example,
            "quote" | "cite" => CalloutFamily::Quote,
            _ => CalloutFamily::Note,
        }
    }
}

/// The color families callouts draw in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalloutFamily {
    Note,
    Tip,
    Success,
    Question,
    Warning,
    Danger,
    Example,
    Quote,
}

fn capitalized(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut word_start = true;
    for ch in text.chars() {
        if word_start {
            result.extend(ch.to_uppercase());
        } else {
            result.push(ch);
        }
        word_start = ch.is_whitespace();
    }
    result
}

/// The length in bytes of a `[!type]` marker with its fold sign at the
/// start of `text`, and the type and fold sign.
fn marker(text: &str) -> Option<(usize, &str, Option<char>)> {
    let rest = text.strip_prefix("[!")?;
    let close = rest.find(']')?;
    let kind = &rest[..close];
    if kind.is_empty() || kind.contains(['\n', '\r']) {
        return None;
    }
    let mut len = 2 + close + 1;
    let fold = match rest[close + 1..].chars().next() {
        Some(sign @ ('+' | '-')) => {
            len += 1;
            Some(sign)
        }
        _ => None,
    };
    Some((len, kind, fold))
}

impl<'ast> MdDoc {
    /// The callout `node` is, if it's a quote whose first line starts with
    /// a `[!type]` marker.
    pub fn callout(&self, node: &'ast AstNode<'ast>) -> Option<Callout> {
        if !matches!(node.data.borrow().value, NodeValue::BlockQuote) {
            return None;
        }
        let paragraph = node.first_child()?;
        if !matches!(paragraph.data.borrow().value, NodeValue::Paragraph) {
            return None;
        }
        // The marker must be text, not the start of a code span or link.
        let first_inline = paragraph.first_child()?;
        if !matches!(first_inline.data.borrow().value, NodeValue::Text(_)) {
            return None;
        }
        let text_range = self.node_range(first_inline);
        let paragraph_range = self.node_range(paragraph);
        if text_range.start() != paragraph_range.start() {
            return None;
        }
        let line = self.node_first_line(paragraph);
        let content = self.node_line(paragraph, line);
        let text = &self.buffer[content];
        let (len, kind, fold) = marker(text)?;
        // Byte offsets in the line; a mark that combines with the marker's
        // last character belongs to the marker.
        let start = self.offset_to_byte(content.start());
        let after = &text[len..];
        let title_from = len + (after.len() - after.trim_start().len());
        let title_to = title_from + after.trim().len();
        let (marker_start, marker_end) = self.range_to_char_ceil((start, start + len));
        if marker_end > text_range.end() {
            return None;
        }
        let (title_start, title_end) =
            self.range_to_char_ceil((start + title_from, start + title_to.max(title_from)));
        Some(Callout {
            kind: kind.to_string(),
            fold,
            marker: (marker_start, marker_end),
            title: (title_start.max(marker_end), title_end.max(marker_end)),
        })
    }

    /// The syntax a callout hides while the caret is elsewhere: the `>`
    /// marker of each of its lines and its `[!type]` marker, in order.
    pub fn callout_syntax(
        &self,
        node: &'ast AstNode<'ast>,
        callout: &Callout,
    ) -> Vec<(Grapheme, Grapheme)> {
        let mut syntax: Vec<(Grapheme, Grapheme)> = self
            .node_lines(node)
            .iter()
            .map(|line_idx| self.line_own_prefix(node, self.bounds.source_lines[line_idx]))
            .filter(|prefix| !prefix.is_empty())
            .collect();
        syntax.push(callout.marker);
        syntax.sort();
        syntax
    }

    /// Whether a callout shows its source: while the caret, the selection,
    /// a find match or a search preview touches it anywhere.
    pub fn callout_revealed(&self, node: &'ast AstNode<'ast>) -> bool {
        self.range_revealed(self.node_range(node), true)
    }
}

#[cfg(test)]
mod tests {
    use super::{capitalized, marker};

    #[test]
    fn markers_name_a_type_and_an_optional_fold_sign() {
        assert_eq!(marker("[!note] Title"), Some((7, "note", None)));
        assert_eq!(marker("[!faq]- x"), Some((7, "faq", Some('-'))));
        assert_eq!(marker("[!a b]+"), Some((7, "a b", Some('+'))));
        assert_eq!(marker("[!] x"), None);
        assert_eq!(marker("[note]"), None);
        assert_eq!(marker("[!note"), None);
    }

    #[test]
    fn labels_capitalize_each_word() {
        assert_eq!(capitalized("note"), "Note");
        assert_eq!(capitalized("my-type"), "My-type");
        assert_eq!(capitalized("two words"), "Two Words");
        assert_eq!(capitalized("éclair"), "Éclair");
    }
}
