//! Seam 1: notes open in several windows. Quit and a switch to another
//! workspace close every window, so they wait for the notes of each one,
//! and the window whose note couldn't save asks. Closing one window waits
//! for its own notes and leaves the others open.

mod support;

use std::time::{Duration, Instant};

use app_core::{App, CommandSource, TabId, command_id};
use support::{Fixture, hold_writes, note, open_from_tree, settle};

const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";
const B: &str = "01jbcdefghjkmnpqrstvwxyz0b";

/// Type `added` at the end of the note in `tab`.
fn type_into(app: &mut App, tab: TabId, added: &str) {
    let text = format!("{}{added}", app.note(tab).unwrap().text());
    app.edit_note(tab, 1, &text, Instant::now());
}

/// Poll every window until no save runs, then a few times more, so each
/// window hears what the others did.
fn settle_windows(windows: &mut [&mut App]) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        for window in windows.iter_mut() {
            window.poll();
        }
        if windows.iter().all(|window| !window.is_saving_notes()) {
            break;
        }
        assert!(Instant::now() < deadline, "a note save never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..3 {
        for window in windows.iter_mut() {
            window.poll();
        }
    }
}

/// Two windows on a workspace with Plan.md and Docs/Other.md, the first
/// showing Plan and the second Other.
fn two_windows(fixture: &Fixture) -> (App, TabId, App, TabId) {
    let mut first = fixture.app();
    let mut second = first.new_window().expect("a second window opens");
    let plan = open_from_tree(&mut first, "Plan.md");
    let other = open_from_tree(&mut second, "Docs/Other.md");
    (first, plan, second, other)
}

fn fixture() -> Fixture {
    Fixture::new(&[
        ("Plan.md", &note(A, "Plan\n")),
        ("Docs/Other.md", &note(B, "Other\n")),
    ])
}

#[test]
fn quitting_waits_for_the_notes_of_every_window() {
    let fixture = fixture();
    let (mut first, plan, mut second, other) = two_windows(&fixture);
    type_into(&mut first, plan, "typed here\n");
    type_into(&mut second, other, "typed there\n");
    let lock = hold_writes(&fixture, &first);

    assert!(first.run_command(command_id::QUIT, CommandSource::Key, Instant::now()));
    std::thread::sleep(Duration::from_millis(200));
    for _ in 0..3 {
        first.poll();
        second.poll();
    }

    assert!(!first.take_quit(), "Quit waits for the saves");
    assert!(first.is_leaving() && second.is_leaving());
    assert!(
        second.is_saving_notes(),
        "the other window saves its note too"
    );
    lock.unlock().unwrap();
    settle_windows(&mut [&mut first, &mut second]);

    assert!(first.take_quit());
    assert!(
        !second.take_quit(),
        "the window that ran Quit carries it out"
    );
    assert_eq!(fixture.read("Plan.md"), note(A, "Plan\ntyped here\n"));
    assert_eq!(
        fixture.read("Docs/Other.md"),
        note(B, "Other\ntyped there\n")
    );
}

#[test]
fn quitting_with_nothing_to_save_in_either_window_quits_at_once() {
    let fixture = fixture();
    let (mut first, _, mut second, _) = two_windows(&fixture);

    second.request_quit();
    first.poll();
    second.poll();

    assert!(second.take_quit());
    first.poll();
    assert!(!first.is_leaving());
}

#[test]
fn switching_workspace_saves_every_window_s_notes_first() {
    let fixture = fixture();
    let other_folder = tempfile::tempdir().unwrap();
    let (mut first, _, mut second, other) = two_windows(&fixture);
    let shown = first.workspace().unwrap().id().to_owned();
    type_into(&mut second, other, "typed there\n");
    let lock = hold_writes(&fixture, &first);

    first.open_folder(other_folder.path().to_owned());
    std::thread::sleep(Duration::from_millis(200));
    for _ in 0..3 {
        first.poll();
        second.poll();
    }

    assert!(
        !first.is_busy(),
        "the other workspace waits for the second window's note"
    );
    lock.unlock().unwrap();
    settle_windows(&mut [&mut first, &mut second]);
    settle(&mut first);

    assert_ne!(first.workspace().unwrap().id(), shown);
    second.poll();
    assert!(
        second.is_closed(),
        "the other window closes with the switch"
    );
    assert_eq!(
        fixture.read("Docs/Other.md"),
        note(B, "Other\ntyped there\n")
    );
}

/// Saves fail with a real permission error while a note's folder is
/// read-only.
#[cfg(unix)]
mod failures {
    use std::os::unix::fs::PermissionsExt;

    use app_core::SAVE_PAUSE;

    use super::*;

    fn read_only(fixture: &Fixture, folder: &str, read_only: bool) {
        let mode = if read_only { 0o555 } else { 0o755 };
        std::fs::set_permissions(
            fixture.root.join(folder),
            std::fs::Permissions::from_mode(mode),
        )
        .unwrap();
    }

    /// The second window's note can't save: its folder is read-only.
    fn failing_in_second(fixture: &Fixture) -> (App, TabId, App, TabId) {
        let (first, plan, mut second, other) = two_windows(fixture);
        read_only(fixture, "Docs", true);
        type_into(&mut second, other, "typed there\n");
        (first, plan, second, other)
    }

    #[test]
    fn the_window_whose_note_cant_save_asks_and_keep_editing_stops_the_quit() {
        let fixture = fixture();
        let (mut first, _, mut second, other) = failing_in_second(&fixture);

        assert!(first.run_command(command_id::QUIT, CommandSource::Key, Instant::now()));
        settle_windows(&mut [&mut first, &mut second]);

        assert!(!first.take_quit(), "nothing closes until you choose");
        assert!(first.unsaved_prompt().is_none());
        let prompt = second
            .unsaved_prompt()
            .expect("the window with the note asks");
        assert_eq!(prompt.title, "Couldn\u{2019}t save \u{201c}Other\u{201d}");

        second.keep_editing();
        settle_windows(&mut [&mut first, &mut second]);

        assert!(!first.take_quit());
        assert!(!first.is_leaving() && !second.is_leaving());
        assert_eq!(second.active_tab().map(|tab| tab.id()), Some(other));
        assert!(second.note(other).unwrap().is_unsaved());
        read_only(&fixture, "Docs", false);
    }

    #[test]
    fn discard_changes_in_the_window_that_asks_lets_the_quit_go_ahead() {
        let fixture = fixture();
        let (mut first, _, mut second, _) = failing_in_second(&fixture);
        first.request_quit();
        settle_windows(&mut [&mut first, &mut second]);
        assert!(second.unsaved_prompt().is_some());

        second.discard_and_leave();
        settle_windows(&mut [&mut first, &mut second]);

        assert!(first.take_quit());
        read_only(&fixture, "Docs", false);
        assert_eq!(fixture.read("Docs/Other.md"), note(B, "Other\n"));
    }

    #[test]
    fn typing_in_a_window_that_was_ready_saves_before_the_quit_goes_ahead() {
        let fixture = fixture();
        let (mut first, plan, mut second, _) = failing_in_second(&fixture);
        first.request_quit();
        settle_windows(&mut [&mut first, &mut second]);
        assert!(second.unsaved_prompt().is_some());

        // The first window was ready; typing there while the second asks
        // has to save before Noura quits.
        let lock = hold_writes(&fixture, &first);
        type_into(&mut first, plan, "typed meanwhile\n");
        second.discard_and_leave();
        std::thread::sleep(Duration::from_millis(200));
        for _ in 0..3 {
            first.poll();
            second.poll();
        }
        assert!(!first.take_quit(), "the new typing saves first");

        lock.unlock().unwrap();
        settle_windows(&mut [&mut first, &mut second]);
        assert!(first.take_quit());
        assert_eq!(fixture.read("Plan.md"), note(A, "Plan\ntyped meanwhile\n"));
        read_only(&fixture, "Docs", false);
    }

    #[test]
    fn closing_another_window_waits_for_its_notes_and_leaves_the_others_open() {
        let fixture = fixture();
        let (mut first, _, mut second, other) = failing_in_second(&fixture);
        second.save_due_notes(Instant::now() + SAVE_PAUSE);
        settle_windows(&mut [&mut first, &mut second]);
        assert!(second.note(other).unwrap().save_problem().is_some());

        assert!(second.run_command(command_id::CLOSE_WINDOW, CommandSource::Key, Instant::now()));
        settle_windows(&mut [&mut first, &mut second]);

        assert!(!second.is_closed(), "the window waits for an answer");
        assert!(second.unsaved_prompt().is_some());
        assert!(first.unsaved_prompt().is_none());

        read_only(&fixture, "Docs", false);
        second.retry_leaving();
        settle_windows(&mut [&mut first, &mut second]);

        assert!(second.is_closed());
        assert!(!first.is_closed(), "the other window stays");
        assert!(!first.take_quit(), "and Noura keeps running");
        assert_eq!(
            fixture.read("Docs/Other.md"),
            note(B, "Other\ntyped there\n")
        );
    }
}
