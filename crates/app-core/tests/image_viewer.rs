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
