//! Image tabs and the default app, seam 1: images in the formats notes
//! draw inline open in an image tab with the file's bytes, which follow
//! the file when another app replaces it, and any other file goes to the
//! system's default app with no tab.

use crate::support;
use std::time::{Duration, Instant};

use app_core::{
    App, CommandSource, IMAGE_EXTENSIONS, IMAGE_ZOOM_STEPS, ImageState, ImageZoom, OpenAs, TabId,
    TreeKey, command_id,
};
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
    #[cfg_attr(not(unix), allow(unused_mut))]
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
                app.open_palette(app_core::SearchScope::Names);
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
            "No app on this computer opens this file.".to_owned()
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

/// The scale the image in `tab` draws at.
fn scale(app: &App, tab: TabId) -> f32 {
    app.image(tab).unwrap().scale()
}

fn close(found: [f32; 2], expected: [f32; 2]) -> bool {
    (found[0] - expected[0]).abs() < 0.01 && (found[1] - expected[1]).abs() < 0.01
}

#[test]
fn an_image_tab_fits_its_image_then_zooms_in_steps_and_back_to_fit() {
    let fixture = Fixture::new(&[("wide.png", b"image"), ("icon.png", b"image")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "wide.png");
    assert_eq!(app.image(tab).unwrap().zoom(), ImageZoom::Fit);

    // 2,000 by 1,000 points in a view of 800 by 600: it fits at 40%,
    // centered down the view.
    app.size_image(tab, [2000.0, 1000.0], [800.0, 600.0], 1.0);
    let image = app.image(tab).unwrap();
    assert!((image.scale() - 0.4).abs() < 0.001);
    assert!(close(image.drawn_size().unwrap(), [800.0, 400.0]));
    assert!(close(image.image_origin(), [0.0, 100.0]));

    app.image_zoom_in(tab);
    assert_eq!(app.image(tab).unwrap().zoom(), ImageZoom::Scale(0.5));
    app.image_zoom_in(tab);
    assert_eq!(app.image(tab).unwrap().zoom(), ImageZoom::Scale(0.67));
    // Zoom Out steps back down, and stops at Fit on the way past it.
    app.image_zoom_out(tab);
    app.image_zoom_out(tab);
    assert_eq!(app.image(tab).unwrap().zoom(), ImageZoom::Fit);
    app.image_zoom_out(tab);
    assert_eq!(app.image(tab).unwrap().zoom(), ImageZoom::Scale(0.33));
    app.image_actual_size(tab);
    assert_eq!(scale(&app, tab), 1.0);
    // Zoom In stops at its largest step.
    for _ in 0..40 {
        app.image_zoom_in(tab);
    }
    assert_eq!(
        scale(&app, tab),
        IMAGE_ZOOM_STEPS[IMAGE_ZOOM_STEPS.len() - 1]
    );

    // An image smaller than its view fits at its own size, centered.
    let icon = open_from_tree(&mut app, "icon.png");
    app.size_image(icon, [64.0, 32.0], [800.0, 600.0], 1.0);
    let image = app.image(icon).unwrap();
    assert_eq!(image.scale(), 1.0);
    assert!(close(image.image_origin(), [368.0, 284.0]));
}

#[test]
fn zooming_keeps_the_middle_of_the_view_and_a_pinch_the_point_under_the_pointer() {
    let fixture = Fixture::new(&[("map.png", b"image")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "map.png");
    // 1,000 points square in a view of 500: it fits at 50%.
    app.size_image(tab, [1000.0, 1000.0], [500.0, 500.0], 1.0);
    app.image_scrolled(tab, [0.0, 0.0], 0);

    // A pinch to twice the size at 100, 100 keeps the image's point 200,
    // 200 there: the view scrolls to 100, 100.
    app.image_zoom_by(tab, 2.0, [100.0, 100.0]);
    let image = app.image(tab).unwrap();
    assert_eq!(image.zoom(), ImageZoom::Scale(1.0));
    let scroll = image.scroll().expect("the view scrolls");
    assert!(close(scroll.offset, [100.0, 100.0]), "{scroll:?}");

    // What the client shows before it makes the scroll doesn't count.
    app.image_scrolled(tab, [0.0, 0.0], scroll.seq - 1);
    assert_eq!(app.image(tab).unwrap().scroll(), Some(scroll));
    app.image_scrolled(tab, scroll.offset, scroll.seq);
    assert_eq!(app.image(tab).unwrap().scroll(), None);

    // Zoom In keeps the image's point at the view's middle, 350, 350.
    app.image_zoom_in(tab);
    let image = app.image(tab).unwrap();
    assert_eq!(image.zoom(), ImageZoom::Scale(1.1));
    assert!(close(image.scroll().unwrap().offset, [135.0, 135.0]));

    // A pinch far out stops at the smallest step and shows it centered.
    app.image_zoom_by(tab, 0.0001, [250.0, 250.0]);
    let image = app.image(tab).unwrap();
    assert_eq!(image.scale(), IMAGE_ZOOM_STEPS[0]);
    assert!(close(image.scroll().unwrap().offset, [0.0, 0.0]));
}

#[test]
fn zoom_commands_zoom_the_image_of_an_image_tab_and_the_interface_elsewhere() {
    let fixture = Fixture::new(&[("map.png", b"image"), ("Plan.md", b"# Plan\n")]);
    let mut app = fixture.app();
    app.open_file("map.png", OpenAs::Kept);
    let tab = app.active_tab().unwrap().id();
    app.size_image(tab, [2000.0, 1000.0], [800.0, 600.0], 1.0);
    let at = Instant::now();

    assert!(app.run_command(command_id::ZOOM_IN, CommandSource::Menu, at));
    assert_eq!(app.image(tab).unwrap().zoom(), ImageZoom::Scale(0.5));
    assert!(app.run_command(command_id::ZOOM_RESET, CommandSource::Menu, at));
    assert_eq!(app.image(tab).unwrap().zoom(), ImageZoom::Scale(1.0));
    assert!(app.run_command(command_id::ZOOM_OUT, CommandSource::Menu, at));
    assert_eq!(app.image(tab).unwrap().zoom(), ImageZoom::Scale(0.9));
    assert_eq!(app.preferences().zoom, 100, "the interface kept its zoom");

    app.open_file("Plan.md", OpenAs::Kept);
    assert!(app.run_command(command_id::ZOOM_IN, CommandSource::Menu, at));
    assert_eq!(app.preferences().zoom, 110);
    assert!(app.run_command(command_id::ZOOM_RESET, CommandSource::Menu, at));
    assert_eq!(app.preferences().zoom, 100);
    assert_eq!(app.image(tab).unwrap().zoom(), ImageZoom::Scale(0.9));
}

#[test]
fn the_interface_zoom_leaves_an_images_size_on_the_screen_alone() {
    let fixture = Fixture::new(&[("map.png", b"image")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "map.png");
    // At a 125% interface zoom each of the client's points covers 1.25 of
    // the screen's, so a view 800 by 600 screen points measures 640 by 480.
    app.size_image(tab, [2000.0, 1000.0], [640.0, 480.0], 1.25);
    app.image_scrolled(tab, [0.0, 0.0], 0);
    // Fit still fills the tab: 40% of the image's size on the screen.
    let image = app.image(tab).unwrap();
    assert!((image.magnification() - 0.4).abs() < 0.001);
    assert!(close(image.drawn_size().unwrap(), [640.0, 320.0]));

    // Actual Size shows each of the image's points on one of the screen's.
    app.image_actual_size(tab);
    let image = app.image(tab).unwrap();
    assert_eq!(image.magnification(), 1.0);
    assert!(close(image.drawn_size().unwrap(), [1600.0, 800.0]));
    let scroll = image.scroll().unwrap();
    assert!(close(scroll.offset, [480.0, 160.0]), "{scroll:?}");
    app.image_scrolled(tab, scroll.offset, scroll.seq);

    // Back at 100%, the image keeps its size on the screen, and the
    // image's point at the middle of the view stays there.
    app.size_image(tab, [2000.0, 1000.0], [800.0, 600.0], 1.0);
    let image = app.image(tab).unwrap();
    assert_eq!(image.zoom(), ImageZoom::Scale(1.0));
    assert!(close(image.drawn_size().unwrap(), [2000.0, 1000.0]));
    assert!(close(image.scroll().unwrap().offset, [600.0, 200.0]));
}
