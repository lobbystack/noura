//! Seam 1: the Inbox's view state, built from the index and kept current
//! by the change stream.

mod support;

use std::{
    sync::{
        Arc,
        mpsc::{Receiver, channel},
    },
    time::{Duration, Instant, SystemTime},
};

use app_core::{App, AppError, Change, InboxSectionProvider, LinkAction};
use support::Fixture;

/// Give the file at `relative` the modification time `seconds` after the
/// Unix epoch.
fn touch(fixture: &Fixture, relative: &str, seconds: u64) {
    let file = std::fs::File::options()
        .write(true)
        .open(fixture.root.join(relative))
        .unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
        .unwrap();
}

#[test]
fn the_greeting_follows_the_time_of_day() {
    let fixture = Fixture::new(&[]);
    let system = support::FakeSystem::new();
    let app = fixture.app_with(system.clone());

    let greetings: Vec<(u8, &str)> = [0, 4, 5, 11, 12, 17, 18, 23]
        .into_iter()
        .map(|hour| {
            system.set_hour(hour);
            (hour, app.inbox().greeting)
        })
        .collect();

    assert_eq!(
        greetings,
        [
            (0, "Working late"),
            (4, "Working late"),
            (5, "Good morning"),
            (11, "Good morning"),
            (12, "Good afternoon"),
            (17, "Good afternoon"),
            (18, "Good evening"),
            (23, "Good evening"),
        ]
    );
}

/// The Recent list's titles, top to bottom.
fn recent(app: &App) -> Vec<String> {
    app.inbox()
        .recent
        .iter()
        .map(|note| note.title.clone())
        .collect()
}

#[test]
fn recent_lists_the_five_notes_changed_last_newest_first() {
    let task =
        b"---\nid: task_01k00000000000000000000002\ntype: task\nstatus: todo\n---\n\nShip.\n";
    let fixture = Fixture::new(&[
        ("Monday.md", b"# Monday\n"),
        ("Tuesday.md", b"# Tuesday\n"),
        (
            "Journal/Wednesday.md",
            &support::note("01k00000000000000000000003", "Midweek."),
        ),
        ("Thursday.md", b"# Thursday\n"),
        ("Friday.md", b"---\nid: [broken\n---\n"),
        ("Saturday.md", b"# Saturday\n"),
        ("Tasks/Ship.md", task),
        ("Photo.png", b"not really an image"),
    ]);
    let order = [
        "Monday.md",
        "Tuesday.md",
        "Journal/Wednesday.md",
        "Thursday.md",
        "Friday.md",
        "Saturday.md",
        "Tasks/Ship.md",
        "Photo.png",
    ];
    for (day, path) in order.iter().enumerate() {
        touch(&fixture, path, 1_800_000_000 + day as u64 * 60);
    }

    let app = fixture.app();

    assert_eq!(
        recent(&app),
        ["Saturday", "Friday", "Thursday", "Wednesday", "Tuesday"]
    );
    let paths: Vec<String> = app
        .inbox()
        .recent
        .iter()
        .map(|note| note.relative_path.clone())
        .collect();
    assert_eq!(paths[3], "Journal/Wednesday.md");
}

#[test]
fn notes_that_share_a_title_in_recent_are_marked_to_show_their_paths() {
    let fixture = Fixture::new(&[
        ("Plan.md", b"# Plan\n"),
        ("Ideas/Plan.md", b"# Plan\n"),
        ("Ideas/Apples.md", b"# Apples\n"),
    ]);
    for (minute, path) in ["Plan.md", "Ideas/Plan.md", "Ideas/Apples.md"]
        .into_iter()
        .enumerate()
    {
        touch(&fixture, path, 1_800_000_000 + minute as u64 * 60);
    }

    let app = fixture.app();

    let marked: Vec<(String, String, bool)> = app
        .inbox()
        .recent
        .into_iter()
        .map(|note| (note.title, note.relative_path, note.shares_title))
        .collect();
    assert_eq!(
        marked,
        [
            ("Apples".to_owned(), "Ideas/Apples.md".to_owned(), false),
            ("Plan".to_owned(), "Ideas/Plan.md".to_owned(), true),
            ("Plan".to_owned(), "Plan.md".to_owned(), true),
        ]
    );
}

#[test]
fn an_empty_workspace_has_no_recent_notes() {
    let fixture = Fixture::new(&[]);

    let app = fixture.app();

    assert_eq!(recent(&app), Vec::<String>::new());
}

/// A file Noura can't read is fixed in another app, so its link shows it in
/// the file manager instead of opening it.
#[cfg(unix)]
#[test]
fn a_file_noura_cannot_read_links_to_the_file_manager() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new(&[("Notes/Locked.md", b"# Locked\n")]);
    let locked = fixture.root.join("Notes/Locked.md");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&locked).is_ok() {
        // An administrator reads every file, so there's nothing to show.
        return;
    }
    let system = support::FakeSystem::on(app_core::Platform::MacOs);

    let mut app = fixture.app_with(system.clone());

    let reveal = (
        "Show Notes/Locked.md in Finder".to_owned(),
        "Notes/Locked.md".to_owned(),
        LinkAction::Reveal,
    );
    assert_eq!(
        attention(&app),
        [(
            "Noura can't open Notes/Locked.md. Check that you have permission to read it."
                .to_owned(),
            vec![reveal],
        )]
    );
    let link = app.inbox().needs_attention[0].links[0].clone();
    app.follow_link(&link, app_core::OpenAs::Preview);
    assert_eq!(system.calls(), [support::Call::Reveal(locked.clone())]);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
}

/// A folder Noura can't open is fixed in another app too, so its problem
/// names it and links to it in the file manager instead of trying to open
/// it in a tab.
#[cfg(unix)]
#[test]
fn a_folder_noura_cannot_open_links_to_the_file_manager() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new(&[("Open.md", b"# Open\n"), ("Locked/Kept.md", b"# Kept\n")]);
    let locked = fixture.root.join("Locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&locked).is_ok() {
        // An administrator opens every folder, so there's nothing to show.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let system = support::FakeSystem::on(app_core::Platform::MacOs);

    let mut app = fixture.app_with(system.clone());
    let shown = attention(&app);
    let tabs_before = app.tabs().len();
    let link = app
        .inbox()
        .needs_attention
        .first()
        .map(|item| item.links[0].clone());
    if let Some(link) = &link {
        app.follow_link(link, app_core::OpenAs::Preview);
    }
    let tabs_after = app.tabs().len();
    let notice = support::notice(&app);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(
        shown,
        [(
            "Noura can't open the folder Locked. Check that you have permission to read it."
                .to_owned(),
            vec![(
                "Show Locked in Finder".to_owned(),
                "Locked".to_owned(),
                LinkAction::Reveal,
            )],
        )]
    );
    assert_eq!(system.calls(), [support::Call::Reveal(locked)]);
    assert_eq!((tabs_after, notice), (tabs_before, None));
}

/// A name with no UTF-8 form on Linux, or with an unpaired surrogate on
/// Windows. macOS refuses to create such names.
#[cfg(target_os = "linux")]
fn unrepresentable(stem: &str) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    std::ffi::OsString::from_vec([stem.as_bytes(), b"\xe9.md"].concat())
}

#[cfg(windows)]
fn unrepresentable(stem: &str) -> std::ffi::OsString {
    use std::os::windows::ffi::OsStringExt;
    let mut wide: Vec<u16> = stem.encode_utf16().collect();
    wide.push(0xD800);
    wide.extend(".md".encode_utf16());
    std::ffi::OsString::from_wide(&wide)
}

/// A name Noura can't represent is fixed in another app: the problem
/// links to the folder that holds it, in the file manager.
#[cfg(any(target_os = "linux", windows))]
#[test]
fn a_name_noura_cannot_represent_links_to_its_folder_in_the_file_manager() {
    let fixture = Fixture::new(&[("Notes/Kept.md", b"# Kept\n")]);
    std::fs::write(fixture.root.join(unrepresentable("Caf")), b"# Odd\n").unwrap();
    std::fs::write(
        fixture.root.join("Notes").join(unrepresentable("D")),
        b"# Odd\n",
    )
    .unwrap();

    let app = fixture.app_with(support::FakeSystem::on(app_core::Platform::Linux));

    let rename = "has characters Noura can't represent, so Noura leaves it as it is. Rename it \
                  in another app to use it in Noura.";
    assert_eq!(
        attention(&app),
        [
            (
                format!("A name at the top of the workspace {rename}"),
                vec![]
            ),
            (
                format!("A name in Notes {rename}"),
                vec![(
                    "Show Notes in the file manager".to_owned(),
                    "Notes".to_owned(),
                    LinkAction::Reveal,
                )],
            ),
        ]
    );
}

/// How long a test waits for the change stream before it fails.
const PATIENCE: Duration = Duration::from_secs(15);

/// Wait on the change stream, polling after each change, until `done`
/// holds for the view state. Only a change wakes the wait, so a view that
/// updates without announcing it fails the test.
fn wait_until(app: &mut App, changes: &Receiver<Change>, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        app.poll();
        let _ = changes.try_iter().count();
        if done(app) {
            return;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "timed out waiting until {what}");
        let _ = changes.recv_timeout(left);
    }
}

/// The paths Needs attention links to.
fn attention_paths(app: &App) -> Vec<String> {
    app.inbox()
        .needs_attention
        .into_iter()
        .flat_map(|item| item.links)
        .map(|link| link.relative_path)
        .collect()
}

#[test]
fn the_inbox_follows_changes_other_apps_make_with_no_refresh() {
    let plan = support::note("01k00000000000000000000001", "The plan.");
    let fixture = Fixture::new(&[("Plan.md", &plan)]);
    let mut app = fixture.app();
    let (sender, changes) = channel();
    app.subscribe(move |change| {
        let _ = sender.send(change);
    });
    assert_eq!(attention_paths(&app), Vec::<String>::new());

    support::write(
        &fixture.root,
        "Plan.md",
        b"---\nid: [broken\n---\n\nThe plan.\n",
    );
    wait_until(&mut app, &changes, "the broken properties show", |app| {
        attention_paths(app) == ["Plan.md"]
    });
    assert_eq!(
        app.inbox().needs_attention[0].message,
        "Noura can't read the properties at the top of Plan.md."
    );

    support::write(&fixture.root, "Plan.md", &plan);
    wait_until(&mut app, &changes, "the fixed properties leave", |app| {
        attention_paths(app).is_empty()
    });

    support::write(&fixture.root, "Ideas/Fresh.md", b"# Fresh\n");
    wait_until(&mut app, &changes, "the new note shows in Recent", |app| {
        recent(app).first().map(String::as_str) == Some("Fresh")
    });
    assert_eq!(recent(&app), ["Fresh", "Plan"]);
}

/// A plugin's Inbox section that shows `lines`.
fn showing(lines: &[&str]) -> Arc<dyn InboxSectionProvider> {
    let lines: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    Arc::new(move || Ok(lines.clone()))
}

/// The plugin sections as (title, what they show).
fn sections(app: &App) -> Vec<(String, Result<Vec<String>, String>)> {
    app.inbox()
        .sections
        .into_iter()
        .map(|section| (section.title, section.content))
        .collect()
}

fn titles(app: &App) -> Vec<String> {
    sections(app).into_iter().map(|(title, _)| title).collect()
}

#[test]
fn plugin_sections_follow_their_plugins_rail_order_and_leave_when_a_plugin_turns_off() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let (sender, changes) = channel();
    app.subscribe(move |change| {
        let _ = sender.send(change);
    });
    app.register_page("test-tasks", "Tasks");
    app.register_page("test-calendar", "Calendar");

    app.add_inbox_section(
        "calendar",
        Some("test-calendar"),
        "Next 7 days",
        showing(&["Dentist on Thursday"]),
    );
    app.add_inbox_section("weather", None, "Weather", showing(&["Sunny"]));
    app.add_inbox_section(
        "tasks",
        Some("test-tasks"),
        "Due today",
        showing(&["Water the plants"]),
    );

    assert_eq!(
        sections(&app),
        [
            (
                "Due today".to_owned(),
                Ok(vec!["Water the plants".to_owned()])
            ),
            (
                "Next 7 days".to_owned(),
                Ok(vec!["Dentist on Thursday".to_owned()])
            ),
            ("Weather".to_owned(), Ok(vec!["Sunny".to_owned()])),
        ]
    );
    assert!(changes.try_iter().any(|change| change == Change::Inbox));
    let second = app.new_window().unwrap();
    assert_eq!(titles(&second), ["Due today", "Next 7 days", "Weather"]);

    app.move_page("test-calendar", 0);
    assert_eq!(titles(&app), ["Next 7 days", "Due today", "Weather"]);

    app.remove_inbox_section("calendar");
    assert_eq!(titles(&app), ["Due today", "Weather"]);
    assert!(changes.try_iter().any(|change| change == Change::Inbox));
}

#[test]
fn a_section_that_fails_shows_its_error_in_its_own_place_while_the_rest_loads() {
    let fixture = Fixture::new(&[
        ("Plan.md", b"# Plan\n"),
        ("Broken.md", b"---\nid: [broken\n---\n"),
    ]);
    let mut app = fixture.app();
    app.register_page("test-tasks", "Tasks");
    app.register_page("test-calendar", "Calendar");

    app.add_inbox_section(
        "tasks",
        Some("test-tasks"),
        "Due today",
        Arc::new(|| Err(AppError::new("plugin_failed", "Tasks stopped responding."))),
    );
    app.add_inbox_section(
        "calendar",
        Some("test-calendar"),
        "Next 7 days",
        showing(&["Dentist on Thursday"]),
    );

    assert_eq!(
        sections(&app),
        [
            (
                "Due today".to_owned(),
                Err("Tasks stopped responding.".to_owned())
            ),
            (
                "Next 7 days".to_owned(),
                Ok(vec!["Dentist on Thursday".to_owned()])
            ),
        ]
    );
    assert_eq!(attention_paths(&app), ["Broken.md"]);
    assert_eq!(recent(&app).len(), 2);
}

#[test]
fn plugin_sections_load_again_when_the_files_change() {
    let fixture = Fixture::new(&[("Plan.md", b"# Plan\n")]);
    let mut app = fixture.app();
    let (sender, changes) = channel();
    app.subscribe(move |change| {
        let _ = sender.send(change);
    });
    let root = fixture.root.clone();
    app.add_inbox_section(
        "counter",
        None,
        "Files",
        Arc::new(move || {
            let files = std::fs::read_dir(&root).unwrap().count();
            Ok(vec![format!("{files} entries")])
        }),
    );
    let before = std::fs::read_dir(&fixture.root).unwrap().count();
    assert_eq!(sections(&app)[0].1, Ok(vec![format!("{before} entries")]));

    support::write(&fixture.root, "Fresh.md", b"# Fresh\n");

    let after = Ok(vec![format!("{} entries", before + 1)]);
    wait_until(&mut app, &changes, "the section loads again", |app| {
        sections(app)[0].1 == after
    });
}

/// The files of the shared `queries-v1` conformance fixture, as
/// (path, text).
fn queries_v1_files() -> Vec<(String, String)> {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/workspace-format/fixtures/queries-v1.json"
    ))
    .unwrap();
    fixture["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap().to_owned(),
                file["content"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// A link as (label, path, action).
type Link = (String, String, LinkAction);

/// Needs attention as (message, links).
fn attention(app: &App) -> Vec<(String, Vec<Link>)> {
    app.inbox()
        .needs_attention
        .into_iter()
        .map(|item| {
            let links = item
                .links
                .into_iter()
                .map(|link| (link.label, link.relative_path, link.action))
                .collect();
            (item.message, links)
        })
        .collect()
}

fn open(path: &str) -> Link {
    (format!("Open {path}"), path.to_owned(), LinkAction::Open)
}

#[test]
fn needs_attention_lists_the_queries_fixture_s_problems_in_plain_words_with_their_files() {
    let files = queries_v1_files();
    let mut files: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_bytes()))
        .collect();
    // The index reports a file once for each object it links to that no
    // longer exists.
    let twice = b"---\nid: task_01k00000000000000000000005\ntype: task\nstatus: todo\n\
        project: project_01k00000000000000000000097\n\
        chat_id: chat_01k00000000000000000000098\n---\n\nTwice.\n";
    files.push(("Tasks/twice.md", twice));
    let fixture = Fixture::new(&files);

    let app = fixture.app();

    assert_eq!(
        attention(&app),
        [
            (
                "Tasks/call.md links to an object that no longer exists.".to_owned(),
                vec![open("Tasks/call.md")],
            ),
            (
                "Tasks/twice.md links to an object that no longer exists.".to_owned(),
                vec![open("Tasks/twice.md")],
            ),
            (
                "Notes/dup-a.md and Notes/dup-b.md use the same ID. Remove the id line from \
                 the copy."
                    .to_owned(),
                vec![open("Notes/dup-a.md"), open("Notes/dup-b.md")],
            ),
            (
                "Noura can't read the properties at the top of Notes/broken.md.".to_owned(),
                vec![open("Notes/broken.md")],
            ),
        ]
    );
}
