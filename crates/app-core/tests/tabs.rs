//! Seam 1: tabs over a real workspace folder.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use app_core::{App, OpenAs, TabPlace, ViewPlace};
use tempfile::TempDir;

/// Poll until background work finishes.
fn settle(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.poll();
        if !app.is_busy() {
            return;
        }
        assert!(Instant::now() < deadline, "background work never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn write(root: &Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A workspace holding `files`, opened in a fresh app.
fn opened(files: &[&str]) -> (TempDir, TempDir, App) {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    for file in files {
        write(folder.path(), file, b"x");
    }
    let mut app = App::launch(app_data.path());
    app.open_folder(folder.path().to_owned());
    settle(&mut app);
    (app_data, folder, app)
}

/// Each tab's title, in order.
fn titles(app: &App) -> Vec<String> {
    app.tabs()
        .iter()
        .map(|tab| tab.title().to_owned())
        .collect()
}

fn active(app: &App) -> Option<String> {
    app.active_tab().map(|tab| tab.title().to_owned())
}

#[test]
fn opening_a_file_opens_a_tab_and_opening_it_again_switches_to_it() {
    let (_app_data, _folder, mut app) = opened(&["A.md", "B.md"]);

    app.open_file("A.md", OpenAs::Kept);
    app.open_file("B.md", OpenAs::Kept);
    assert_eq!(titles(&app), ["A", "B"]);
    assert_eq!(active(&app).as_deref(), Some("B"));

    app.open_file("A.md", OpenAs::Kept);

    assert_eq!(titles(&app), ["A", "B"]);
    assert_eq!(active(&app).as_deref(), Some("A"));
    assert_eq!(app.document().unwrap().relative_path, "A.md");
}

fn tab_id(app: &App, title: &str) -> app_core::TabId {
    app.tabs()
        .iter()
        .find(|tab| tab.title() == title)
        .unwrap_or_else(|| panic!("no tab named {title}"))
        .id()
}

#[test]
fn closing_the_active_tab_activates_the_tab_that_takes_its_place() {
    let (_app_data, _folder, mut app) = opened(&["A.md", "B.md", "C.md"]);
    for file in ["A.md", "B.md", "C.md"] {
        app.open_file(file, OpenAs::Kept);
    }
    app.open_file("B.md", OpenAs::Kept);

    app.close_tab(tab_id(&app, "B"));
    assert_eq!(titles(&app), ["A", "C"]);
    assert_eq!(active(&app).as_deref(), Some("C"));

    app.close_tab(tab_id(&app, "C"));
    assert_eq!(titles(&app), ["A"]);
    assert_eq!(active(&app).as_deref(), Some("A"));
}

#[test]
fn closing_a_tab_in_the_background_keeps_the_active_tab() {
    let (_app_data, _folder, mut app) = opened(&["A.md", "B.md"]);
    app.open_file("A.md", OpenAs::Kept);
    app.open_file("B.md", OpenAs::Kept);

    app.close_tab(tab_id(&app, "A"));

    assert_eq!(titles(&app), ["B"]);
    assert_eq!(active(&app).as_deref(), Some("B"));
}

/// Each tab's title, with "(preview)" after the preview tab's.
fn shown(app: &App) -> Vec<String> {
    app.tabs()
        .iter()
        .map(|tab| {
            if tab.is_preview() {
                format!("{} (preview)", tab.title())
            } else {
                tab.title().to_owned()
            }
        })
        .collect()
}

#[test]
fn a_preview_tab_is_replaced_by_the_next_preview_in_place() {
    let (_app_data, _folder, mut app) = opened(&["A.md", "B.md", "C.md"]);
    app.open_file("A.md", OpenAs::Preview);
    assert_eq!(shown(&app), ["A (preview)"]);
    app.open_file("C.md", OpenAs::Kept);

    app.activate_tab(tab_id(&app, "A"));
    app.open_file("B.md", OpenAs::Preview);

    assert_eq!(shown(&app), ["B (preview)", "C"]);
    assert_eq!(active(&app).as_deref(), Some("B"));
}

#[test]
fn a_preview_of_a_file_with_a_tab_switches_to_that_tab() {
    let (_app_data, _folder, mut app) = opened(&["A.md", "B.md"]);
    app.open_file("A.md", OpenAs::Kept);
    app.open_file("B.md", OpenAs::Preview);

    app.open_file("A.md", OpenAs::Preview);

    assert_eq!(shown(&app), ["A", "B (preview)"]);
    assert_eq!(active(&app).as_deref(), Some("A"));
}

#[test]
fn a_double_click_keeps_the_preview_tab() {
    let (_app_data, _folder, mut app) = opened(&["A.md", "B.md"]);
    // A double-click's first click opens the preview; the second keeps it.
    app.open_file("A.md", OpenAs::Preview);
    app.open_file("A.md", OpenAs::Kept);

    app.open_file("B.md", OpenAs::Preview);

    assert_eq!(shown(&app), ["A", "B (preview)"]);
}

#[test]
fn the_first_edit_keeps_the_preview_tab() {
    let (_app_data, _folder, mut app) = opened(&["A.md", "B.md"]);
    app.open_file("A.md", OpenAs::Preview);

    app.keep_tab(tab_id(&app, "A"));
    app.open_file("B.md", OpenAs::Preview);

    assert_eq!(shown(&app), ["A", "B (preview)"]);
}

#[test]
fn new_notes_and_duplicates_open_as_kept_tabs_beside_the_preview() {
    let (_app_data, _folder, mut app) = opened(&["A.md", "Plan.md", "Plan 1.md"]);
    app.open_file("A.md", OpenAs::Preview);

    // New note and Duplicate (ticket 08) open their file this way.
    app.open_file("Plan 1.md", OpenAs::Kept);

    assert_eq!(shown(&app), ["A (preview)", "Plan 1"]);
    assert_eq!(active(&app).as_deref(), Some("Plan 1"));
}

/// Until the Inbox (ticket 16) and plugins (ticket 24) add pages, tests
/// register their own.
fn with_pages(app: &mut App) {
    app.register_page("test-alpha", "Alpha");
    app.register_page("test-beta", "Beta");
}

#[test]
fn a_rail_click_opens_a_page_in_the_preview_tab_and_reuses_its_tab() {
    let (_app_data, _folder, mut app) = opened(&["A.md"]);
    with_pages(&mut app);
    app.open_file("A.md", OpenAs::Kept);

    app.open_page("test-alpha", OpenAs::Preview);
    assert_eq!(shown(&app), ["A", "Alpha (preview)"]);
    assert_eq!(app.current_page(), "test-alpha");
    assert!(app.document().is_none());

    app.activate_tab(tab_id(&app, "A"));
    app.open_page("test-alpha", OpenAs::Preview);
    assert_eq!(shown(&app), ["A", "Alpha (preview)"]);
    assert_eq!(active(&app).as_deref(), Some("Alpha"));

    app.open_page("test-beta", OpenAs::Preview);
    assert_eq!(shown(&app), ["A", "Beta (preview)"]);
}

#[test]
fn open_in_new_tab_on_a_page_keeps_the_current_tab_in_place() {
    let (_app_data, _folder, mut app) = opened(&["A.md"]);
    with_pages(&mut app);
    app.open_file("A.md", OpenAs::Preview);

    app.open_page("test-alpha", OpenAs::Kept);

    assert_eq!(shown(&app), ["A (preview)", "Alpha"]);
    assert_eq!(active(&app).as_deref(), Some("Alpha"));
}

#[test]
fn closing_a_page_tab_does_not_reopen_it() {
    let (_app_data, _folder, mut app) = opened(&["A.md"]);
    with_pages(&mut app);
    app.open_page("test-alpha", OpenAs::Preview);

    app.close_tab(tab_id(&app, "Alpha"));

    assert!(app.tabs().is_empty());
    assert_eq!(app.current_page(), app_core::FILES_PAGE);
}

#[test]
fn files_shows_the_document_tab_you_had_before_a_page_took_over() {
    let (_app_data, _folder, mut app) = opened(&["A.md", "B.md"]);
    with_pages(&mut app);
    app.open_file("A.md", OpenAs::Kept);
    app.open_file("B.md", OpenAs::Kept);
    app.activate_tab(tab_id(&app, "A"));
    app.open_page("test-alpha", OpenAs::Kept);
    assert_eq!(app.current_page(), "test-alpha");

    app.open_page(app_core::FILES_PAGE, OpenAs::Preview);

    assert_eq!(active(&app).as_deref(), Some("A"));
    assert_eq!(app.current_page(), app_core::FILES_PAGE);
    assert_eq!(titles(&app), ["A", "B", "Alpha"]);
}

#[test]
fn files_falls_back_to_the_last_document_tab_or_to_nothing() {
    let (_app_data, _folder, mut app) = opened(&["A.md", "B.md"]);
    with_pages(&mut app);
    app.open_file("A.md", OpenAs::Kept);
    app.open_file("B.md", OpenAs::Kept);
    app.open_page("test-alpha", OpenAs::Kept);
    app.close_tab(tab_id(&app, "B"));
    app.activate_tab(tab_id(&app, "Alpha"));

    app.open_page(app_core::FILES_PAGE, OpenAs::Preview);
    assert_eq!(active(&app).as_deref(), Some("A"));

    app.close_tab(tab_id(&app, "A"));
    app.open_page(app_core::FILES_PAGE, OpenAs::Preview);
    assert_eq!(active(&app), None);
    assert_eq!(titles(&app), ["Alpha"]);
    assert_eq!(app.current_page(), app_core::FILES_PAGE);
}

#[test]
fn switching_tabs_keeps_each_tab_s_place() {
    let (_app_data, _folder, mut app) = opened(&["A.md", "B.md"]);
    with_pages(&mut app);
    app.open_file("A.md", OpenAs::Kept);
    let a = tab_id(&app, "A");
    // A test view reports a selection the way the editor (ticket 17) will.
    let place_a = TabPlace {
        scroll: [0.0, 480.0],
        view: Some(ViewPlace::Selection {
            anchor: 12,
            caret: 30,
        }),
    };
    app.set_tab_place(a, place_a.clone());
    app.open_page("test-alpha", OpenAs::Kept);
    let alpha = tab_id(&app, "Alpha");
    let place_alpha = TabPlace {
        scroll: [0.0, 64.0],
        view: None,
    };
    app.set_tab_place(alpha, place_alpha.clone());

    app.activate_tab(a);
    assert_eq!(app.tab_place(a), place_a);
    app.activate_tab(alpha);
    assert_eq!(app.tab_place(alpha), place_alpha);
}

#[test]
fn a_tab_that_opens_starts_at_the_top() {
    let (_app_data, _folder, mut app) = opened(&["A.md", "B.md"]);
    app.open_file("A.md", OpenAs::Preview);
    let a = tab_id(&app, "A");
    app.set_tab_place(
        a,
        TabPlace {
            scroll: [0.0, 480.0],
            view: None,
        },
    );

    app.open_file("B.md", OpenAs::Preview);
    app.close_tab(tab_id(&app, "B"));
    app.open_file("A.md", OpenAs::Preview);

    assert_eq!(app.tab_place(tab_id(&app, "A")), TabPlace::default());
}

/// Every file under `root` with its bytes.
fn snapshot(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut folders = vec![root.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
            } else {
                files.push((path.clone(), std::fs::read(&path).unwrap()));
            }
        }
    }
    files.sort();
    files
}

/// The JSON files in app data that mention `text`. The index mentions
/// file paths too, but it isn't where view state lives.
fn app_data_files_mentioning(app_data: &Path, text: &str) -> Vec<std::path::PathBuf> {
    snapshot(app_data)
        .into_iter()
        .filter(|(path, bytes)| {
            path.extension()
                .is_some_and(|extension| extension == "json")
                && String::from_utf8_lossy(bytes).contains(text)
        })
        .map(|(path, _)| path)
        .collect()
}

/// Quit by dropping `app`, then launch again on the same app data.
fn relaunch(app: App, app_data: &Path) -> App {
    drop(app);
    launch(app_data)
}

/// Launch on `app_data` with the test pages, and wait for the last
/// workspace to reopen.
fn launch(app_data: &Path) -> App {
    let mut app = App::launch(app_data);
    with_pages(&mut app);
    settle(&mut app);
    app
}

#[test]
fn tabs_come_back_with_the_last_workspace_at_launch() {
    let (app_data, folder, mut app) = opened(&["A.md", "B.md", "Docs/C.md"]);
    with_pages(&mut app);
    let before = snapshot(folder.path());
    app.open_file("A.md", OpenAs::Kept);
    app.open_file("Docs/C.md", OpenAs::Preview);
    app.open_page("test-alpha", OpenAs::Kept);
    app.activate_tab(tab_id(&app, "C"));

    let app = relaunch(app, app_data.path());

    assert_eq!(shown(&app), ["A", "C (preview)", "Alpha"]);
    assert_eq!(active(&app).as_deref(), Some("C"));
    assert_eq!(app.document().unwrap().relative_path, "Docs/C.md");
    assert_eq!(snapshot(folder.path()), before, "the workspace changed");
    assert!(!app_data_files_mentioning(app_data.path(), "Docs/C.md").is_empty());
}

#[test]
fn each_workspace_keeps_its_own_tabs() {
    let (_app_data, _first, mut app) = opened(&["A.md"]);
    let second = tempfile::tempdir().unwrap();
    write(second.path(), "Other.md", b"x");
    app.open_file("A.md", OpenAs::Kept);

    app.open_folder(second.path().to_owned());
    settle(&mut app);
    assert!(app.tabs().is_empty());
    app.open_file("Other.md", OpenAs::Kept);
    let first_id = app.recents()[1].workspace_id.clone();
    app.open_recent(&first_id);
    settle(&mut app);

    assert_eq!(titles(&app), ["A"]);
}

#[test]
fn tabs_whose_file_is_gone_stay_closed_at_launch() {
    let (app_data, folder, mut app) = opened(&["A.md", "B.md", "C.md"]);
    for file in ["A.md", "B.md", "C.md"] {
        app.open_file(file, OpenAs::Kept);
    }
    app.activate_tab(tab_id(&app, "B"));
    drop(app);
    std::fs::remove_file(folder.path().join("B.md")).unwrap();

    let app = launch(app_data.path());

    assert_eq!(titles(&app), ["A", "C"]);
    assert_eq!(active(&app).as_deref(), Some("C"));
}

#[test]
fn stored_tabs_that_point_outside_the_workspace_are_dropped() {
    let (app_data, _folder, mut app) = opened(&["A.md", "B.md"]);
    app.open_file("A.md", OpenAs::Kept);
    app.open_file("B.md", OpenAs::Kept);
    drop(app);
    for path in app_data_files_mentioning(app_data.path(), "B.md") {
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("B.md", "../B.md")).unwrap();
    }

    let app = launch(app_data.path());

    assert_eq!(titles(&app), ["A"]);
}

#[test]
fn unreadable_stored_tabs_are_dropped_without_blocking_the_launch() {
    let (app_data, _folder, mut app) = opened(&["A.md"]);
    app.open_file("A.md", OpenAs::Kept);
    drop(app);
    let stored = app_data_files_mentioning(app_data.path(), "A.md");
    assert!(!stored.is_empty());
    for path in stored {
        std::fs::write(&path, b"{ not json").unwrap();
    }

    let app = launch(app_data.path());

    assert_eq!(app.screen(), &app_core::Screen::Workspace);
    assert!(app.tabs().is_empty());
}

/// A managed note's bytes with the stable ID `id`.
fn managed_note(id: &str) -> Vec<u8> {
    format!(
        "---\nid: {id}\ntype: note\ncreated: 2026-08-01T00:00:00Z\nupdated: \
         2026-08-01T00:00:00Z\n---\n\n# Plan\n\nThe plan.\n"
    )
    .into_bytes()
}

/// Each tab's workspace-relative path, or its page.
fn paths(app: &App) -> Vec<String> {
    app.tabs()
        .iter()
        .map(|tab| match tab.file() {
            Some(file) => file.relative_path.clone(),
            None => format!("page:{}", tab.page_id().unwrap()),
        })
        .collect()
}

fn rename(folder: &Path, from: &str, to: &str) {
    let to = folder.join(to);
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    std::fs::rename(folder.join(from), to).unwrap();
}

#[test]
fn tabs_follow_files_the_tree_renames_and_moves() {
    let (_app_data, folder, mut app) = opened(&["Docs/Guide.md", "Docs/Deep/Notes.md", "B.md"]);
    app.open_file("Docs/Guide.md", OpenAs::Kept);
    app.open_file("Docs/Deep/Notes.md", OpenAs::Kept);
    app.open_file("B.md", OpenAs::Kept);

    // The tree's rename and move (ticket 08) report each move this way.
    rename(folder.path(), "Docs", "Archive/Docs");
    app.item_moved("Docs", "Archive/Docs");
    rename(folder.path(), "B.md", "Archive/Brief.md");
    app.item_moved("B.md", "Archive/Brief.md");

    assert_eq!(
        paths(&app),
        [
            "Archive/Docs/Guide.md",
            "Archive/Docs/Deep/Notes.md",
            "Archive/Brief.md"
        ]
    );
    assert_eq!(titles(&app), ["Guide", "Notes", "Brief"]);
    assert_eq!(app.document().unwrap().relative_path, "Archive/Brief.md");
}

#[test]
fn a_managed_note_another_app_moves_keeps_its_tab() {
    let (_app_data, folder, mut app) = opened(&["A.md"]);
    write(
        folder.path(),
        "Notes/Plan.md",
        &managed_note("note_01k00000000000000000000020"),
    );
    app.rescan();
    app.open_file("A.md", OpenAs::Kept);
    app.open_file("Notes/Plan.md", OpenAs::Kept);

    rename(folder.path(), "Notes/Plan.md", "Archive/Old plan.md");
    app.rescan();

    assert_eq!(paths(&app), ["A.md", "Archive/Old plan.md"]);
    assert_eq!(titles(&app), ["A", "Old plan"]);
    assert_eq!(active(&app).as_deref(), Some("Old plan"));
}

#[test]
fn two_tabs_that_end_up_on_one_file_become_one() {
    let (_app_data, folder, mut app) = opened(&["A.md"]);
    write(
        folder.path(),
        "Plan.md",
        &managed_note("note_01k00000000000000000000023"),
    );
    app.rescan();
    app.open_file("A.md", OpenAs::Kept);
    app.open_file("Plan.md", OpenAs::Kept);

    // Another app replaces A.md with the note.
    std::fs::remove_file(folder.path().join("A.md")).unwrap();
    rename(folder.path(), "Plan.md", "A.md");
    app.rescan();

    assert_eq!(paths(&app), ["A.md"]);
    assert_eq!(active(&app).as_deref(), Some("A"));
}

#[test]
fn a_file_without_an_id_that_another_app_moves_closes_its_tab() {
    let (_app_data, folder, mut app) = opened(&["A.md", "Loose.md"]);
    app.open_file("A.md", OpenAs::Kept);
    app.open_file("Loose.md", OpenAs::Kept);

    rename(folder.path(), "Loose.md", "Elsewhere/Loose.md");
    app.rescan();

    assert_eq!(paths(&app), ["A.md"]);
    assert_eq!(active(&app).as_deref(), Some("A"));
}

#[test]
fn moving_a_folder_to_the_trash_closes_its_tabs() {
    let (_app_data, folder, mut app) = opened(&["A.md", "Docs/One.md", "Docs/Two.md"]);
    write(
        folder.path(),
        "Docs/Plan.md",
        &managed_note("note_01k00000000000000000000021"),
    );
    app.rescan();
    for file in ["Docs/One.md", "A.md", "Docs/Plan.md", "Docs/Two.md"] {
        app.open_file(file, OpenAs::Kept);
    }

    // A trash moves the folder out of the workspace.
    let trash = tempfile::tempdir().unwrap();
    std::fs::rename(folder.path().join("Docs"), trash.path().join("Docs")).unwrap();
    app.rescan();

    assert_eq!(paths(&app), ["A.md"]);
    assert_eq!(active(&app).as_deref(), Some("A"));
}

#[test]
fn a_managed_note_moved_while_noura_was_closed_comes_back_at_its_new_path() {
    let (app_data, folder, mut app) = opened(&[]);
    write(
        folder.path(),
        "Plan.md",
        &managed_note("note_01k00000000000000000000022"),
    );
    app.rescan();
    app.open_file("Plan.md", OpenAs::Kept);
    drop(app);

    rename(folder.path(), "Plan.md", "Done/Plan.md");
    let app = launch(app_data.path());

    assert_eq!(paths(&app), ["Done/Plan.md"]);
}

#[test]
fn closing_the_last_tab_leaves_nothing_open() {
    let (_app_data, _folder, mut app) = opened(&["A.md"]);
    app.open_file("A.md", OpenAs::Kept);

    app.close_tab(tab_id(&app, "A"));

    assert!(app.tabs().is_empty());
    assert!(app.active_tab().is_none());
    assert!(app.document().is_none());
}
