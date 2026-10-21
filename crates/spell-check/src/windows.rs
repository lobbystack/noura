//! ISpellChecker, the Windows spell checking API.
//!
//! A COM spell checker belongs to the thread that made it, so each thread
//! that checks makes its own, once per language.

use std::cell::RefCell;
use std::ops::Range;

use windows::Win32::Globalization::{
    CORRECTIVE_ACTION_NONE, GetUserDefaultLocaleName, ISpellChecker, ISpellCheckerFactory,
    SpellCheckerFactory,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::core::HSTRING;

/// The longest locale name, with its terminating null, in UTF-16 units.
const LOCALE_NAME_MAX_LENGTH: usize = 85;

pub(crate) struct Checker {
    /// The language to check, as a BCP 47 tag.
    language: HSTRING,
}

thread_local! {
    /// This thread's spell checkers, by language.
    static CHECKERS: RefCell<Vec<(HSTRING, ISpellChecker)>> = const { RefCell::new(Vec::new()) };
}

impl Checker {
    pub(crate) fn new(language: Option<&str>) -> Option<Self> {
        let language = match language {
            Some(language) => HSTRING::from(language),
            None => user_language()?,
        };
        with_checker(&language, |_| ())?;
        Some(Self { language })
    }

    pub(crate) fn misspelled(&self, text: &str) -> Vec<Range<usize>> {
        with_checker(&self.language, |checker| check(checker, text)).unwrap_or_default()
    }
}

/// Runs `work` with this thread's spell checker for `language`, made on
/// first use, or returns `None` when Windows can't check `language`.
fn with_checker<R>(language: &HSTRING, work: impl FnOnce(&ISpellChecker) -> R) -> Option<R> {
    CHECKERS.with(|checkers| {
        let mut checkers = checkers.borrow_mut();
        let index = match checkers
            .iter()
            .position(|(made_for, _)| made_for == language)
        {
            Some(index) => index,
            None => {
                checkers.push((language.clone(), create(language)?));
                checkers.len() - 1
            }
        };
        Some(work(&checkers[index].1))
    })
}

fn create(language: &HSTRING) -> Option<ISpellChecker> {
    // SAFETY: COM calls on this thread once COM is initialized. A thread
    // that already initialized COM, as winit's does, keeps its own mode.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let factory: ISpellCheckerFactory =
            CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        if !factory.IsSupported(language).ok()?.as_bool() {
            return None;
        }
        factory.CreateSpellChecker(language).ok()
    }
}

fn check(checker: &ISpellChecker, text: &str) -> Vec<Range<usize>> {
    let offsets = super::utf16_to_bytes(text);
    let wide = HSTRING::from(text);
    let mut found = Vec::new();
    // SAFETY: `wide` outlives the check, and the errors are read on the
    // thread that made the checker.
    unsafe {
        let Ok(errors) = checker.Check(&wide) else {
            return found;
        };
        loop {
            let mut error = None;
            if errors.Next(&mut error).is_err() {
                break;
            }
            let Some(error) = error else {
                break;
            };
            if error
                .CorrectiveAction()
                .is_ok_and(|action| action == CORRECTIVE_ACTION_NONE)
            {
                continue;
            }
            let (Ok(start), Ok(length)) = (error.StartIndex(), error.Length()) else {
                continue;
            };
            if let Some(bytes) = super::byte_range(&offsets, start as usize, length as usize) {
                found.push(bytes);
            }
        }
    }
    found
}

/// The user's locale, such as `en-US`.
fn user_language() -> Option<HSTRING> {
    let mut name = [0u16; LOCALE_NAME_MAX_LENGTH];
    // SAFETY: the buffer holds the longest locale name.
    let written = unsafe { GetUserDefaultLocaleName(&mut name) };
    let length = usize::try_from(written).ok()?.checked_sub(1)?;
    Some(HSTRING::from_wide(&name[..length]))
}
