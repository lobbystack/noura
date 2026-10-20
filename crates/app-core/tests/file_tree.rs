//! Seam 1: the file tree's view state over a real workspace folder.

use std::path::Path;

use app_core::{FileTree, TreeAction, TreeKey, TreeRow, WorkspaceService};

fn write(root: &Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn tree_of(files: &[&str], folders: &[&str]) -> (tempfile::TempDir, FileTree) {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    for file in files {
        write(folder.path(), file, b"x");
    }
    for empty in folders {
        std::fs::create_dir_all(folder.path().join(empty)).unwrap();
    }
    let workspace = WorkspaceService::new(app_data.path())
        .open_folder(folder.path())
        .unwrap();
    let tree = workspace.file_tree().unwrap();
    drop(workspace);
    drop(app_data);
    (folder, tree)
}

/// (shown name, depth, expanded) for each visible row.
fn shown(tree: &FileTree) -> Vec<(String, usize, Option<bool>)> {
    tree.rows()
        .iter()
        .map(|row: &TreeRow| (row.name.clone(), row.depth, row.expanded))
        .collect()
}

fn row(name: &str, depth: usize, expanded: Option<bool>) -> (String, usize, Option<bool>) {
    (name.to_owned(), depth, expanded)
}

#[test]
fn the_tree_lists_folders_first_and_sorts_siblings_by_name() {
    let (_folder, tree) = tree_of(
        &["Zeta/inner.md", "Note 10.md", "Note 2.md", "b.txt", "A.md"],
        &["alpha", "sub"],
    );

    assert_eq!(
        shown(&tree),
        [
            row("alpha", 0, Some(false)),
            row("sub", 0, Some(false)),
            row("Zeta", 0, Some(false)),
            row("A", 0, None),
            row("b.txt", 0, None),
            row("Note 2", 0, None),
            row("Note 10", 0, None),
        ]
    );
}

#[test]
fn the_tree_hides_metadata_dot_names_and_top_level_build_folders() {
    let (_folder, mut tree) = tree_of(
        &[
            "Visible.md",
            ".hidden.md",
            ".dotfolder/inside.md",
            "node_modules/pkg/index.js",
            "target/debug/out.bin",
            "sub/node_modules/kept.js",
            "sub/target/kept.txt",
            "sub/.secret",
        ],
        &[],
    );
    let sub = tree
        .rows()
        .iter()
        .position(|row| row.name == "sub")
        .unwrap();

    tree.toggle(sub);

    assert_eq!(
        shown(&tree),
        [
            row("sub", 0, Some(true)),
            row("node_modules", 1, Some(false)),
            row("target", 1, Some(false)),
            row("Visible", 0, None),
        ]
    );
}

#[test]
fn rows_carry_their_workspace_relative_paths() {
    let (_folder, mut tree) = tree_of(&["Projects/Plan.md"], &[]);

    tree.toggle(0);

    let paths: Vec<&str> = tree
        .rows()
        .iter()
        .map(|row| row.relative_path.as_str())
        .collect();
    assert_eq!(paths, ["Projects", "Projects/Plan.md"]);
}

fn focused_name(tree: &FileTree) -> &str {
    &tree.rows()[tree.focused().unwrap()].name
}

#[test]
fn arrow_keys_home_and_end_move_the_focus() {
    let (_folder, mut tree) = tree_of(&["a.md", "b.md", "c.md"], &[]);
    assert_eq!(tree.focused(), None);

    tree.press(TreeKey::Down);
    assert_eq!(focused_name(&tree), "a");
    tree.press(TreeKey::Down);
    tree.press(TreeKey::Down);
    tree.press(TreeKey::Down);
    assert_eq!(focused_name(&tree), "c");
    tree.press(TreeKey::Up);
    assert_eq!(focused_name(&tree), "b");
    tree.press(TreeKey::Home);
    assert_eq!(focused_name(&tree), "a");
    tree.press(TreeKey::End);
    assert_eq!(focused_name(&tree), "c");
}

#[test]
fn right_and_left_open_close_and_move_between_folders() {
    let (_folder, mut tree) = tree_of(&["Docs/Guide.md", "Readme.md"], &[]);
    tree.set_focus(0);

    tree.press(TreeKey::Right);
    assert_eq!(
        shown(&tree),
        [
            row("Docs", 0, Some(true)),
            row("Guide", 1, None),
            row("Readme", 0, None)
        ]
    );
    assert_eq!(focused_name(&tree), "Docs");

    tree.press(TreeKey::Right);
    assert_eq!(focused_name(&tree), "Guide");

    tree.press(TreeKey::Left);
    assert_eq!(focused_name(&tree), "Docs");

    tree.press(TreeKey::Left);
    assert_eq!(
        shown(&tree),
        [row("Docs", 0, Some(false)), row("Readme", 0, None)]
    );
    assert_eq!(focused_name(&tree), "Docs");
}

#[test]
fn enter_or_space_opens_a_file_and_toggles_a_folder() {
    let (_folder, mut tree) = tree_of(&["Docs/Guide.md"], &[]);
    tree.set_focus(0);

    assert_eq!(tree.press(TreeKey::Activate), None);
    assert_eq!(tree.rows()[0].expanded, Some(true));

    tree.press(TreeKey::Down);
    assert_eq!(
        tree.press(TreeKey::Activate),
        Some(TreeAction::OpenFile {
            relative_path: "Docs/Guide.md".into()
        })
    );

    tree.press(TreeKey::Home);
    tree.press(TreeKey::Activate);
    assert_eq!(shown(&tree), [row("Docs", 0, Some(false))]);
}

#[test]
fn collapsing_a_folder_by_click_moves_focus_out_of_it() {
    let (_folder, mut tree) = tree_of(&["Docs/Guide.md"], &[]);
    tree.toggle(0);
    tree.set_focus(1);

    tree.toggle(0);

    assert_eq!(focused_name(&tree), "Docs");
}

#[test]
fn type_ahead_jumps_to_the_next_name_starting_with_the_typed_letters() {
    let (_folder, mut tree) = tree_of(&["Apple.md", "Banana.md", "Blueberry.md", "Cherry.md"], &[]);
    let start = std::time::Instant::now();
    let later = |millis: u64| start + std::time::Duration::from_millis(millis);

    tree.type_ahead("b", later(0));
    assert_eq!(focused_name(&tree), "Banana");
    tree.type_ahead("l", later(200));
    assert_eq!(focused_name(&tree), "Blueberry");

    // After a pause, typing starts a new search.
    tree.type_ahead("c", later(2000));
    assert_eq!(focused_name(&tree), "Cherry");

    // Typing the same letter again cycles through its matches.
    tree.type_ahead("b", later(4000));
    assert_eq!(focused_name(&tree), "Banana");
    tree.type_ahead("b", later(4100));
    assert_eq!(focused_name(&tree), "Blueberry");
    tree.type_ahead("b", later(4200));
    assert_eq!(focused_name(&tree), "Banana");
}
