//! Release check: the system spell checker flags a known misspelling on
//! macOS and Windows, and Linux has none.

use spell_check::SystemSpellChecker;

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn the_system_spell_checker_flags_a_known_misspelling() {
    let checker =
        SystemSpellChecker::for_language("en-US").expect("the system checks English spelling");
    let text = "I recieve the parcel today, and naïve café owners recieve theirs.";

    let found: Vec<&str> = checker
        .misspelled(text)
        .into_iter()
        .map(|range| &text[range])
        .collect();

    assert_eq!(found, ["recieve", "recieve"]);
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn the_user_s_languages_have_a_spell_checker() {
    let checker = SystemSpellChecker::new().expect("the system has a spell checker");

    assert_eq!(checker.misspelled(""), []);
}

#[test]
fn every_window_can_share_one_spell_checker() {
    fn shared<T: Send + Sync>() {}
    shared::<SystemSpellChecker>();
}

#[test]
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn linux_has_no_spell_checker() {
    assert!(SystemSpellChecker::new().is_none());
}
