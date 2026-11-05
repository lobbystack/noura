//! A workspace with the kit test plugin installed and on, and the steps
//! the UI kit tests take in it: opening one of its pages, pressing keys,
//! waiting for the plugin's answers and reading what the plugin says the
//! last event was.

use std::time::{Duration, Instant};

use egui::{Key, Modifiers, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{By, NodeT, Queryable},
};

use super::{FakePicker, create_workspace, harness, plugins, settle};

pub type Ui = Harness<'static, app_ui::NouraUi>;

pub struct Opened {
    pub harness: Ui,
    /// The app data and the folder around the workspace, which last as long
    /// as the test binds them.
    _folders: (tempfile::TempDir, tempfile::TempDir),
}

/// A workspace with the kit plugin installed and on, opened at launch.
pub fn opened(kit: &[u8]) -> Opened {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), "Garden", &parent.path().join("Garden"));
    let folder = plugins::folder(parent.path(), &plugins::kit_manifest(), "plugin.wasm", kit);
    plugins::install_and_enable(app_data.path(), &folder, "kit");
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);
    Opened {
        harness,
        _folders: (app_data, parent),
    }
}

/// A harness state that says whether the window it draws waits for
/// background work, such as a plugin's answer.
pub trait Busy {
    fn busy(&self) -> bool;
}

impl Busy for app_ui::NouraUi {
    fn busy(&self) -> bool {
        self.is_busy()
    }
}

/// Run frames until the window settles. Plugins answer on their own
/// threads, so this also waits, frame by frame, until their answers
/// arrive.
pub fn run<S: Busy>(harness: &mut Harness<'static, S>) {
    harness.run();
    let deadline = Instant::now() + Duration::from_secs(120);
    while harness.state().busy() {
        assert!(Instant::now() < deadline, "background work never finished");
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
    harness.run();
}

pub fn open_page<S: Busy>(harness: &mut Harness<'static, S>, title: &str) {
    harness
        .get_by_role_and_label(Role::Navigation, "Pages")
        .get_by_role_and_label(Role::Link, title)
        .click();
    run(harness);
}

pub fn press<S: Busy>(harness: &mut Harness<'static, S>, key: Key) {
    harness.key_press(key);
    run(harness);
}

pub fn press_with<S: Busy>(harness: &mut Harness<'static, S>, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    run(harness);
}

/// What the plugin says the last event was.
pub fn last_event<S>(harness: &Harness<'static, S>) -> String {
    let said: Vec<String> = harness
        .query_all(By::new().role(Role::Label))
        .filter_map(|label| label.accesskit_node().value())
        .filter_map(|value| value.strip_prefix("Last event: ").map(str::to_owned))
        .collect();
    assert_eq!(said.len(), 1, "{said:?}");
    said[0].clone()
}

pub fn labels<'a>(nodes: impl Iterator<Item = egui_kittest::Node<'a>>) -> Vec<String> {
    nodes
        .filter_map(|node| node.accesskit_node().label())
        .collect()
}

pub fn is_selected(node: &egui_kittest::Node<'_>) -> bool {
    node.accesskit_node().is_selected() == Some(true)
}

/// The label of the widget with keyboard focus.
pub fn focused<S>(harness: &Harness<'static, S>) -> Option<String> {
    harness
        .query_all(By::new())
        .find(|node| node.is_focused())
        .and_then(|node| node.accesskit_node().label())
}
