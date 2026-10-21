//! Seam 1: notes open in the editor. Opening a note from the tree, what
//! the editor and the properties panel show, saves after typing pauses and
//! during long typing, saves that count only once durable, and bytes on
//! disk before and after.

mod support;

use std::time::{Duration, Instant};

use app_core::{App, CommandSource, LineWidth, PropertyValue, SAVE_PAUSE, TabId};
use support::{Fixture, hold_writes, open_from_tree, settle_saves};

const ID: &str = "01jbcdefghjkmnpqrstvwxyz0a";

fn managed(frontmatter_endings: &str, body: &str) -> Vec<u8> {
    let end = frontmatter_endings;
    format!(
        "---{end}id: note_{ID}{end}type: note{end}color: green{end}\
         created: 2026-10-20T10:00:00Z{end}updated: 2026-10-20T10:00:00Z{end}---{end}{body}"
    )
    .into_bytes()
}

/// The editor's text, as the editor reports it after an edit: `change`
/// applied to what the note shows.
fn type_into(app: &mut App, tab: TabId, at: Instant, change: impl FnOnce(&str) -> String) {
    let text = change(app.note(tab).unwrap().text());
    app.edit_note(tab, 1, &text, at);
}

/// Let the save that typing at `at` started run to its end.
fn save_after_pause(app: &mut App, at: Instant) {
    app.save_due_notes(at + SAVE_PAUSE);
    settle_saves(app);
}

fn modified(fixture: &Fixture, relative: &str) -> std::time::SystemTime {
    std::fs::metadata(fixture.root.join(relative))
        .unwrap()
        .modified()
        .unwrap()
}

#[test]
fn a_managed_note_opens_from_the_tree_with_its_text_after_the_frontmatter() {
    let bytes = managed("\n", "# Heading\n\nSome *text*.\n");
    let fixture = Fixture::new(&[("Docs/Plan.md", &bytes)]);
    let mut app = fixture.app();

    let tab = open_from_tree(&mut app, "Docs/Plan.md");

    let note = app.note(tab).expect("the note opens in the editor");
    assert_eq!(note.text(), "# Heading\n\nSome *text*.\n");
    assert_eq!(note.name(), "Plan");
    assert_eq!(note.object_id(), Some(format!("note_{ID}").as_str()));
    let properties = note.properties().expect("a managed note has properties");
    let shown: Vec<(&str, bool)> = properties
        .iter()
        .map(|property| (property.key.as_str(), property.read_only))
        .collect();
    assert_eq!(
        shown,
        [
            ("id", true),
            ("type", true),
            ("color", false),
            ("created", true),
            ("updated", true)
        ]
    );
    assert_eq!(properties[2].value, PropertyValue::Text("green".into()));
}

#[test]
fn a_markdown_file_without_an_id_shows_its_whole_text_frontmatter_included() {
    let bytes = b"---\ntags: [a]\n---\n# Heading\nText";
    let fixture = Fixture::new(&[("Plain.md", bytes)]);
    let mut app = fixture.app();

    let tab = open_from_tree(&mut app, "Plain.md");

    let note = app.note(tab).unwrap();
    assert_eq!(note.text().as_bytes(), bytes);
    assert_eq!(note.properties(), None);
    assert_eq!(note.object_id(), None);
}

#[test]
fn a_file_noura_cant_read_as_text_says_so_in_its_tab() {
    let fixture = Fixture::new(&[("Bytes.md", b"\xff\xfe not UTF-8")]);
    let mut app = fixture.app();

    let tab = open_from_tree(&mut app, "Bytes.md");

    assert!(app.note(tab).is_none());
    assert!(app.note_unreadable(tab).is_some());
    assert_eq!(fixture.read("Bytes.md"), b"\xff\xfe not UTF-8");
}

#[test]
fn opening_showing_and_closing_an_unchanged_note_writes_nothing() {
    let notes: [(&str, Vec<u8>); 3] = [
        ("Managed.md", managed("\r\n", "Body\r\nno final newline")),
        (
            "Plain.md",
            b"\xEF\xBB\xBF# Plain\r\nmixed\nlone\rcr".to_vec(),
        ),
        ("Empty.md", Vec::new()),
    ];
    let files: Vec<(&str, &[u8])> = notes
        .iter()
        .map(|(path, bytes)| (*path, bytes.as_slice()))
        .collect();
    let fixture = Fixture::new(&files);
    let mut app = fixture.app();
    for (path, bytes) in &notes {
        let before = modified(&fixture, path);
        let tab = open_from_tree(&mut app, path);
        // The editor reports its text every frame it changes; unchanged text
        // isn't an edit.
        let text = app.note(tab).unwrap().text().to_owned();
        let now = Instant::now();
        app.edit_note(tab, 0, &text, now);
        app.save_due_notes(now + Duration::from_secs(10));
        settle_saves(&mut app);
        assert!(!app.note(tab).unwrap().is_unsaved());
        app.close_tab(tab);
        assert!(app.tabs().iter().all(|open| open.id() != tab));
        assert_eq!(&fixture.read(path), bytes, "{path}");
        assert_eq!(
            local_core::parse_markdown(path, &fixture.read(path)),
            local_core::parse_markdown(path, bytes)
        );
        assert_eq!(modified(&fixture, path), before, "{path} wasn't written");
    }
}

type Edit = (&'static str, Vec<u8>, &'static str, &'static str, Vec<u8>);

#[test]
fn an_edit_changes_only_the_bytes_it_touched() {
    // Each file, its bytes, the text replaced and its replacement, and the
    // bytes expected after the save.
    let cases: [Edit; 4] = [
        (
            "CRLF managed.md",
            managed("\r\n", "one\r\ntwo\r\nthree\r\n"),
            "two",
            "2",
            managed("\r\n", "one\r\n2\r\nthree\r\n"),
        ),
        (
            "BOM.md",
            b"\xEF\xBB\xBF# Title\nfirst\nsecond".to_vec(),
            "second",
            "second, edited",
            b"\xEF\xBB\xBF# Title\nfirst\nsecond, edited".to_vec(),
        ),
        (
            "No final newline.md",
            managed("\n", "last line"),
            "last line",
            "last line\nand one more",
            managed("\n", "last line\nand one more"),
        ),
        (
            "Mixed.md",
            b"a\r\nb\nc\r\nd\n".to_vec(),
            "b\n",
            "b\nnew\n",
            b"a\r\nb\nnew\nc\r\nd\n".to_vec(),
        ),
    ];
    let files: Vec<(&str, &[u8])> = cases
        .iter()
        .map(|(path, bytes, ..)| (*path, bytes.as_slice()))
        .collect();
    let fixture = Fixture::new(&files);
    let mut app = fixture.app();
    for (path, _, from, to, expected) in &cases {
        let tab = open_from_tree(&mut app, path);
        let at = Instant::now();
        type_into(&mut app, tab, at, |text| text.replacen(from, to, 1));
        save_after_pause(&mut app, at);
        assert_eq!(
            String::from_utf8_lossy(&fixture.read(path)),
            String::from_utf8_lossy(expected),
            "{path}"
        );
        assert!(!app.note(tab).unwrap().is_unsaved(), "{path}");
    }
}

#[test]
fn a_save_starts_once_typing_pauses_for_300_milliseconds() {
    let bytes = managed("\n", "Body\n");
    let fixture = Fixture::new(&[("Plan.md", &bytes)]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let at = Instant::now();

    type_into(&mut app, tab, at, |text| format!("{text}more\n"));
    assert_eq!(app.next_note_save(), Some(at + SAVE_PAUSE));
    app.save_due_notes(at + Duration::from_millis(299));

    assert!(!app.is_saving_notes());
    assert!(app.note(tab).unwrap().is_unsaved());
    assert_eq!(fixture.read("Plan.md"), bytes);
    app.save_due_notes(at + Duration::from_millis(300));
    assert!(app.is_saving_notes());
    settle_saves(&mut app);
    assert_eq!(fixture.read("Plan.md"), managed("\n", "Body\nmore\n"));
}

#[test]
fn continuous_typing_saves_at_least_every_two_seconds() {
    let fixture = Fixture::new(&[("Plan.md", &managed("\n", ""))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let start = Instant::now();

    // A key every 200 milliseconds never pauses long enough.
    let mut saved_at = None;
    for step in 0..15u32 {
        let at = start + Duration::from_millis(200) * step;
        type_into(&mut app, tab, at, |text| format!("{text}x"));
        app.save_due_notes(at);
        if app.is_saving_notes() {
            saved_at = Some(at - start);
            break;
        }
    }

    assert_eq!(saved_at, Some(Duration::from_secs(2)));
    settle_saves(&mut app);
    assert!(
        String::from_utf8(fixture.read("Plan.md"))
            .unwrap()
            .ends_with("---\nxxxxxxxxxxx")
    );
}

#[test]
fn mod_s_saves_at_once() {
    let fixture = Fixture::new(&[("Plan.md", &managed("\n", "Body\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let at = Instant::now();
    type_into(&mut app, tab, at, |text| format!("{text}typed\n"));

    assert!(app.run_command("save", CommandSource::Key, at));

    assert!(app.is_saving_notes() || !app.note(tab).unwrap().is_unsaved());
    settle_saves(&mut app);
    assert_eq!(fixture.read("Plan.md"), managed("\n", "Body\ntyped\n"));
}

#[test]
fn a_save_counts_only_once_its_durable_write_returns_and_closing_waits_for_it() {
    let bytes = managed("\n", "Body\n");
    let fixture = Fixture::new(&[("Plan.md", &bytes)]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let at = Instant::now();
    type_into(&mut app, tab, at, |text| format!("{text}typed\n"));

    let lock = hold_writes(&fixture, &app);
    app.save_due_notes(at + SAVE_PAUSE);
    std::thread::sleep(Duration::from_millis(300));
    app.poll();

    assert!(app.is_saving_notes());
    assert!(app.note(tab).unwrap().is_unsaved());
    assert_eq!(fixture.read("Plan.md"), bytes);
    app.close_tab(tab);
    assert!(
        app.tabs().iter().any(|open| open.id() == tab),
        "the tab waits for its save"
    );
    assert!(app.unsaved_prompt().is_none());

    lock.unlock().unwrap();
    settle_saves(&mut app);

    assert_eq!(fixture.read("Plan.md"), managed("\n", "Body\ntyped\n"));
    assert!(app.tabs().iter().all(|open| open.id() != tab));
}

#[test]
fn the_first_edit_keeps_a_preview_tab() {
    let fixture = Fixture::new(&[("Plan.md", &managed("\n", "Body\n")), ("Other.md", b"x")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    assert!(app.active_tab().unwrap().is_preview());

    type_into(&mut app, tab, Instant::now(), |text| format!("{text}typed"));

    assert!(!app.active_tab().unwrap().is_preview());
    open_from_tree(&mut app, "Other.md");
    assert_eq!(app.tabs().len(), 2, "the edited note keeps its tab");
}

#[test]
fn switching_to_another_note_saves_the_one_you_leave() {
    let fixture = Fixture::new(&[("Plan.md", &managed("\n", "Body\n")), ("Other.md", b"x")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let at = Instant::now();
    type_into(&mut app, tab, at, |text| format!("{text}typed\n"));

    open_from_tree(&mut app, "Other.md");

    assert!(app.is_saving_notes() || !app.note(tab).unwrap().is_unsaved());
    settle_saves(&mut app);
    assert_eq!(fixture.read("Plan.md"), managed("\n", "Body\ntyped\n"));
}

#[test]
fn a_property_edit_writes_the_frontmatter_and_leaves_the_body_bytes_alone() {
    let body = "\r\n# Heading\r\nkept\nas it is";
    let fixture = Fixture::new(&[("Plan.md", &managed("\r\n", body))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");

    app.set_note_property(tab, "color", PropertyValue::Text("blue".into()));
    settle_saves(&mut app);

    let written = String::from_utf8(fixture.read("Plan.md")).unwrap();
    assert!(written.contains("\r\ncolor: blue\r\n"), "{written:?}");
    assert!(
        written.ends_with(&format!("\r\n---\r\n{body}")),
        "{written:?}"
    );
    let note = app.note(tab).unwrap();
    assert!(!note.is_unsaved());
    assert!(note.save_problem().is_none());
    let color = note
        .properties()
        .unwrap()
        .into_iter()
        .find(|property| property.key == "color")
        .unwrap();
    assert_eq!(color.value, PropertyValue::Text("blue".into()));
}

#[test]
fn noura_keeps_its_own_properties_read_only() {
    let bytes = managed("\n", "Body\n");
    let fixture = Fixture::new(&[("Plan.md", &bytes)]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");

    for key in ["id", "type", "created", "updated", "missing"] {
        app.set_note_property(tab, key, PropertyValue::Text("changed".into()));
    }
    settle_saves(&mut app);

    assert_eq!(fixture.read("Plan.md"), bytes);
}

#[test]
fn another_types_properties_show_read_only_until_its_plugin_supplies_a_panel() {
    let bytes =
        format!("---\nid: task_{ID}\ntype: task\nstatus: todo\npriority: high\n---\nSteps\n");
    let fixture = Fixture::new(&[("Task.md", bytes.as_bytes())]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Task.md");

    let note = app.note(tab).unwrap();
    assert_eq!(note.text(), "Steps\n");
    let properties = note.properties().unwrap();
    assert!(properties.iter().all(|property| property.read_only));
    app.set_note_property(tab, "status", PropertyValue::Text("done".into()));
    settle_saves(&mut app);
    assert_eq!(fixture.read("Task.md"), bytes.as_bytes());
}

#[test]
fn a_property_edit_never_overwrites_a_file_that_changed_since_noura_read_it() {
    let fixture = Fixture::new(&[("Plan.md", &managed("\n", "Body\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let outside = managed("\n", "Changed in another app\n");
    support::write(&fixture.root, "Plan.md", &outside);

    app.set_note_property(tab, "color", PropertyValue::Text("blue".into()));
    settle_saves(&mut app);

    assert_eq!(fixture.read("Plan.md"), outside);
    assert_eq!(
        app.note(tab).unwrap().save_problem().unwrap().reason,
        "The file changed outside Noura while it was saving."
    );
}

#[test]
fn the_name_field_renames_the_file_and_the_note_keeps_its_id() {
    let bytes = managed("\n", "Body\n");
    let fixture = Fixture::new(&[("Docs/Plan.md", &bytes)]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Docs/Plan.md");

    app.rename_note(tab, "  ..New/plan: v2?  ");

    assert!(!fixture.exists("Docs/Plan.md"));
    assert_eq!(fixture.read("Docs/Newplan v2.md"), bytes);
    let note = app.note(tab).unwrap();
    assert_eq!(note.relative_path(), "Docs/Newplan v2.md");
    assert_eq!(note.name(), "Newplan v2");
    assert_eq!(note.object_id(), Some(format!("note_{ID}").as_str()));
    assert!(note.name_problem().is_none());
}

#[test]
fn an_empty_or_unchanged_name_keeps_the_old_one() {
    let fixture = Fixture::new(&[("Plan.md", &managed("\n", "Body\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");

    for typed in ["", "   ", " / ", "Plan", " Plan "] {
        app.rename_note(tab, typed);
        assert!(fixture.exists("Plan.md"), "{typed:?}");
        assert_eq!(app.note(tab).unwrap().relative_path(), "Plan.md");
        assert!(app.note(tab).unwrap().name_problem().is_none());
    }
}

#[test]
fn a_taken_name_keeps_the_old_one_and_says_why() {
    let fixture = Fixture::new(&[
        ("Plan.md", &managed("\n", "Body\n")),
        ("Taken.md", b"other"),
    ]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");

    app.rename_note(tab, "taken");

    assert!(fixture.exists("Plan.md"));
    assert_eq!(fixture.read("Taken.md"), b"other");
    assert_eq!(
        app.note(tab).unwrap().name_problem(),
        Some("A file with that name already exists in this folder.")
    );
}

#[test]
fn a_committed_name_stops_at_200_characters() {
    let fixture = Fixture::new(&[("Plan.md", &managed("\n", "Body\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");

    app.rename_note(tab, &"n".repeat(250));

    assert_eq!(
        app.note(tab).unwrap().relative_path(),
        format!("{}.md", "n".repeat(200))
    );
}

#[test]
fn renaming_saves_the_notes_typing_first() {
    let fixture = Fixture::new(&[("Plan.md", &managed("\n", "Body\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    type_into(&mut app, tab, Instant::now(), |text| {
        format!("{text}typed\n")
    });

    app.rename_note(tab, "Renamed");

    assert_eq!(fixture.read("Renamed.md"), managed("\n", "Body\ntyped\n"));
    assert!(!app.note(tab).unwrap().is_unsaved());
}

#[test]
fn a_save_of_a_file_deleted_outside_noura_says_so_and_keeps_the_text() {
    let fixture = Fixture::new(&[("Plan.md", &managed("\n", "Body\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    std::fs::remove_file(fixture.root.join("Plan.md")).unwrap();
    let at = Instant::now();

    type_into(&mut app, tab, at, |text| format!("{text}typed\n"));
    save_after_pause(&mut app, at);

    let note = app.note(tab).unwrap();
    assert_eq!(
        note.save_problem().unwrap().reason,
        "The file was moved or deleted outside Noura."
    );
    assert!(note.is_unsaved());
    assert_eq!(note.text(), "Body\ntyped\n");
    assert!(!fixture.exists("Plan.md"), "a failed save writes nothing");
}

#[test]
fn line_width_applies_to_every_note_and_survives_a_restart() {
    let fixture = Fixture::new(&[("Plan.md", &managed("\n", "Body\n"))]);
    let mut app = fixture.app();
    assert_eq!(app.line_width(), LineWidth::Default);

    app.set_line_width(LineWidth::Narrow);
    assert_eq!(app.line_width(), LineWidth::Narrow);
    drop(app);

    let app = fixture.app();
    assert_eq!(app.line_width(), LineWidth::Narrow);
    assert_eq!(app.layout().line_width, LineWidth::Narrow);
    assert_eq!(LineWidth::Narrow.points(), Some(640.0));
    assert_eq!(LineWidth::Default.points(), Some(768.0));
    assert_eq!(LineWidth::Full.points(), None);
}

/// Saves fail with a real permission error while the note's folder is
/// read-only.
#[cfg(unix)]
mod failures {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn read_only(fixture: &Fixture, folder: &str, read_only: bool) {
        let mode = if read_only { 0o555 } else { 0o755 };
        std::fs::set_permissions(
            fixture.root.join(folder),
            std::fs::Permissions::from_mode(mode),
        )
        .unwrap();
    }

    /// A note in a read-only folder, with typing that can't save.
    fn failing(fixture: &Fixture, app: &mut App) -> TabId {
        let tab = open_from_tree(app, "Docs/Plan.md");
        read_only(fixture, "Docs", true);
        let at = Instant::now();
        type_into(app, tab, at, |text| format!("{text}typed\n"));
        save_after_pause(app, at);
        tab
    }

    fn fixture() -> Fixture {
        Fixture::new(&[("Docs/Plan.md", &managed("\n", "Body\n"))])
    }

    const NO_PERMISSION: &str = "Noura doesn't have permission to change this file.";

    #[test]
    fn a_failed_save_says_why_and_the_next_edit_tries_again() {
        let fixture = fixture();
        let mut app = fixture.app();
        let tab = failing(&fixture, &mut app);

        let note = app.note(tab).unwrap();
        assert_eq!(note.save_problem().unwrap().reason, NO_PERMISSION);
        assert!(note.is_unsaved());
        assert_eq!(fixture.read("Docs/Plan.md"), managed("\n", "Body\n"));
        // No retry until the next edit or Try again.
        app.save_due_notes(Instant::now() + Duration::from_secs(10));
        assert!(!app.is_saving_notes());

        read_only(&fixture, "Docs", false);
        let at = Instant::now();
        type_into(&mut app, tab, at, |text| format!("{text}more\n"));
        save_after_pause(&mut app, at);

        assert!(app.note(tab).unwrap().save_problem().is_none());
        assert_eq!(
            fixture.read("Docs/Plan.md"),
            managed("\n", "Body\ntyped\nmore\n")
        );
    }

    #[test]
    fn try_again_saves_once_the_problem_is_gone() {
        let fixture = fixture();
        let mut app = fixture.app();
        let tab = failing(&fixture, &mut app);
        read_only(&fixture, "Docs", false);

        app.save_note_now(tab);
        settle_saves(&mut app);

        assert!(app.note(tab).unwrap().save_problem().is_none());
        assert_eq!(fixture.read("Docs/Plan.md"), managed("\n", "Body\ntyped\n"));
    }

    #[test]
    fn closing_a_tab_whose_save_fails_asks_and_keep_editing_keeps_it_open() {
        let fixture = fixture();
        let mut app = fixture.app();
        let tab = failing(&fixture, &mut app);

        app.close_tab(tab);
        settle_saves(&mut app);

        let prompt = app.unsaved_prompt().expect("Noura asks before closing");
        assert_eq!(prompt.title, "Couldn\u{2019}t save \u{201c}Plan\u{201d}");
        assert_eq!(
            prompt.message,
            format!("{NO_PERMISSION} Your changes are still in the note.")
        );
        assert!(app.tabs().iter().any(|open| open.id() == tab));

        app.keep_editing();

        assert!(app.unsaved_prompt().is_none());
        assert_eq!(app.active_tab().map(|open| open.id()), Some(tab));
        assert_eq!(app.note(tab).unwrap().text(), "Body\ntyped\n");
        read_only(&fixture, "Docs", false);
    }

    #[test]
    fn discard_changes_closes_and_leaves_the_file_as_it_is_on_disk() {
        let fixture = fixture();
        let mut app = fixture.app();
        let tab = failing(&fixture, &mut app);
        app.close_tab(tab);
        settle_saves(&mut app);
        read_only(&fixture, "Docs", false);

        app.discard_and_leave();

        assert!(app.tabs().iter().all(|open| open.id() != tab));
        assert_eq!(fixture.read("Docs/Plan.md"), managed("\n", "Body\n"));
    }

    #[test]
    fn try_again_in_the_prompt_saves_then_closes() {
        let fixture = fixture();
        let mut app = fixture.app();
        let tab = failing(&fixture, &mut app);
        app.close_tab(tab);
        settle_saves(&mut app);
        read_only(&fixture, "Docs", false);

        app.retry_leaving();
        settle_saves(&mut app);

        assert!(app.unsaved_prompt().is_none());
        assert!(app.tabs().iter().all(|open| open.id() != tab));
        assert_eq!(fixture.read("Docs/Plan.md"), managed("\n", "Body\ntyped\n"));
    }

    #[test]
    fn quitting_waits_for_every_note_and_asks_when_one_fails() {
        let fixture = fixture();
        let mut app = fixture.app();
        failing(&fixture, &mut app);

        assert!(app.run_command("quit", CommandSource::Key, Instant::now()));
        settle_saves(&mut app);

        assert!(!app.take_quit());
        assert!(app.unsaved_prompt().is_some());
        read_only(&fixture, "Docs", false);
        app.retry_leaving();
        settle_saves(&mut app);
        assert!(app.take_quit());
        assert_eq!(fixture.read("Docs/Plan.md"), managed("\n", "Body\ntyped\n"));
    }

    #[test]
    fn closing_the_window_waits_for_every_note_too() {
        let fixture = fixture();
        let mut app = fixture.app();
        failing(&fixture, &mut app);

        app.request_close_window();
        settle_saves(&mut app);

        assert!(!app.take_close_window());
        assert!(app.unsaved_prompt().is_some());
        app.discard_and_leave();
        assert!(app.take_close_window());
        read_only(&fixture, "Docs", false);
        assert_eq!(fixture.read("Docs/Plan.md"), managed("\n", "Body\n"));
    }

    #[test]
    fn switching_workspace_waits_for_every_note() {
        let fixture = fixture();
        let other = tempfile::tempdir().unwrap();
        let mut app = fixture.app();
        failing(&fixture, &mut app);
        let first = app.workspace().unwrap().id().to_owned();

        app.open_folder(other.path().to_owned());
        settle_saves(&mut app);

        assert!(!app.is_busy(), "the other workspace doesn't start opening");
        assert!(app.unsaved_prompt().is_some());
        app.discard_and_leave();
        support::settle(&mut app);
        assert_ne!(app.workspace().unwrap().id(), first);
        read_only(&fixture, "Docs", false);
        assert_eq!(fixture.read("Docs/Plan.md"), managed("\n", "Body\n"));
    }
}
