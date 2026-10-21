//! Seam 1: the command registry, the default keymap and how key presses
//! and menu choices reach commands.

mod support;

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use app_core::{
    App, Area, Command, CommandSource, FILES_PAGE, INBOX_PAGE, Key, KeyPress, Platform, Shortcut,
    command_id, default_shortcuts,
};
use support::{FakeSystem, Fixture};

/// A command that counts its runs.
fn counting(id: &str, label: &str) -> (Command, Arc<AtomicUsize>) {
    let runs = Arc::new(AtomicUsize::new(0));
    (Command::new(id, label, Area::Anywhere), runs)
}

fn register_counting(
    app: &mut app_core::App,
    command: Command,
    runs: &Arc<AtomicUsize>,
) -> Result<(), app_core::AppError> {
    let runs = runs.clone();
    app.register_command(command, move |_| {
        runs.fetch_add(1, Ordering::SeqCst);
    })
}

#[test]
fn each_command_has_an_id_a_label_an_area_and_its_default_shortcut() {
    let fixture = Fixture::new(&[]);
    let app = fixture.app();

    let commands = app.commands();
    let new_note = commands
        .iter()
        .find(|command| command.id == command_id::NEW_NOTE)
        .expect("New Note is registered");

    assert_eq!(new_note.label, "New Note");
    assert_eq!(new_note.area, Area::Anywhere);
    assert_eq!(new_note.shortcuts, [Shortcut::command(Key::N)]);
    let palette = commands
        .iter()
        .find(|command| command.id == command_id::COMMAND_PALETTE)
        .expect("the command palette is registered");
    assert_eq!(palette.shortcuts, [Shortcut::command(Key::K)]);
}

#[test]
fn registering_a_second_command_with_a_taken_id_fails() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let (first, runs) = counting("test.count", "Count");
    register_counting(&mut app, first, &runs).unwrap();

    let (again, _) = counting("test.count", "Count again");
    let error = register_counting(&mut app, again, &runs).unwrap_err();
    assert_eq!(error.code, "command_exists");

    let (kernel, _) = counting(command_id::NEW_NOTE, "Another note");
    let error = register_counting(&mut app, kernel, &runs).unwrap_err();
    assert_eq!(error.code, "command_exists");
    let labels: Vec<_> = app
        .commands()
        .into_iter()
        .filter(|command| command.id == "test.count" || command.id == command_id::NEW_NOTE)
        .map(|command| command.label)
        .collect();
    assert_eq!(labels, ["New Note", "Count"]);
}

#[test]
fn registering_a_command_with_a_taken_shortcut_fails() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();

    // Mod-N belongs to New Note.
    let (command, runs) = counting("test.count", "Count");
    let taken = command.with_shortcuts([Shortcut::command(Key::N)]);
    assert_eq!(
        register_counting(&mut app, taken, &runs).unwrap_err().code,
        "shortcut_taken"
    );

    // Mod-O belongs to Quick Open.
    let (command, runs) = counting("test.count", "Count");
    let reserved = command.with_shortcuts([Shortcut::command(Key::O)]);
    assert_eq!(
        register_counting(&mut app, reserved, &runs)
            .unwrap_err()
            .code,
        "shortcut_taken"
    );

    // Mod-Tab and Ctrl-Tab are one key press on Windows and Linux.
    let (command, runs) = counting("test.count", "Count");
    let same_press = command.with_shortcuts([Shortcut::command(Key::Tab)]);
    assert_eq!(
        register_counting(&mut app, same_press, &runs)
            .unwrap_err()
            .code,
        "shortcut_taken"
    );

    // The Edit menu's keys, Quit's and Find's (tickets 19 and 40) stay
    // theirs.
    for key in [Key::Z, Key::X, Key::C, Key::V, Key::A, Key::Q, Key::F] {
        let (command, runs) = counting("test.count", "Count");
        let edit = command.with_shortcuts([Shortcut::command(key)]);
        assert_eq!(
            register_counting(&mut app, edit, &runs).unwrap_err().code,
            "shortcut_taken",
            "Mod-{key:?}"
        );
    }

    let (command, runs) = counting("test.count", "Count");
    let free = command.with_shortcuts([Shortcut::command(Key::J)]);
    register_counting(&mut app, free, &runs).unwrap();
    assert!(
        app.commands()
            .iter()
            .any(|command| command.id == "test.count")
    );
}

#[test]
fn no_two_commands_share_an_id_or_a_shortcut() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    for (id, title) in [("tasks", "Tasks"), ("calendar", "Calendar")] {
        app.register_page(id, title);
    }

    let commands = app.commands();
    for (index, command) in commands.iter().enumerate() {
        for other in &commands[index + 1..] {
            assert_ne!(command.id, other.id, "two commands share an ID");
            for shortcut in &command.shortcuts {
                for theirs in &other.shortcuts {
                    assert!(
                        !shortcut.conflicts_with(theirs),
                        "{} and {} share {shortcut:?}",
                        command.label,
                        other.label
                    );
                }
            }
        }
    }
}

/// The command a key press runs, outside any text field.
fn command_for(app: &App, press: KeyPress) -> Option<String> {
    app.command_for_key(&press, None)
}

/// Mod and `key` on `platform`: Command on macOS, Ctrl elsewhere.
fn with_mod(platform: Platform, key: Key) -> KeyPress {
    match platform {
        Platform::MacOs => KeyPress::new(key).with_cmd(),
        Platform::Windows | Platform::Linux => KeyPress::new(key).with_ctrl(),
    }
}

#[test]
fn the_default_keymap_keeps_today_s_keys() {
    use command_id::*;
    for platform in [Platform::MacOs, Platform::Windows, Platform::Linux] {
        let fixture = Fixture::new(&[]);
        let app = fixture.app_with(FakeSystem::on(platform));
        let shift = |press: KeyPress| press.with_shift();
        let cases = [
            (with_mod(platform, Key::N), NEW_NOTE),
            (shift(with_mod(platform, Key::N)), NEW_FOLDER),
            (with_mod(platform, Key::K), COMMAND_PALETTE),
            (KeyPress::new(Key::F2), RENAME_FILE),
            (with_mod(platform, Key::W), CLOSE_TAB),
            (shift(with_mod(platform, Key::W)), CLOSE_WINDOW),
            (KeyPress::new(Key::Tab).with_ctrl(), NEXT_TAB),
            (shift(with_mod(platform, Key::CloseBracket)), NEXT_TAB),
            (shift(KeyPress::new(Key::Tab).with_ctrl()), PREVIOUS_TAB),
            (shift(with_mod(platform, Key::OpenBracket)), PREVIOUS_TAB),
            (with_mod(platform, Key::Backslash), TOGGLE_SIDEBAR),
            (with_mod(platform, Key::Q), QUIT),
        ];
        for (press, id) in cases {
            assert_eq!(
                command_for(&app, press).as_deref(),
                Some(id),
                "{press:?} on {platform:?}"
            );
        }
    }

    // Features that register later get their keys from the keymap.
    let shortcut = Shortcut::command;
    assert_eq!(default_shortcuts(QUICK_OPEN), [shortcut(Key::O)]);
    assert_eq!(default_shortcuts(SEARCH), [shortcut(Key::F).with_shift()]);
    assert_eq!(
        default_shortcuts(ZOOM_IN),
        [shortcut(Key::Plus), shortcut(Key::Equals)]
    );
    assert_eq!(default_shortcuts(ZOOM_OUT), [shortcut(Key::Minus)]);
    assert_eq!(default_shortcuts(ZOOM_RESET), [shortcut(Key::Num0)]);
    assert_eq!(default_shortcuts(SETTINGS), [shortcut(Key::Comma)]);
    // Find belongs to the open note or PDF, not to the app.
    let fixture = Fixture::new(&[]);
    let app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    assert_eq!(command_for(&app, KeyPress::new(Key::F).with_cmd()), None);
}

#[test]
fn mod_1_shows_the_inbox_mod_2_files_and_mod_3_to_9_the_other_pages_in_rail_order() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    let plugins = [
        "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight",
    ];
    // The Inbox is the kernel's page, so it has Mod-1 without registering.
    for title in plugins {
        app.register_page(&title.to_lowercase(), title);
    }

    let shown_by =
        |app: &App, digit: usize| command_for(app, KeyPress::new(Key::DIGITS[digit]).with_cmd());
    let go_to = |page: &str| Some(format!("{}{page}", command_id::GO_TO_PAGE));
    assert_eq!(shown_by(&app, 1), go_to(INBOX_PAGE));
    assert_eq!(shown_by(&app, 2), go_to(FILES_PAGE));
    assert_eq!(shown_by(&app, 3), go_to("one"));
    assert_eq!(shown_by(&app, 9), go_to("seven"));
    // The eighth plugin page has no key left, but the palette lists it.
    let eight = app
        .commands()
        .into_iter()
        .find(|command| command.label == "Go to Eight")
        .unwrap();
    assert!(eight.shortcuts.is_empty());

    // The keys follow the rail's order, except the Inbox's and Files'.
    app.move_page("seven", 0);
    assert_eq!(shown_by(&app, 3), go_to("seven"));
    assert_eq!(shown_by(&app, 4), go_to("one"));
    assert_eq!(shown_by(&app, 1), go_to(INBOX_PAGE));

    let two = go_to("two").unwrap();
    assert!(app.run_command(&two, CommandSource::Key, Instant::now()));
    assert_eq!(app.current_page(), "two");
}

#[test]
fn mod_is_command_on_macos_and_ctrl_on_windows_and_linux() {
    let id = Some(command_id::NEW_NOTE.to_owned());

    let fixture = Fixture::new(&[]);
    let mac = fixture.app_with(FakeSystem::on(Platform::MacOs));
    assert_eq!(command_for(&mac, KeyPress::new(Key::N).with_cmd()), id);
    assert_eq!(command_for(&mac, KeyPress::new(Key::N).with_ctrl()), None);
    // Ctrl-Tab is Control on a Mac too.
    assert_eq!(
        command_for(&mac, KeyPress::new(Key::Tab).with_cmd()),
        None,
        "Cmd-Tab belongs to macOS"
    );
    drop(mac);

    for platform in [Platform::Windows, Platform::Linux] {
        let fixture = Fixture::new(&[]);
        let app = fixture.app_with(FakeSystem::on(platform));
        assert_eq!(command_for(&app, KeyPress::new(Key::N).with_ctrl()), id);
        assert_eq!(command_for(&app, KeyPress::new(Key::N).with_cmd()), None);
        assert_eq!(command_for(&app, KeyPress::new(Key::N)), None);
        assert_eq!(
            command_for(&app, KeyPress::new(Key::N).with_ctrl().with_alt()),
            None
        );
    }
}

#[test]
fn zoom_follows_the_printed_character_so_equals_and_plus_both_zoom_in() {
    let fixture = Fixture::new(&[]);
    let app = fixture.app_with(FakeSystem::on(Platform::MacOs));

    let zoom_in = Some(command_id::ZOOM_IN.to_owned());
    let cmd = |key| KeyPress::new(key).with_cmd();
    // US layout: = unshifted, + with Shift on the same key.
    assert_eq!(command_for(&app, cmd(Key::Equals)), zoom_in);
    assert_eq!(command_for(&app, cmd(Key::Plus).with_shift()), zoom_in);
    // A layout with its own + key.
    assert_eq!(command_for(&app, cmd(Key::Plus)), zoom_in);
    assert_eq!(
        command_for(&app, cmd(Key::Minus)).as_deref(),
        Some(command_id::ZOOM_OUT)
    );
    assert_eq!(
        command_for(&app, cmd(Key::Num0)).as_deref(),
        Some(command_id::ZOOM_RESET)
    );
}

#[test]
fn brackets_and_backslash_also_match_by_their_place_on_a_us_keyboard() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    let (command, runs) = counting("test.count", "Count");
    let command = command.with_shortcuts([Shortcut::command(Key::J).with_shift()]);
    register_counting(&mut app, command, &runs).unwrap();
    // A layout that prints another character on the key right of P.
    let press = KeyPress::new(Key::Period)
        .with_cmd()
        .with_shift()
        .at(Key::CloseBracket);
    assert_eq!(
        command_for(&app, press).as_deref(),
        Some(command_id::NEXT_TAB)
    );
    // The printed character wins over the key's place.
    let press = KeyPress::new(Key::J)
        .with_cmd()
        .with_shift()
        .at(Key::CloseBracket);
    assert_eq!(command_for(&app, press).as_deref(), Some("test.count"));
}

#[test]
fn keys_the_focused_text_field_types_or_edits_with_run_no_command() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::Windows));
    let (command, runs) = counting("test.erase", "Erase");
    let command = command.with_shortcuts([Shortcut::new(Key::Backspace).with_alt()]);
    register_counting(&mut app, command, &runs).unwrap();

    let in_field = |app: &App, press: KeyPress| app.command_for_key(&press, Some(Area::Writing));
    // Outside a text field the test command takes Alt-Backspace; inside
    // one, Alt-Backspace deletes a word.
    let erase = KeyPress::new(Key::Backspace).with_alt();
    assert_eq!(command_for(&app, erase).as_deref(), Some("test.erase"));
    assert_eq!(in_field(&app, erase), None);
    assert_eq!(in_field(&app, KeyPress::new(Key::A).with_ctrl()), None);
    // F2 renames the open file, except while you type in a field.
    assert_eq!(in_field(&app, KeyPress::new(Key::F2)), None);
    assert_eq!(
        in_field(&app, KeyPress::new(Key::Backspace).with_ctrl()),
        None
    );
    // Shortcuts no text field uses still run.
    assert_eq!(
        in_field(&app, KeyPress::new(Key::N).with_ctrl()).as_deref(),
        Some(command_id::NEW_NOTE)
    );
    assert_eq!(
        in_field(&app, KeyPress::new(Key::K).with_ctrl()).as_deref(),
        Some(command_id::COMMAND_PALETTE)
    );
    assert_eq!(
        in_field(&app, KeyPress::new(Key::Tab).with_ctrl()).as_deref(),
        Some(command_id::NEXT_TAB)
    );

    let at = Instant::now();
    assert!(!app.press_key(&erase, Some(Area::Writing), at));
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    assert!(app.press_key(&erase, None, at));
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[test]
fn a_command_for_one_area_runs_only_while_that_area_has_the_keyboard() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    let runs = Arc::new(AtomicUsize::new(0));
    let command = Command::new("test.sort", "Sort the tree", Area::FileTree)
        .with_shortcuts([Shortcut::command(Key::J)]);
    register_counting(&mut app, command, &runs).unwrap();
    let press = KeyPress::new(Key::J).with_cmd();

    assert_eq!(app.command_for_key(&press, None), None);
    assert_eq!(app.command_for_key(&press, Some(Area::Tabs)), None);
    assert_eq!(
        app.command_for_key(&press, Some(Area::FileTree)).as_deref(),
        Some("test.sort")
    );
    // Commands for anywhere run in every area.
    let new_note = KeyPress::new(Key::N).with_cmd();
    assert_eq!(
        app.command_for_key(&new_note, Some(Area::FileTree))
            .as_deref(),
        Some(command_id::NEW_NOTE)
    );
}

#[test]
fn keys_an_area_handles_itself_stay_with_it_while_it_has_the_keyboard() {
    let fixture = Fixture::new(&[]);
    let app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    let f2 = KeyPress::new(Key::F2);
    let rename_file = Some(command_id::RENAME_FILE.to_owned());

    // In the tree, F2 renames the current row, not the open file.
    assert_eq!(app.command_for_key(&f2, Some(Area::FileTree)), None);
    assert_eq!(app.command_for_key(&f2, Some(Area::Tabs)), rename_file);
    assert_eq!(app.command_for_key(&f2, None), rename_file);
    // Alt-Up moves a rail page; nothing else uses it.
    let alt_up = KeyPress::new(Key::ArrowUp).with_alt();
    assert_eq!(app.command_for_key(&alt_up, Some(Area::Pages)), None);
    // The tree's keys don't reach past it.
    let new_note = KeyPress::new(Key::N).with_cmd();
    assert_eq!(
        app.command_for_key(&new_note, Some(Area::FileTree))
            .as_deref(),
        Some(command_id::NEW_NOTE)
    );
}

#[test]
fn a_press_the_menu_and_the_window_both_report_runs_once_and_two_quick_presses_run_twice() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    let (command, runs) = counting("test.count", "Count");
    let command = command.with_shortcuts([Shortcut::command(Key::J)]);
    register_counting(&mut app, command, &runs).unwrap();
    let press = KeyPress::new(Key::J).with_cmd();
    let start = Instant::now();
    let after = |millis| start + Duration::from_millis(millis);
    let count = || runs.load(Ordering::SeqCst);

    // One press, reported by the window and then by the menu.
    assert!(app.press_key(&press, None, after(0)));
    assert!(!app.run_command("test.count", CommandSource::Menu, after(10)));
    assert_eq!(count(), 1);

    // Two quick presses, each reported once.
    assert!(app.press_key(&press, None, after(100)));
    assert!(app.press_key(&press, None, after(150)));
    assert_eq!(count(), 3);

    // The menu reports first this time.
    assert!(app.run_command("test.count", CommandSource::Menu, after(500)));
    assert!(app.press_key(&press, None, after(520)));
    assert_eq!(count(), 4, "the window's report of the same press");
    assert!(app.run_command("test.count", CommandSource::Menu, after(560)));
    assert_eq!(count(), 5);

    // Long after, the window's report is a new press.
    assert!(app.press_key(&press, None, after(900)));
    assert_eq!(count(), 6);
    // The palette never counts as a second report.
    assert!(app.run_command("test.count", CommandSource::Palette, after(905)));
    assert_eq!(count(), 7);
}

#[test]
fn the_shortcuts_list_groups_every_shortcut_by_area() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    let (command, runs) = counting("test.count", "Count");
    let command = command.with_shortcuts([Shortcut::command(Key::J)]);
    register_counting(&mut app, command, &runs).unwrap();
    let local = Command::new("test.tree", "Sort the tree", Area::FileTree)
        .with_shortcuts([Shortcut::command(Key::J).with_alt()]);
    app.register_command(local, |_| {}).unwrap();

    let groups = app.shortcut_groups();
    let areas: Vec<Area> = groups.iter().map(|group| group.area).collect();
    assert_eq!(
        areas,
        [
            Area::Anywhere,
            Area::FileTree,
            Area::Tabs,
            Area::Pages,
            Area::Writing
        ]
    );
    let entry = |area: Area, label: &str| {
        groups
            .iter()
            .find(|group| group.area == area)
            .and_then(|group| group.entries.iter().find(|entry| entry.label == label))
            .map(|entry| entry.shortcuts.clone())
    };
    assert_eq!(
        entry(Area::Anywhere, "New Note"),
        Some(vec![Shortcut::command(Key::N)])
    );
    assert_eq!(
        entry(Area::Anywhere, "Next Tab"),
        Some(vec![
            Shortcut::command(Key::CloseBracket).with_shift(),
            Shortcut::ctrl(Key::Tab)
        ])
    );
    assert_eq!(
        entry(Area::Anywhere, "Count"),
        Some(vec![Shortcut::command(Key::J)])
    );
    assert_eq!(
        entry(Area::Anywhere, "Close a dialog"),
        Some(vec![Shortcut::new(Key::Escape)])
    );
    // Keys that work only while one area has the keyboard.
    assert_eq!(
        entry(Area::FileTree, "Rename"),
        Some(vec![Shortcut::new(Key::F2)])
    );
    assert_eq!(
        entry(Area::FileTree, "Sort the tree"),
        Some(vec![Shortcut::command(Key::J).with_alt()])
    );
    assert_eq!(
        entry(Area::Tabs, "Close the focused tab"),
        Some(vec![
            Shortcut::new(Key::Delete),
            Shortcut::new(Key::Backspace)
        ])
    );
    assert_eq!(
        entry(Area::Pages, "Move a page up or down"),
        Some(vec![
            Shortcut::new(Key::ArrowUp).with_alt(),
            Shortcut::new(Key::ArrowDown).with_alt()
        ])
    );
    assert_eq!(
        entry(Area::Anywhere, "Quit Noura"),
        Some(vec![Shortcut::command(Key::Q)])
    );
    assert_eq!(
        entry(Area::FileTree, "Open a file"),
        Some(vec![Shortcut::new(Key::Enter), Shortcut::new(Key::Space)])
    );
    assert_eq!(
        entry(Area::Tabs, "Show the focused tab"),
        Some(vec![Shortcut::new(Key::Enter), Shortcut::new(Key::Space)])
    );
    assert_eq!(
        entry(Area::Pages, "Open a page"),
        Some(vec![Shortcut::new(Key::Enter), Shortcut::new(Key::Space)])
    );
    assert_eq!(
        entry(Area::Writing, "Undo"),
        Some(vec![Shortcut::command(Key::Z)])
    );
    // The Edit menu's keys act on the focused text field.
    for (label, key) in [
        ("Cut", Key::X),
        ("Copy", Key::C),
        ("Paste", Key::V),
        ("Select All", Key::A),
    ] {
        assert_eq!(
            entry(Area::Writing, label),
            Some(vec![Shortcut::command(key)])
        );
    }
    assert_eq!(
        entry(Area::Writing, "Redo"),
        Some(vec![Shortcut::command(Key::Z).with_shift()])
    );
    // Commands without keys stay off the list.
    assert_eq!(entry(Area::Anywhere, "Keyboard Shortcuts"), None);
    drop(app);

    // Outside macOS, Quit is Exit on Windows, and Ctrl-Y also redoes.
    for (platform, quit) in [(Platform::Windows, "Exit"), (Platform::Linux, "Quit")] {
        let fixture = Fixture::new(&[]);
        let app = fixture.app_with(FakeSystem::on(platform));
        let groups = app.shortcut_groups();
        let entry = |area: Area, label: &str| {
            groups
                .iter()
                .find(|group| group.area == area)
                .and_then(|group| group.entries.iter().find(|entry| entry.label == label))
                .map(|entry| entry.shortcuts.clone())
        };
        assert_eq!(
            entry(Area::Anywhere, quit),
            Some(vec![Shortcut::command(Key::Q)]),
            "{platform:?}"
        );
        assert_eq!(
            entry(Area::Writing, "Redo"),
            Some(vec![
                Shortcut::command(Key::Z).with_shift(),
                Shortcut::command(Key::Y)
            ]),
            "{platform:?}"
        );
    }
}

#[test]
fn keys_the_tree_tabs_and_pages_open_things_with_stay_with_them() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    // A command on a plain key runs only where no area uses that key.
    let (command, runs) = counting("test.count", "Count");
    let command = command.with_shortcuts([Shortcut::new(Key::Space), Shortcut::new(Key::Enter)]);
    register_counting(&mut app, command, &runs).unwrap();
    for key in [Key::Space, Key::Enter] {
        let press = KeyPress::new(key);
        for area in [Area::FileTree, Area::Tabs, Area::Pages, Area::Writing] {
            assert_eq!(
                app.command_for_key(&press, Some(area)),
                None,
                "{key:?} in {area:?}"
            );
        }
        assert_eq!(
            app.command_for_key(&press, None).as_deref(),
            Some("test.count")
        );
    }
}
