//! Quick open and search in the palette overlay, through AccessKit. These
//! translate the legacy app's quick-open checks and its "content search
//! finds text inside notes" check.

mod support;

use std::time::{Duration, Instant};

use egui::{Key, Modifiers, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, create_workspace, harness, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

/// A workspace holding `files`, opened at launch.
fn opened(app_data: &std::path::Path, folder: &std::path::Path, files: &[(&str, &str)]) -> Ui {
    create_workspace(app_data, "Garden", folder);
    for (path, text) in files {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let mut harness = harness(app_data, FakePicker::default());
    settle(&mut harness);
    harness
}

/// Every file under `folder` outside `.noura`, with its text.
fn files(folder: &std::path::Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut stack = vec![folder.to_owned()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(next).unwrap() {
            let path = entry.unwrap().path();
            let relative = path
                .strip_prefix(folder)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if relative == ".noura" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push((relative, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    found.sort();
    found
}

fn dialog<'a>(harness: &'a Ui, name: &'a str) -> Option<egui_kittest::Node<'a>> {
    harness.query_by_role_and_label(Role::Dialog, name)
}

fn shortcut(harness: &mut Ui, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    harness.run();
}

fn press(harness: &mut Ui, key: Key) {
    harness.key_press(key);
    harness.run();
}

fn type_text(harness: &mut Ui, text: &str) {
    harness
        .input_mut()
        .events
        .push(egui::Event::Text(text.to_owned()));
    harness.run();
}

/// The selected tab's label: the open file's name, and whether the tab
/// is a preview, as a file the tree opens is.
fn open_file(harness: &Ui) -> Option<String> {
    harness
        .query_all_by_role(Role::Tab)
        .find(|tab| tab.accesskit_node().is_selected() == Some(true))
        .and_then(|tab| tab.accesskit_node().label())
}

fn preview(name: &str) -> Option<String> {
    Some(format!("{name} (preview)"))
}

/// The labels of the options the palette lists.
fn options(harness: &Ui) -> Vec<String> {
    harness
        .query_all_by_role(Role::ListBoxOption)
        .filter_map(|option| option.accesskit_node().label())
        .collect()
}

/// Run frames until `ready` holds, as search answers in the background.
fn wait_until(harness: &mut Ui, ready: impl Fn(&Ui) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        harness.step();
        if ready(harness) {
            harness.run();
            return;
        }
        assert!(Instant::now() < deadline, "the palette never showed it");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn focused_label(harness: &Ui) -> Option<String> {
    harness
        .query_by(|node| node.is_focused())
        .and_then(|node| node.accesskit_node().label())
}

#[test]
fn quick_open_finds_a_note_by_name_and_enter_opens_the_best_match() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(
        app_data.path(),
        &folder,
        &[
            ("Meeting notes.md", "agenda"),
            ("Groceries.md", "milk"),
            ("Meeting archive.md", "old"),
        ],
    );
    let before = files(&folder);

    shortcut(&mut harness, Modifiers::COMMAND, Key::O);
    assert!(dialog(&harness, "Quick open").is_some());
    type_text(&mut harness, "groc");
    assert_eq!(options(&harness)[0], "Groceries");
    assert!(
        harness.unnamed_widgets().is_empty(),
        "{:?}",
        harness.unnamed_widgets()
    );
    press(&mut harness, Key::Enter);
    assert!(dialog(&harness, "Quick open").is_none());
    assert_eq!(open_file(&harness), preview("Groceries"));

    harness
        .get_by_role_and_label(Role::TreeItem, "Groceries")
        .click();
    harness.run();
    shortcut(&mut harness, Modifiers::COMMAND, Key::O);
    type_text(&mut harness, "meeting n");
    press(&mut harness, Key::Enter);
    assert!(dialog(&harness, "Quick open").is_none());
    assert_eq!(open_file(&harness), preview("Meeting notes"));
    assert_eq!(files(&folder), before);
}

#[test]
fn quick_open_opens_again_right_after_opening_a_file() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(
        app_data.path(),
        &folder,
        &[("Alpha.md", "a"), ("Beta.md", "b")],
    );

    for name in ["Alpha", "Beta", "Alpha"] {
        shortcut(&mut harness, Modifiers::COMMAND, Key::O);
        assert!(dialog(&harness, "Quick open").is_some());
        type_text(&mut harness, &name.to_lowercase());
        assert_eq!(options(&harness)[0], name);
        press(&mut harness, Key::Enter);
        assert_eq!(open_file(&harness), preview(name));
    }
    assert_eq!(
        files(&folder),
        [
            ("Alpha.md".to_owned(), "a".to_owned()),
            ("Beta.md".to_owned(), "b".to_owned())
        ]
    );
}

#[test]
fn keys_typed_right_after_mod_o_land_in_quick_open_and_escape_gives_the_keyboard_back() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(
        app_data.path(),
        &folder,
        &[("Quiet.md", "unchanged"), ("Zebra.md", "z")],
    );
    harness
        .get_by_role_and_label(Role::TreeItem, "Quiet")
        .click();
    harness.run();
    assert_eq!(focused_label(&harness).as_deref(), Some("Quiet"));

    for _ in 0..3 {
        // The shortcut and the typing arrive in one frame.
        harness.input_mut().events.extend([
            egui::Event::Key {
                key: Key::O,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::COMMAND,
            },
            egui::Event::Text("zeb".to_owned()),
        ]);
        harness.run();
        let field = harness.get_by_role_and_label(Role::TextInput, "Find a file by name");
        assert_eq!(field.value().as_deref(), Some("zeb"));
        assert!(field.is_focused());
        press(&mut harness, Key::Escape);
        assert!(dialog(&harness, "Quick open").is_none());
        // The tree row that had the keyboard has it again, and the typing
        // never moved the tree's type-ahead to Zebra.
        assert_eq!(focused_label(&harness).as_deref(), Some("Quiet"));
    }
    assert_eq!(open_file(&harness), preview("Quiet"));
}

#[test]
fn content_search_finds_text_inside_notes() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(
        app_data.path(),
        &folder,
        &[
            ("Recipe.md", "Add the saffron last"),
            ("Other.md", "nothing here"),
        ],
    );

    shortcut(&mut harness, Modifiers::COMMAND | Modifiers::SHIFT, Key::F);
    assert!(dialog(&harness, "Search").is_some());
    type_text(&mut harness, "saffron");
    wait_until(&mut harness, |harness| !options(harness).is_empty());
    assert_eq!(options(&harness), ["Recipe"]);
    let result = harness.get_by_role_and_label(Role::ListBoxOption, "Recipe");
    assert!(
        result
            .accesskit_node()
            .description()
            .is_some_and(|text| text.contains("Add the saffron last")),
        "{:?}",
        result.accesskit_node().description()
    );
    result.click();
    harness.run();

    assert!(dialog(&harness, "Search").is_none());
    assert_eq!(open_file(&harness), preview("Recipe"));
}

#[test]
fn search_filters_and_states_are_reachable_from_the_keyboard() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let task = "---\nid: task_01jbcdefghjkmnpqrstvwxyz0a\ntype: task\n\
                created: 2026-10-20T10:00:00Z\nupdated: 2026-10-20T10:00:00Z\n---\n\nBuy saffron\n";
    let mut harness = opened(
        app_data.path(),
        &folder,
        &[
            (
                "Kitchen/Paella.md",
                "---\ntags: [recipe]\n---\nsaffron rice\n",
            ),
            ("Kitchen/Shopping.md", task),
            ("Spices.md", "saffron and pepper\n"),
        ],
    );

    shortcut(&mut harness, Modifiers::COMMAND | Modifiers::SHIFT, Key::F);
    type_text(&mut harness, "saffron");
    wait_until(&mut harness, |harness| options(harness).len() == 3);
    assert!(
        harness.unnamed_widgets().is_empty(),
        "{:?}",
        harness.unnamed_widgets()
    );

    // The type filter offers the index's types.
    press(&mut harness, Key::Tab);
    assert_eq!(focused_label(&harness).as_deref(), Some("Any type"));
    press(&mut harness, Key::Tab);
    assert_eq!(focused_label(&harness).as_deref(), Some("Task"));
    press(&mut harness, Key::Space);
    wait_until(&mut harness, |harness| options(harness).len() == 1);
    let shopping = harness.get_by_role_and_label(Role::ListBoxOption, "Shopping");
    let described = shopping.accesskit_node().description().unwrap_or_default();
    assert!(described.contains("Task"), "{described}");
    assert!(described.contains("Kitchen"), "{described}");

    // Back to any type, then the folder filter.
    press_shift_tab(&mut harness, 1);
    assert_eq!(focused_label(&harness).as_deref(), Some("Any type"));
    press(&mut harness, Key::Space);
    wait_until(&mut harness, |harness| options(harness).len() == 3);
    while focused_label(&harness).as_deref() != Some("Folder") {
        press(&mut harness, Key::Tab);
    }
    type_text(&mut harness, "Kitchen");
    wait_until(&mut harness, |harness| options(harness).len() == 2);
    press(&mut harness, Key::Tab);
    assert_eq!(focused_label(&harness).as_deref(), Some("Tag"));
    type_text(&mut harness, "recipe");
    wait_until(&mut harness, |harness| options(harness) == ["Paella"]);

    // A tag nothing holds.
    type_text(&mut harness, "s");
    wait_until(&mut harness, |harness| {
        harness.query_by_label("No matches").is_some()
    });
    assert!(options(&harness).is_empty());

    // A folder outside the workspace says why.
    while focused_label(&harness).as_deref() != Some("Folder") {
        press_shift_tab(&mut harness, 1);
    }
    type_text(&mut harness, "/..");
    wait_until(&mut harness, |harness| {
        harness.query_by_role(Role::Alert).is_some()
    });
    press(&mut harness, Key::Escape);
    assert!(dialog(&harness, "Search").is_none());
}

#[test]
fn search_says_when_it_isnt_available() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder, &[("Recipe.md", "saffron")]);
    let workspaces = app_data.path().join("workspaces");
    let id = std::fs::read_dir(&workspaces)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name();
    rusqlite::Connection::open(workspaces.join(id).join("index.sqlite"))
        .unwrap()
        .execute_batch("DROP TABLE object_fts;")
        .unwrap();

    shortcut(&mut harness, Modifiers::COMMAND | Modifiers::SHIFT, Key::F);
    type_text(&mut harness, "saffron");
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Alert, "Search isn't available right now.")
            .is_some()
    });
    assert!(options(&harness).is_empty());
}

fn press_shift_tab(harness: &mut Ui, times: usize) {
    for _ in 0..times {
        harness.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
        harness.run();
    }
}
