//! The system spell checker: NSSpellChecker on macOS and ISpellChecker on
//! Windows. Linux has none, so [`SystemSpellChecker::new`] returns `None`
//! there.
//!
//! This adapter lives outside `app-core` and `editor-core`, which stay free
//! of desktop-only code. The desktop app hands it to the editor.

use std::ops::Range;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as platform;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use std::ops::Range;

    pub(crate) enum Checker {}

    impl Checker {
        pub(crate) fn new(_language: Option<&str>) -> Option<Self> {
            None
        }

        pub(crate) fn misspelled(&self, _text: &str) -> Vec<Range<usize>> {
            match *self {}
        }
    }
}

/// The system's spell checker.
pub struct SystemSpellChecker {
    checker: platform::Checker,
}

impl SystemSpellChecker {
    /// The spell checker for the languages the user chose in the system's
    /// settings, or `None` where the system has none.
    pub fn new() -> Option<Self> {
        platform::Checker::new(None).map(|checker| Self { checker })
    }

    /// The spell checker for `language`, a BCP 47 tag such as `en-US`, or
    /// `None` where the system can't check it.
    pub fn for_language(language: &str) -> Option<Self> {
        platform::Checker::new(Some(language)).map(|checker| Self { checker })
    }

    /// The byte ranges of the misspelled words in `text`, in order.
    pub fn misspelled(&self, text: &str) -> Vec<Range<usize>> {
        if text.is_empty() {
            return Vec::new();
        }
        self.checker.misspelled(text)
    }
}

/// Where each UTF-16 code unit of `text` starts, as a byte offset, followed
/// by the text's length: both system checkers count in UTF-16.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn utf16_to_bytes(text: &str) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(text.len() + 1);
    for (byte, character) in text.char_indices() {
        for _ in 0..character.len_utf16() {
            offsets.push(byte);
        }
    }
    offsets.push(text.len());
    offsets
}

/// The byte range of the UTF-16 range `start..start + length` of a text
/// whose code units start at `offsets`, or `None` when it falls outside.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn byte_range(offsets: &[usize], start: usize, length: usize) -> Option<Range<usize>> {
    let end = start.checked_add(length)?;
    let range = *offsets.get(start)?..*offsets.get(end)?;
    (range.start < range.end).then_some(range)
}
