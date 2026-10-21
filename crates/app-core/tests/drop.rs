//! Seam 1: files dragged in from the file manager and dropped on the tree.

mod support;

use std::{path::PathBuf, time::Instant};

use app_core::{App, DropSpot, NoticeKind, SPRING_OPEN_DELAY};
use support::{Fixture, current, note, notice, object_id, rows, write};

fn row_of(app: &App, relative_path: &str) -> usize {
    app.tree()
        .unwrap()
        .find_row(relative_path)
        .unwrap_or_else(|| panic!("{relative_path} isn't visible"))
}

/// Hover `paths` over `spot` and drop them there, as the tree widget does.
fn drop_on(app: &mut App, paths: &[PathBuf], spot: DropSpot) {
    app.tree_mut().unwrap().files_over(spot, Instant::now());
    app.drop_files(paths);
}

#[test]
fn files_dropped_on_a_folder_row_copy_into_it_and_the_originals_stay() {
    let fixture = Fixture::new(&[("Docs/Guide.md", b"g"), ("Readme.md", b"r")]);
    let outside = fixture.outside();
    write(&outside, "scan.pdf", b"%PDF");
    write(&outside, "Photos/cat.jpg", b"jpg");
    let mut app = fixture.app();
    let docs = DropSpot::Row(row_of(&app, "Docs"));

    drop_on(
        &mut app,
        &[outside.join("scan.pdf"), outside.join("Photos")],
        docs,
    );

    assert_eq!(fixture.read("Docs/scan.pdf"), b"%PDF");
    assert_eq!(fixture.read("Docs/Photos/cat.jpg"), b"jpg");
    assert_eq!(std::fs::read(outside.join("scan.pdf")).unwrap(), b"%PDF");
    assert_eq!(
        std::fs::read(outside.join("Photos/cat.jpg")).unwrap(),
        b"jpg"
    );
}

#[test]
fn files_dropped_on_a_file_row_copy_into_its_folder_and_on_blank_space_to_the_top() {
    let fixture = Fixture::new(&[("Docs/Guide.md", b"g"), ("Readme.md", b"r")]);
    let outside = fixture.outside();
    write(&outside, "scan.pdf", b"%PDF");
    write(&outside, "map.jpg", b"jpg");
    let mut app = fixture.app();
    support::select(&mut app, "Docs/Guide.md");
    let guide = DropSpot::Row(row_of(&app, "Docs/Guide.md"));

    drop_on(&mut app, &[outside.join("scan.pdf")], guide);
    drop_on(&mut app, &[outside.join("map.jpg")], DropSpot::Blank);

    assert_eq!(fixture.read("Docs/scan.pdf"), b"%PDF");
    assert_eq!(fixture.read("map.jpg"), b"jpg");
}

#[test]
fn files_resting_on_a_closed_folder_open_it_and_leaving_the_window_closes_it_again() {
    let fixture = Fixture::new(&[("Docs/Guide.md", b"g"), ("Readme.md", b"r")]);
    let outside = fixture.outside();
    write(&outside, "scan.pdf", b"%PDF");
    let mut app = fixture.app();
    let docs = DropSpot::Row(row_of(&app, "Docs"));
    let start = Instant::now();

    let tree = app.tree_mut().unwrap();
    tree.files_over(docs, start);
    assert_eq!(tree.drop_target(), Some("Docs"));
    assert_eq!(rows(&app), ["Docs", "Readme"]);
    let tree = app.tree_mut().unwrap();
    tree.files_over(docs, start + SPRING_OPEN_DELAY);
    assert_eq!(rows(&app), ["Docs", "Guide", "Readme"]);

    app.tree_mut().unwrap().files_left();
    assert_eq!(app.tree().unwrap().drop_target(), None);
    assert_eq!(rows(&app), ["Docs", "Readme"]);

    // A folder the drag opened stays open when the drop lands in it.
    let tree = app.tree_mut().unwrap();
    tree.files_over(docs, start);
    tree.files_over(docs, start + SPRING_OPEN_DELAY);
    app.drop_files(&[outside.join("scan.pdf")]);
    assert_eq!(rows(&app), ["Docs", "Guide", "scan.pdf", "Readme"]);
}

#[test]
fn a_drop_never_overwrites_and_a_taken_name_gets_the_next_number() {
    let fixture = Fixture::new(&[
        ("Projects/Plan.md", b"mine"),
        ("Projects/Trip/Day.md", b"day"),
    ]);
    let outside = fixture.outside();
    write(&outside, "Plan.md", b"dropped");
    write(&outside, "Trip/Map.png", b"map");
    let mut app = fixture.app();
    let projects = DropSpot::Row(row_of(&app, "Projects"));

    drop_on(
        &mut app,
        &[outside.join("Plan.md"), outside.join("Trip")],
        projects,
    );

    assert_eq!(fixture.read("Projects/Plan.md"), b"mine");
    assert_eq!(fixture.read("Projects/Plan 1.md"), b"dropped");
    assert_eq!(fixture.read("Projects/Trip/Day.md"), b"day");
    assert_eq!(fixture.read("Projects/Trip 1/Map.png"), b"map");
}

#[test]
fn folders_copy_whole_notes_get_a_new_id_only_when_theirs_is_taken_and_other_files_keep_their_bytes()
 {
    let taken = note("01jbcdefghjkmnpqrstvwxyz00", "Mine\n");
    let fresh = note("01jbcdefghjkmnpqrstvwxyz11", "From elsewhere\n");
    let bytes = b"%PDF-1.7\n\xff\xfe\r\nbinary".to_vec();
    let fixture = Fixture::new(&[("Brief.md", &taken)]);
    let outside = fixture.outside();
    write(&outside, "Trip/Days/Monday/Brief.md", &taken);
    write(&outside, "Trip/Days/Monday/Fresh.md", &fresh);
    write(&outside, "Trip/scan.pdf", &bytes);
    write(&outside, "Trip/Plain.md", b"# Plain\r\nno id\r\n");
    let mut app = fixture.app();

    drop_on(&mut app, &[outside.join("Trip")], DropSpot::Blank);

    let copy = fixture.read("Trip/Days/Monday/Brief.md");
    assert_ne!(object_id(&copy), object_id(&taken));
    assert!(String::from_utf8_lossy(&copy).contains("Mine"));
    assert_eq!(fixture.read("Trip/Days/Monday/Fresh.md"), fresh);
    assert_eq!(fixture.read("Trip/scan.pdf"), bytes);
    assert_eq!(fixture.read("Trip/Plain.md"), b"# Plain\r\nno id\r\n");
    assert_eq!(fixture.read("Brief.md"), taken);
}

#[test]
fn every_copied_file_is_on_disk_before_the_drop_returns() {
    let fixture = Fixture::new(&[("Docs/Guide.md", b"g")]);
    let outside = fixture.outside();
    write(&outside, "scan.pdf", b"%PDF");
    write(&outside, "Trip/Days/notes.txt", b"beach");
    let mut app = fixture.app();
    let docs = DropSpot::Row(row_of(&app, "Docs"));

    let ((), synced) = local_core::durability_probe::record(|| {
        drop_on(
            &mut app,
            &[outside.join("scan.pdf"), outside.join("Trip")],
            docs,
        );
    });

    // Syncing a folder makes the entries in it durable: the copied files
    // and the new folders that hold them.
    for folder in ["Docs", "Docs/Trip", "Docs/Trip/Days"] {
        assert!(
            synced.contains(&fixture.root.join(folder)),
            "{folder} wasn't synced; synced: {synced:?}"
        );
    }
}

#[test]
fn a_drop_skips_hidden_files_and_links_and_says_what_it_copied() {
    let fixture = Fixture::new(&[("Projects/Plan.md", b"p")]);
    let outside = fixture.outside();
    write(&outside, "a.txt", b"a");
    write(&outside, "b.txt", b"b");
    write(&outside, "Trip/.DS_Store", b"junk");
    write(&outside, "Trip/map.jpg", b"map");
    let mut app = fixture.app();
    let projects = DropSpot::Row(row_of(&app, "Projects"));

    drop_on(
        &mut app,
        &[
            outside.join("a.txt"),
            outside.join("b.txt"),
            outside.join("Trip"),
        ],
        projects,
    );

    assert!(!fixture.exists("Projects/Trip/.DS_Store"));
    assert_eq!(fixture.read("Projects/Trip/map.jpg"), b"map");
    assert_eq!(
        notice(&app),
        Some((
            "Copied 3 items to \u{201c}Projects\u{201d}".to_owned(),
            "Skipped 1 hidden file or link.".to_owned()
        ))
    );
    assert_eq!(app.notice().unwrap().kind, NoticeKind::Done);
    assert_eq!(current(&app).as_deref(), Some("Projects/a.txt"));
}

#[test]
fn one_copied_item_is_named_and_a_drop_with_nothing_to_copy_says_so() {
    let fixture = Fixture::new(&[("Readme.md", b"r")]);
    let outside = fixture.outside();
    write(&outside, "Plan.md", b"p");
    write(&outside, ".env", b"secret");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.join("Plan.md"), outside.join("Link.md")).unwrap();
    let mut app = fixture.app();

    drop_on(&mut app, &[outside.join("Plan.md")], DropSpot::Blank);
    assert_eq!(
        notice(&app),
        Some((
            "Copied \u{201c}Plan\u{201d} to the top of the workspace".to_owned(),
            String::new()
        ))
    );

    let mut hidden = vec![outside.join(".env")];
    if cfg!(unix) {
        hidden.push(outside.join("Link.md"));
    }
    drop_on(&mut app, &hidden, DropSpot::Blank);
    let skipped = match hidden.len() {
        1 => "Skipped 1 hidden file or link.",
        _ => "Skipped 2 hidden files or links.",
    };
    assert_eq!(
        notice(&app),
        Some(("Nothing to copy".to_owned(), skipped.to_owned()))
    );
    assert!(!fixture.exists(".env"));
    assert!(!fixture.exists("Link.md"));
}

/// Linux file systems take any bytes as a name; macOS and Windows refuse
/// names that aren't valid Unicode.
#[cfg(target_os = "linux")]
#[test]
fn a_drop_skips_names_that_arent_utf8() {
    use std::os::unix::ffi::OsStrExt;
    let fixture = Fixture::new(&[("Readme.md", b"r")]);
    let outside = fixture.outside();
    write(&outside, "Trip/map.jpg", b"map");
    let odd = outside.join(std::ffi::OsStr::from_bytes(b"Trip/caf\xe9.txt"));
    std::fs::write(&odd, b"odd").unwrap();
    let mut app = fixture.app();

    drop_on(&mut app, &[outside.join("Trip")], DropSpot::Blank);

    assert_eq!(fixture.read("Trip/map.jpg"), b"map");
    assert_eq!(
        std::fs::read_dir(fixture.root.join("Trip"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(notice(&app).unwrap().1, "Skipped 1 hidden file or link.");
}

#[test]
fn dropping_a_folder_that_holds_the_workspace_or_the_target_is_refused_and_copies_nothing() {
    let fixture = Fixture::new(&[("Docs/Deep/Plan.md", b"p")]);
    let outside = fixture.outside();
    write(&outside, "first.txt", b"1");
    let mut app = fixture.app();
    support::select(&mut app, "Docs/Deep/Plan.md");
    let before = fixture.snapshot();
    let holder = fixture.root.parent().unwrap().to_owned();

    for (sources, target, refusal) in [
        (
            vec![outside.join("first.txt"), holder],
            "Docs",
            "That folder holds this workspace, so Noura can't copy it here.",
        ),
        (
            vec![outside.join("first.txt"), fixture.root.join("Docs")],
            "Docs/Deep",
            "Noura can't copy a folder into itself.",
        ),
        (
            vec![fixture.root.join("Docs")],
            "Docs",
            "Noura can't copy a folder into itself.",
        ),
    ] {
        let spot = DropSpot::Row(row_of(&app, target));
        drop_on(&mut app, &sources, spot);

        assert_eq!(
            notice(&app),
            Some((
                "Couldn't copy the dropped files".to_owned(),
                refusal.to_owned()
            )),
            "dropped into {target}"
        );
        assert_eq!(app.notice().unwrap().kind, NoticeKind::Problem);
        assert_eq!(fixture.snapshot(), before);
    }
}

#[test]
fn files_dropped_outside_the_tree_copy_nothing() {
    let fixture = Fixture::new(&[("Readme.md", b"r")]);
    let outside = fixture.outside();
    write(&outside, "scan.pdf", b"%PDF");
    let mut app = fixture.app();
    let before = fixture.snapshot();

    drop_on(&mut app, &[outside.join("scan.pdf")], DropSpot::Away);

    assert_eq!(fixture.snapshot(), before);
    assert_eq!(notice(&app), None);
}
