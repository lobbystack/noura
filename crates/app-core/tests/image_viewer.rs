//! Image tabs and the default app, seam 1: images in the formats notes
//! draw inline open in an image tab with the file's bytes, which follow
//! the file when another app replaces it, and any other file goes to the
//! system's default app with no tab.

mod support;

use std::time::{Duration, Instant};

use app_core::{App, IMAGE_EXTENSIONS, ImageState, OpenAs, TabId, TreeKey};
use support::{Call, FakeSystem, Fixture, open_from_tree};

/// Poll until `done` holds.
fn wait_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll();
        if done(app) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The bytes the image tab `tab` shows, once it shows them.
fn shown(app: &App, tab: TabId) -> Option<Vec<u8>> {
    match app.image(tab)?.state() {
        ImageState::Shown { bytes, .. } => Some(bytes.to_vec()),
        _ => None,
    }
}

#[test]
fn an_image_opens_in_an_image_tab_with_the_files_bytes() {
    let fixture = Fixture::new(&[("Photos/cat.png", b"\x89PNG\r\n\x1a\ncat")]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());

    let tab = open_from_tree(&mut app, "Photos/cat.png");

    let image = app.image(tab).expect("the tab shows an image");
    assert_eq!((image.path(), image.name()), ("Photos/cat.png", "cat.png"));
    wait_until(&mut app, "the image is read", |app| {
        shown(app, tab).is_some()
    });
    assert_eq!(shown(&app, tab).unwrap(), b"\x89PNG\r\n\x1a\ncat");
    assert!(app.note(tab).is_none());
    assert!(app.pdf(tab).is_none());
    assert_eq!(system.calls(), []);
}

#[test]
fn every_format_a_note_draws_inline_opens_in_an_image_tab() {
    let files: Vec<(String, Vec<u8>)> = IMAGE_EXTENSIONS
        .iter()
        .flat_map(|extension| {
            [
                (format!("pic.{extension}"), b"image".to_vec()),
                (
                    format!("Big.{}", extension.to_uppercase()),
                    b"image".to_vec(),
                ),
            ]
        })
        .collect();
    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    let fixture = Fixture::new(&borrowed);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());

    for (path, _) in &files {
        app.open_file(path, OpenAs::Kept);
        let tab = app.active_tab().unwrap();
        assert_eq!(tab.file().unwrap().relative_path, *path);
        assert!(app.image(tab.id()).is_some(), "{path}");
    }
    assert_eq!(system.calls(), []);
}

#[test]
fn any_other_file_goes_to_the_default_app_and_opens_no_tab() {
    let fixture = Fixture::new(&[
        ("Plan.md", b"# Plan\n"),
        ("archive.zip", b"PK"),
        ("Talks/Slides.key", b"key"),
        ("song.mp3", b"ID3"),
    ]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    let note = open_from_tree(&mut app, "Plan.md");

    support::select(&mut app, "archive.zip");
    app.press_tree_key(TreeKey::Enter);
    app.open_file("Talks/Slides.key", OpenAs::Kept);
    app.open_file("song.mp3", OpenAs::Preview);

    let tabs: Vec<TabId> = app.tabs().iter().map(app_core::Tab::id).collect();
    assert_eq!(tabs, [note]);
    assert_eq!(app.active_tab().unwrap().id(), note);
    assert_eq!(
        system.calls(),
        [
            Call::OpenFile(fixture.root.join("archive.zip")),
            Call::OpenFile(fixture.root.join("Talks/Slides.key")),
            Call::OpenFile(fixture.root.join("song.mp3")),
        ]
    );
    assert_eq!(app.notice(), None);
}

/// Files whose default app would run them, as a workspace someone else
/// wrote could hold them, beside a note that links to each.
const RUNNABLE: &[&str] = &[
    "install.command",
    "Setup/setup.EXE",
    "run.bat",
    "tool.jar",
    "Profile.mobileconfig",
];

/// A way to open a file, and what it does.
type Way<'a> = (&'a str, &'a dyn Fn(&mut App));

#[test]
fn an_app_or_script_never_goes_to_the_default_app() {
    let links: String = RUNNABLE
        .iter()
        .chain(&["script"])
        .map(|path| format!("[[{path}]]\n"))
        .collect();
    let mut files: Vec<(&str, &[u8])> = RUNNABLE
        .iter()
        .map(|path| (*path, b"echo hi\n".as_slice()))
        .collect();
    files.push(("script", b"#!/bin/sh\necho hi\n"));
    files.push(("Links.md", links.as_bytes()));
    let fixture = Fixture::new(&files);
    let mut runnable: Vec<&str> = RUNNABLE.to_vec();
    #[cfg(unix)]
    {
        // An extensionless file anyone may execute, which macOS runs in
        // Terminal.
        use std::os::unix::fs::PermissionsExt;
        let script = fixture.root.join("script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        runnable.push("script");
    }
    let system = FakeSystem::on(app_core::Platform::MacOs);
    let mut app = fixture.app_with(system.clone());
    let note = open_from_tree(&mut app, "Links.md");
    app.keep_tab(note);

    for path in &runnable {
        let name = path.rsplit('/').next().unwrap();
        let refused = Some((
            format!("Couldn't open \u{201c}{name}\u{201d}"),
            "Noura doesn't open apps or scripts. Show the file in Finder and open it there."
                .to_owned(),
        ));
        let ways: [Way; 7] = [
            ("Enter in the tree", &|app| {
                support::select(app, path);
                app.press_tree_key(TreeKey::Enter);
            }),
            ("Space in the tree", &|app| {
                support::select(app, path);
                app.press_tree_key(TreeKey::Activate);
            }),
            ("open_file kept", &|app| app.open_file(path, OpenAs::Kept)),
            ("open_file preview", &|app| {
                app.open_file(path, OpenAs::Preview)
            }),
            ("a link", &|app| {
                app.follow_note_link(note, name, OpenAs::Preview)
            }),
            ("a link to a new tab", &|app| {
                app.follow_note_link(note, path, OpenAs::Kept);
            }),
            ("quick open", &|app| {
                app.open_quick_open();
                app.set_palette_query(name);
                // No document opens, so the pick reports false.
                assert!(!app.run_palette_selection(Instant::now()));
                assert!(app.palette().is_none());
            }),
        ];
        for (way, open) in ways {
            app.activate_tab(note);
            app.dismiss_notice();
            open(&mut app);
            assert_eq!(support::notice(&app), refused, "{path} by {way}");
            assert_eq!(app.active_tab().unwrap().id(), note, "{path} by {way}");
        }
    }

    // A duplicate stays in the tree and opens nowhere either.
    app.duplicate("install.command");
    assert!(fixture.exists("install 1.command"));
    assert_eq!(app.tabs().len(), 1);
    assert_eq!(system.calls(), []);
}

#[test]
fn the_refusal_names_the_file_manager_of_each_platform() {
    for (platform, file_manager) in [
        (app_core::Platform::MacOs, "Finder"),
        (app_core::Platform::Windows, "Explorer"),
        (app_core::Platform::Linux, "your file manager"),
    ] {
        let fixture = Fixture::new(&[("setup.exe", b"MZ")]);
        let system = FakeSystem::on(platform);
        let mut app = fixture.app_with(system.clone());

        app.open_file("setup.exe", OpenAs::Preview);

        assert_eq!(
            support::notice(&app).map(|(_, message)| message),
            Some(format!(
                "Noura doesn't open apps or scripts. Show the file in {file_manager} and open it \
                 there."
            ))
        );
        assert_eq!(system.calls(), []);
    }
}

#[test]
fn a_click_only_selects_a_file_that_opens_in_no_tab_and_enter_or_space_opens_it_once() {
    let fixture = Fixture::new(&[("Plan.md", b"# Plan\n"), ("archive.zip", b"PK")]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    let note = open_from_tree(&mut app, "Plan.md");
    let archive = app
        .tree()
        .unwrap()
        .rows()
        .iter()
        .position(|row| row.relative_path == "archive.zip")
        .unwrap();

    // A click, a Cmd-click and the clicks of a double-click.
    app.click_tree_row(archive, OpenAs::Preview);
    app.click_tree_row(archive, OpenAs::Kept);

    assert_eq!(app.tree().unwrap().focused(), Some(archive));
    assert_eq!(app.active_tab().unwrap().id(), note);
    assert_eq!(system.calls(), []);

    app.press_tree_key(TreeKey::Enter);
    app.press_tree_key(TreeKey::Activate);

    let opened = Call::OpenFile(fixture.root.join("archive.zip"));
    assert_eq!(system.calls(), [opened.clone(), opened]);
    let tabs: Vec<TabId> = app.tabs().iter().map(app_core::Tab::id).collect();
    assert_eq!(tabs, [note]);
}

#[test]
fn a_file_no_app_opens_says_so_and_opens_no_tab() {
    let fixture = Fixture::new(&[("archive.zip", b"PK")]);
    // Without operating system services, no app opens anything.
    let mut app = App::launch(fixture.app_data.path());
    support::settle(&mut app);

    app.open_file("archive.zip", OpenAs::Preview);

    assert!(app.tabs().is_empty());
    assert_eq!(
        support::notice(&app),
        Some((
            "Couldn't open \u{201c}archive.zip\u{201d}".to_owned(),
            "Noura couldn't find an app on this computer that opens this file.".to_owned()
        ))
    );
}

#[test]
fn a_saved_tab_of_a_file_that_opens_in_no_tab_stays_closed_at_the_next_launch() {
    let fixture = Fixture::new(&[("Plan.md", b"# Plan\n"), ("archive.zip", b"PK")]);
    let workspace_id = fixture.app().workspace().unwrap().id().to_owned();
    // A tab an older version saved for the archive, beside the note's.
    let view_state = fixture
        .app_data
        .path()
        .join("workspaces")
        .join(&workspace_id)
        .join("view-state.json");
    std::fs::write(
        &view_state,
        r#"{"version":2,"windows":[{"tabs":[{"file":"archive.zip"},{"file":"Plan.md"}],"active":0,"expandedFolders":[]}],"railOrder":[]}"#,
    )
    .unwrap();
    let system = FakeSystem::new();

    let app = fixture.app_with(system.clone());

    let paths: Vec<&str> = app
        .tabs()
        .iter()
        .filter_map(|tab| tab.file().map(|file| file.relative_path.as_str()))
        .collect();
    assert_eq!(paths, ["Plan.md"]);
    assert_eq!(
        app.active_tab()
            .and_then(|tab| tab.file())
            .map(|file| file.relative_path.as_str()),
        Some("Plan.md")
    );
    assert_eq!(system.calls(), []);
}

#[test]
fn replacing_an_open_image_on_disk_shows_the_new_file() {
    let fixture = Fixture::new(&[("map.png", b"first map")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "map.png");
    wait_until(&mut app, "the image is read", |app| {
        shown(app, tab).as_deref() == Some(b"first map")
    });

    support::write(&fixture.root, "map.png", b"the second, longer map");

    wait_until(&mut app, "the new image shows", |app| {
        shown(app, tab).as_deref() == Some(b"the second, longer map")
    });
}

#[test]
fn an_image_larger_than_noura_shows_says_so() {
    let big = vec![0u8; usize::try_from(app_core::MAX_IMAGE_BYTES).unwrap() + 1];
    let fixture = Fixture::new(&[("huge.png", &big)]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "huge.png");

    wait_until(&mut app, "the image fails", |app| {
        matches!(app.image(tab).unwrap().state(), ImageState::Failed { .. })
    });
    assert_eq!(
        app.image(tab).unwrap().state(),
        &ImageState::Failed {
            message: "This image is larger than 20 MB, the most Noura shows.".to_owned()
        }
    );
}
