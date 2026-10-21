//! NSSpellChecker, AppKit's shared spell checker.

use std::ops::Range;

use objc2::rc::autoreleasepool;
use objc2_app_kit::NSSpellChecker;
use objc2_foundation::NSString;

pub(crate) struct Checker {
    /// The spell document the checks belong to, so words ignored in Noura
    /// stay apart from other apps.
    tag: isize,
    /// AppKit's name for the language to check, or `None` for the
    /// spelling panel's choice, which by default follows the user's
    /// languages.
    language: Option<String>,
}

impl Checker {
    pub(crate) fn new(language: Option<&str>) -> Option<Self> {
        let language = match language {
            Some(language) => Some(available(language)?),
            None => None,
        };
        Some(Self {
            tag: NSSpellChecker::uniqueSpellDocumentTag(),
            language,
        })
    }

    pub(crate) fn misspelled(&self, text: &str) -> Vec<Range<usize>> {
        let offsets = super::utf16_to_bytes(text);
        let units = offsets.len() - 1;
        autoreleasepool(|_| {
            let checker = NSSpellChecker::sharedSpellChecker();
            let string = NSString::from_str(text);
            let language = self.language.as_deref().map(NSString::from_str);
            let mut found = Vec::new();
            let mut start = 0;
            while start < units {
                // SAFETY: a null word count asks AppKit not to count words.
                let range = unsafe {
                    checker
                        .checkSpellingOfString_startingAt_language_wrap_inSpellDocumentWithTag_wordCount(
                            &string,
                            start as isize,
                            language.as_deref(),
                            false,
                            self.tag,
                            std::ptr::null_mut(),
                        )
                };
                // NSNotFound, or nothing more to report.
                if range.length == 0 || range.location < start || range.location >= units {
                    break;
                }
                if let Some(bytes) = super::byte_range(&offsets, range.location, range.length) {
                    found.push(bytes);
                }
                start = range.location + range.length;
            }
            found
        })
    }
}

impl Drop for Checker {
    fn drop(&mut self) {
        NSSpellChecker::sharedSpellChecker().closeSpellDocumentWithTag(self.tag);
    }
}

/// AppKit's name for `language`, such as `en_US` or `en` for `en-US`, when
/// it can check it.
fn available(language: &str) -> Option<String> {
    let wanted = language.replace('-', "_");
    let base = wanted.split('_').next().unwrap_or_default().to_owned();
    let names: Vec<String> = autoreleasepool(|_| {
        NSSpellChecker::sharedSpellChecker()
            .availableLanguages()
            .to_vec()
            .into_iter()
            .map(|name| name.to_string())
            .collect()
    });
    let name = names
        .iter()
        .find(|name| **name == wanted)
        .or_else(|| names.iter().find(|name| **name == base))
        .or_else(|| {
            names
                .iter()
                .find(|name| name.split('_').next() == Some(base.as_str()))
        })?;
    Some(name.clone())
}
