//! Seam 1: another app edits, deletes or moves a note while it is open in
//! the editor. Edits of other text merge with your typing, edits of the
//! same text open conflict review, and a deleted note can come back.

mod support;

use std::{
    sync::mpsc::{Receiver, channel},
    time::Instant,
};

use app_core::{App, Change, ConflictChoice, SAVE_PAUSE, TabId};
use support::{Fixture, note, open_from_tree, settle_saves};

const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";

/// Every change `app` announces from now on.
fn changes(app: &App) -> Receiver<Change> {
    let (sender, changes) = channel();
    app.subscribe(move |change| {
        let _ = sender.send(change);
    });
    changes
}

/// How many notices `changes` announced so far.
fn notices(changes: &Receiver<Change>) -> usize {
    changes
        .try_iter()
        .filter(|change| *change == Change::Notice)
        .count()
}

/// The editor's text after `change`, as the editor reports it.
fn type_into(app: &mut App, tab: TabId, change: impl FnOnce(&str) -> String) -> Instant {
    let at = Instant::now();
    let text = change(app.note(tab).unwrap().text());
    app.edit_note(tab, 1, &text, at);
    at
}

/// Let the save typing at `at` started run to its end.
fn save(app: &mut App, at: Instant) {
    app.save_due_notes(at + SAVE_PAUSE);
    settle_saves(app);
}

#[test]
fn an_outside_edit_to_a_note_with_nothing_unsaved_shows_without_a_notice() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "one\ntwo\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let changes = changes(&app);

    let outside = note(A, "one\ntwo, outside\n");
    support::write(&fixture.root, "Plan.md", &outside);
    app.rescan();
    settle_saves(&mut app);

    let session = app.note(tab).unwrap();
    assert_eq!(session.text(), "\none\ntwo, outside\n");
    assert!(!session.is_unsaved());
    assert_eq!(
        app.take_note_reset(tab).as_deref(),
        Some("\none\ntwo, outside\n"),
        "the editor takes the file's text"
    );
    assert_eq!(notices(&changes), 0);
    assert_eq!(fixture.read("Plan.md"), outside, "Noura writes nothing");
}

#[test]
fn an_outside_edit_of_other_text_merges_with_your_typing_and_says_so_once() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "one\ntwo\nthree\nfour\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let changes = changes(&app);
    let at = type_into(&mut app, tab, |text| text.replace("one", "one, typed"));
    support::write(
        &fixture.root,
        "Plan.md",
        &note(A, "one\ntwo\nthree\nfour, outside\n"),
    );
    app.rescan();
    assert_eq!(
        fixture.read("Plan.md"),
        note(A, "one\ntwo\nthree\nfour, outside\n")
    );

    // The save after the typing pauses merges. It waits for the disk, and
    // you keep typing meanwhile.
    let lock = support::hold_writes(&fixture, &app);
    app.save_due_notes(at + SAVE_PAUSE);
    assert!(app.note(tab).unwrap().is_saving());
    type_into(&mut app, tab, |text| {
        text.replace("two", "two, typed later")
    });
    lock.unlock().unwrap();
    settle_saves(&mut app);

    assert_eq!(
        fixture.read("Plan.md"),
        note(A, "one, typed\ntwo\nthree\nfour, outside\n")
    );
    let session = app.note(tab).unwrap();
    assert_eq!(
        session.text(),
        "\none, typed\ntwo, typed later\nthree\nfour, outside\n",
        "the note shows both edits and keeps the keystrokes typed during the merge"
    );
    let sync = app.take_note_sync(tab).expect("the editor catches up");
    assert_eq!(
        sync.merged.as_deref(),
        Some("\none, typed\ntwo\nthree\nfour, outside\n")
    );
    assert_eq!(notices(&changes), 1);
    assert_eq!(
        app.notice().unwrap().title,
        "Merged changes from another app"
    );

    // The typing from during the merge saves on top, without a notice.
    save(&mut app, Instant::now());
    assert_eq!(
        fixture.read("Plan.md"),
        note(A, "one, typed\ntwo, typed later\nthree\nfour, outside\n")
    );
    assert_eq!(notices(&changes), 0);
}

/// `Plan.md` open with "one" typed over by you and by another app.
fn in_conflict(fixture: &Fixture) -> (App, TabId) {
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let at = type_into(&mut app, tab, |text| text.replace("one", "one, mine"));
    support::write(&fixture.root, "Plan.md", &note(A, "one, theirs\ntwo\n"));
    app.rescan();
    save(&mut app, at);
    (app, tab)
}

#[test]
fn an_outside_edit_of_the_text_you_typed_pauses_saves_and_opens_conflict_review() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "one\ntwo\n"))]);
    let (mut app, tab) = in_conflict(&fixture);

    let theirs = note(A, "one, theirs\ntwo\n");
    assert_eq!(fixture.read("Plan.md"), theirs, "Noura writes nothing");
    let session = app.note(tab).unwrap();
    assert_eq!(session.text(), "\none, mine\ntwo\n", "your version");
    let conflict = session.conflict().expect("conflict review opens");
    assert_eq!(conflict.file_text(), "\none, theirs\ntwo\n");
    assert!(conflict.is_reviewing());

    // Autosave waits: typing more and pausing writes nothing.
    let at = type_into(&mut app, tab, |text| format!("{text}three\n"));
    save(&mut app, at);
    assert_eq!(fixture.read("Plan.md"), theirs);
    assert!(app.note(tab).unwrap().is_unsaved());

    // Keep editing closes the review, and the banner's Compare opens it
    // again.
    app.close_conflict_review(tab);
    let conflict = app.note(tab).unwrap().conflict().expect("the banner stays");
    assert!(!conflict.is_reviewing());
    app.review_conflict(tab);
    assert!(app.note(tab).unwrap().conflict().unwrap().is_reviewing());
    assert_eq!(fixture.read("Plan.md"), theirs);
}

/// The recovery snapshots in the workspace's history folder, by the kind
/// of version each keeps (`local` or `external`), with their bytes.
fn snapshots(fixture: &Fixture) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    let history = fixture.root.join(".noura/history");
    for folder in std::fs::read_dir(history).into_iter().flatten() {
        for file in std::fs::read_dir(folder.unwrap().path()).unwrap() {
            let path = file.unwrap().path();
            let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
            let kind = name.rsplit('-').next().unwrap().to_owned();
            found.push((kind, std::fs::read(path).unwrap()));
        }
    }
    found
}

#[test]
fn use_my_version_asks_first_keeps_the_files_version_and_writes_yours() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "one\ntwo\n"))]);
    let (mut app, tab) = in_conflict(&fixture);
    let theirs = note(A, "one, theirs\ntwo\n");

    app.choose_version(tab, ConflictChoice::Mine);
    let conflict = app.note(tab).unwrap().conflict().unwrap();
    assert_eq!(conflict.confirming(), Some(ConflictChoice::Mine));
    assert_eq!(
        fixture.read("Plan.md"),
        theirs,
        "nothing happens before you confirm"
    );
    assert!(snapshots(&fixture).is_empty());
    // Typing while the review is open goes in with your version.
    type_into(&mut app, tab, |text| format!("{text}three\n"));

    app.confirm_version(tab);

    assert_eq!(snapshots(&fixture), vec![("external".to_owned(), theirs)]);
    assert_eq!(fixture.read("Plan.md"), note(A, "one, mine\ntwo\nthree\n"));
    let session = app.note(tab).unwrap();
    assert!(session.conflict().is_none());
    assert!(session.save_problem().is_none());
    assert!(!session.is_unsaved());

    // Autosave runs again, and the next typing saves on top.
    let at = type_into(&mut app, tab, |text| format!("{text}four\n"));
    save(&mut app, at);
    assert_eq!(
        fixture.read("Plan.md"),
        note(A, "one, mine\ntwo\nthree\nfour\n")
    );
}

#[test]
fn use_the_files_version_asks_first_keeps_yours_and_shows_the_files() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "one\ntwo\n"))]);
    let (mut app, tab) = in_conflict(&fixture);
    let theirs = note(A, "one, theirs\ntwo\n");

    // Cancel takes the question back.
    app.choose_version(tab, ConflictChoice::File);
    app.cancel_version_choice(tab);
    assert_eq!(
        app.note(tab).unwrap().conflict().unwrap().confirming(),
        None
    );
    app.choose_version(tab, ConflictChoice::File);
    app.confirm_version(tab);

    assert_eq!(
        snapshots(&fixture),
        vec![("local".to_owned(), note(A, "one, mine\ntwo\n"))]
    );
    assert_eq!(fixture.read("Plan.md"), theirs);
    let session = app.note(tab).unwrap();
    assert!(session.conflict().is_none());
    assert!(!session.is_unsaved());
    assert_eq!(session.text(), "\none, theirs\ntwo\n");
    assert_eq!(
        app.take_note_reset(tab).as_deref(),
        Some("\none, theirs\ntwo\n"),
        "the editor shows the file's version"
    );

    let at = type_into(&mut app, tab, |text| format!("{text}three\n"));
    save(&mut app, at);
    assert_eq!(
        fixture.read("Plan.md"),
        note(A, "one, theirs\ntwo\nthree\n")
    );
}

#[test]
fn leaving_a_note_with_a_conflict_asks_first_with_the_conflict_as_the_reason() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "one\ntwo\n"))]);
    let (mut app, tab) = in_conflict(&fixture);
    let theirs = note(A, "one, theirs\ntwo\n");

    app.close_tab(tab);
    settle_saves(&mut app);

    let prompt = app.unsaved_prompt().expect("closing asks first").clone();
    assert_eq!(prompt.title, "Couldn\u{2019}t save \u{201c}Plan\u{201d}");
    assert_eq!(
        prompt.message,
        "The file changed in another app. Your changes are still in the note."
    );
    assert!(app.tabs().iter().any(|open| open.id() == tab));
    assert_eq!(
        fixture.read("Plan.md"),
        theirs,
        "the attempt writes nothing"
    );

    // Keep editing keeps the conflict; Discard changes shows the file's
    // version and lets the tab go.
    app.keep_editing();
    assert!(app.note(tab).unwrap().conflict().is_some());
    app.close_tab(tab);
    settle_saves(&mut app);
    app.discard_and_leave();
    assert!(app.tabs().iter().all(|open| open.id() != tab));
    assert_eq!(fixture.read("Plan.md"), theirs);
    assert!(snapshots(&fixture).is_empty());
}

#[test]
fn a_note_deleted_outside_noura_keeps_your_text_and_restore_writes_it_back() {
    let fixture = Fixture::new(&[("Docs/Plan.md", &note(A, "one\ntwo\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Docs/Plan.md");
    let at = type_into(&mut app, tab, |text| format!("{text}three\n"));

    std::fs::remove_dir_all(fixture.root.join("Docs")).unwrap();
    app.rescan();
    settle_saves(&mut app);

    let session = app.note(tab).expect("the note stays open");
    assert_eq!(session.text(), "\none\ntwo\nthree\n");
    assert!(session.is_deleted(), "the note offers to restore the file");
    save(&mut app, at);
    assert!(!fixture.exists("Docs/Plan.md"), "saves wait for Restore");

    app.restore_note(tab);

    assert_eq!(fixture.read("Docs/Plan.md"), note(A, "one\ntwo\nthree\n"));
    let session = app.note(tab).unwrap();
    assert!(!session.is_deleted());
    assert!(!session.is_unsaved());
    assert!(session.save_problem().is_none());
    assert_eq!(session.object_id(), Some(format!("note_{A}").as_str()));

    // Saves run again.
    let at = type_into(&mut app, tab, |text| format!("{text}four\n"));
    save(&mut app, at);
    assert_eq!(
        fixture.read("Docs/Plan.md"),
        note(A, "one\ntwo\nthree\nfour\n")
    );
}

#[test]
fn a_note_another_app_renames_and_moves_stays_open_under_its_new_name() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "one\ntwo\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let at = type_into(&mut app, tab, |text| format!("{text}three\n"));

    std::fs::create_dir(fixture.root.join("Archive")).unwrap();
    std::fs::rename(
        fixture.root.join("Plan.md"),
        fixture.root.join("Archive/Renamed.md"),
    )
    .unwrap();
    app.rescan();
    settle_saves(&mut app);

    let session = app.note(tab).expect("the tab stays on the note");
    assert_eq!(session.relative_path(), "Archive/Renamed.md");
    assert_eq!(session.name(), "Renamed");
    assert_eq!(session.text(), "\none\ntwo\nthree\n");
    assert!(session.conflict().is_none());
    assert!(!session.is_deleted());
    assert!(session.save_problem().is_none());

    save(&mut app, at);
    assert_eq!(
        fixture.read("Archive/Renamed.md"),
        note(A, "one\ntwo\nthree\n")
    );
    assert!(!fixture.exists("Plan.md"));

    // Another rename with nothing unsaved follows too.
    std::fs::rename(
        fixture.root.join("Archive/Renamed.md"),
        fixture.root.join("Archive/Again.md"),
    )
    .unwrap();
    app.rescan();
    let session = app.note(tab).unwrap();
    assert_eq!(session.name(), "Again");
    assert_eq!(session.text(), "\none\ntwo\nthree\n");
    assert!(!session.is_unsaved());
}

#[test]
fn restore_keeps_the_files_line_endings_and_byte_order_mark() {
    let fixture = Fixture::new(&[("Plan.md", b"\xEF\xBB\xBFone\r\ntwo\r\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    type_into(&mut app, tab, |text| format!("{text}three\n"));
    std::fs::remove_file(fixture.root.join("Plan.md")).unwrap();
    app.rescan();
    settle_saves(&mut app);

    app.restore_note(tab);

    assert_eq!(
        fixture.read("Plan.md"),
        b"\xEF\xBB\xBFone\r\ntwo\r\nthree\r\n"
    );
}

#[test]
fn restore_never_writes_over_a_file_that_came_back() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "one\ntwo\nthree\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    type_into(&mut app, tab, |text| text.replace("one", "one, mine"));
    std::fs::remove_file(fixture.root.join("Plan.md")).unwrap();
    app.rescan();
    settle_saves(&mut app);
    assert!(app.note(tab).unwrap().is_deleted());

    // Another app puts the file back before you choose Restore.
    support::write(
        &fixture.root,
        "Plan.md",
        &note(A, "one\ntwo\nthree, theirs\n"),
    );
    app.restore_note(tab);
    settle_saves(&mut app);

    assert_eq!(
        fixture.read("Plan.md"),
        note(A, "one, mine\ntwo\nthree, theirs\n"),
        "your typing merges with the file that came back"
    );
    assert!(!app.note(tab).unwrap().is_deleted());
}
