//! Seam 2: the editor's shortcuts and what each one does to the text.

use std::collections::HashSet;

use editor_core::MdDoc;
use editor_core::commands::Format;
use editor_core::input::{Event, Location, NoLayout, Region};
use editor_core::keymap::{self, Command, Key, Shortcut};
use editor_core::offset_types::Grapheme;

/// The text after pressing `shortcut` with `from..to` of `text` selected.
fn press(text: &str, from: usize, to: usize, shortcut: Shortcut) -> String {
    let command = keymap::command_for(shortcut, false)
        .unwrap_or_else(|| panic!("{shortcut:?} runs no command"));
    let mut doc = MdDoc::new(text);
    doc.apply(
        [Event::Select {
            region: Region::BetweenLocations {
                start: Location::Grapheme(Grapheme(from)),
                end: Location::Grapheme(Grapheme(to)),
            },
        }],
        &NoLayout,
    );
    doc.apply(command.event(), &NoLayout);
    doc.text().to_owned()
}

#[test]
fn the_obsidian_keys_format_the_selection() {
    let command = Shortcut::command;
    assert_eq!(press("word", 0, 4, command(Key::B)), "**word**");
    assert_eq!(press("word", 0, 4, command(Key::I)), "*word*");
    assert_eq!(press("word", 0, 4, command(Key::U)), "<u>word</u>");
    assert_eq!(
        press("word", 0, 4, command(Key::X).with_shift()),
        "~~word~~"
    );
    assert_eq!(press("word", 0, 4, command(Key::K)), "[word]()");
}

#[test]
fn headings_quotes_code_and_lists_have_keys_of_their_own() {
    let command = Shortcut::command;
    assert_eq!(press("line", 0, 0, command(Key::Num1).with_alt()), "# line");
    assert_eq!(
        press("line", 0, 0, command(Key::Num2).with_alt()),
        "## line"
    );
    assert_eq!(
        press("line", 0, 0, command(Key::Num3).with_alt()),
        "### line"
    );
    assert_eq!(
        press("## line", 0, 0, command(Key::Num0).with_alt()),
        "line"
    );
    assert_eq!(press("line", 0, 0, command(Key::Q).with_alt()), "> line");
    assert_eq!(press("word", 0, 4, command(Key::C).with_shift()), "`word`");
    assert_eq!(
        press("line", 0, 0, command(Key::Num8).with_shift()),
        "- line"
    );
    assert_eq!(
        press("line", 0, 0, command(Key::Num7).with_shift()),
        "1. line"
    );
    assert_eq!(
        press("line", 0, 0, command(Key::Num9).with_shift()),
        "- [ ] line"
    );
}

#[test]
fn mod_enter_tab_and_shift_tab_edit_tasks_and_lists() {
    assert_eq!(
        press("- [ ] task", 0, 0, Shortcut::command(Key::Enter)),
        "- [x] task"
    );
    assert_eq!(
        press("- a\n- b", 5, 5, Shortcut::new(Key::Tab)),
        "- a\n  - b"
    );
    assert_eq!(
        press("- a\n  - b", 7, 7, Shortcut::new(Key::Tab).with_shift()),
        "- a\n- b"
    );
}

#[test]
fn undo_redo_and_find_have_the_platform_keys() {
    let command = Shortcut::command;
    for apple in [true, false] {
        assert_eq!(
            keymap::command_for(command(Key::Z), apple),
            Some(Command::Undo)
        );
        assert_eq!(
            keymap::command_for(command(Key::Z).with_shift(), apple),
            Some(Command::Redo)
        );
        assert_eq!(
            keymap::command_for(command(Key::F), apple),
            Some(Command::Find)
        );
    }
    // Ctrl-Y redoes on Windows and Linux only.
    assert_eq!(
        keymap::command_for(command(Key::Y), false),
        Some(Command::Redo)
    );
    assert_eq!(keymap::command_for(command(Key::Y), true), None);
    assert_eq!(Command::Find.event(), None);
}

#[test]
fn every_shortcut_runs_one_command_and_every_format_has_a_key() {
    for apple in [true, false] {
        let mut seen = HashSet::new();
        for (command, shortcuts) in keymap::keymap(apple) {
            assert!(!shortcuts.is_empty(), "{command:?} has no key");
            for shortcut in shortcuts {
                assert!(seen.insert(shortcut), "{shortcut:?} runs two commands");
            }
        }
    }
    for format in [
        Format::Bold,
        Format::Italic,
        Format::Underline,
        Format::Strikethrough,
        Format::Code,
        Format::Link,
        Format::BulletList,
        Format::NumberedList,
        Format::CheckList,
        Format::BlockQuote,
        Format::Heading(0),
        Format::Heading(1),
        Format::Heading(2),
        Format::Heading(3),
    ] {
        assert!(
            !keymap::shortcuts_of(Command::Format(format), false).is_empty(),
            "{format:?}"
        );
    }
}
