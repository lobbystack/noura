//! The system spell checker, as the notes ask it: NSSpellChecker on macOS
//! and ISpellChecker on Windows. Linux has none.

use std::ops::Range;
use std::sync::Arc;

use app_ui::SpellChecker;
use spell_check::SystemSpellChecker;

struct SystemSpelling(SystemSpellChecker);

impl SpellChecker for SystemSpelling {
    fn misspelled(&self, text: &str) -> Vec<Range<usize>> {
        self.0.misspelled(text)
    }
}

/// The system's spell checker for the user's languages, or `None` where
/// the system has none.
pub fn system_spell_checker() -> Option<Arc<dyn SpellChecker>> {
    let checker = SystemSpellChecker::new()?;
    Some(Arc::new(SystemSpelling(checker)))
}
