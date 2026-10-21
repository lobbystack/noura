//! Settings' Updates section and the Restart offer, through AccessKit.

mod support;

use std::{
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, Sender, channel},
    },
    time::{Duration, Instant},
};

use app_core::{App, PendingUpdate, UpdateError, UpdateErrorKind, UpdateSource, settings_section};
use app_ui::NouraUi;
use egui::accesskit::Role;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::FakePicker;

#[derive(Clone, Default)]
struct FakeSource {
    log: Arc<Mutex<Vec<String>>>,
    release: Arc<Mutex<Option<String>>>,
    unreachable: Arc<Mutex<bool>>,
    gate: Arc<Mutex<Option<Receiver<()>>>>,
}

impl FakeSource {
    fn hold_downloads(&self) -> Sender<()> {
        let (sender, receiver) = channel();
        *self.gate.lock().unwrap() = Some(receiver);
        sender
    }
}

impl UpdateSource for FakeSource {
    fn check(&self) -> Result<Option<Box<dyn PendingUpdate>>, UpdateError> {
        self.log.lock().unwrap().push("check".into());
        if *self.unreachable.lock().unwrap() {
            return Err(UpdateError::new(UpdateErrorKind::Unreachable, "offline"));
        }
        Ok(self.release.lock().unwrap().clone().map(|version| {
            Box::new(FakeUpdate {
                version,
                log: self.log.clone(),
                gate: self.gate.lock().unwrap().take(),
            }) as Box<dyn PendingUpdate>
        }))
    }

    fn restart(&self) -> Result<(), UpdateError> {
        self.log.lock().unwrap().push("restart".into());
        Ok(())
    }
}

struct FakeUpdate {
    version: String,
    log: Arc<Mutex<Vec<String>>>,
    gate: Option<Receiver<()>>,
}

impl PendingUpdate for FakeUpdate {
    fn version(&self) -> &str {
        &self.version
    }

    fn notes(&self) -> Option<&str> {
        Some("Faster search.")
    }

    fn download(&mut self) -> Result<(), UpdateError> {
        if let Some(gate) = &self.gate {
            gate.recv().unwrap();
        }
        self.log.lock().unwrap().push("download".into());
        Ok(())
    }

    fn install(&mut self) -> Result<(), UpdateError> {
        self.log.lock().unwrap().push("install".into());
        Ok(())
    }
}

/// An app whose updates come from `source`. The scheduled check stays
/// hours away, so only the tests' own actions check.
fn app_with(source: &FakeSource, app_data: &std::path::Path) -> App {
    let mut app = App::launch(app_data);
    let far_future = Instant::now() + Duration::from_secs(60 * 60);
    app.enable_updates("0.2.0", Arc::new(source.clone()), far_future);
    app
}

/// The window with Settings open on its Updates section.
fn section(mut app: App) -> Harness<'static, NouraUi> {
    app.open_settings_at(settings_section::UPDATES);
    let ui = NouraUi::new(app, Box::new(FakePicker::default()));
    let mut harness = Harness::builder()
        .with_size([1000.0, 700.0])
        .build_ui_state(|ui, state: &mut NouraUi| state.ui(ui), ui);
    harness.step();
    let updates = harness
        .get_by_role_and_label(Role::Navigation, "Settings sections")
        .get_by_role_and_label(Role::Button, "Updates");
    assert_eq!(
        updates.accesskit_node().toggled(),
        Some(egui::accesskit::Toggled::True)
    );
    harness
}

/// The open Settings dialog.
fn dialog<'a>(harness: &'a Harness<'static, NouraUi>) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::Dialog, "Settings")
}

/// Run frames until `label` shows. Steps one frame at a time, because the
/// spinner keeps repainting while work runs.
fn wait_for<State>(harness: &mut Harness<'static, State>, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        harness.step();
        if harness.query_by_label(label).is_some() {
            harness.step();
            return;
        }
        assert!(Instant::now() < deadline, "{label:?} never showed");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn check_for_updates_says_when_noura_is_up_to_date() {
    let app_data = tempfile::tempdir().unwrap();
    let source = FakeSource::default();
    let mut harness = section(app_with(&source, app_data.path()));
    harness.run();

    dialog(&harness).get_by_label_contains("You have version 0.2.0.");
    dialog(&harness)
        .get_by_role_and_label(Role::Button, "Check for updates")
        .click();
    wait_for(&mut harness, "Noura is up to date.");

    assert_eq!(*source.log.lock().unwrap(), ["check"]);
    assert!(harness.unnamed_widgets().is_empty());
}

#[test]
fn the_section_shows_the_download_then_offers_restart() {
    let app_data = tempfile::tempdir().unwrap();
    let source = FakeSource::default();
    *source.release.lock().unwrap() = Some("0.3.0".into());
    let release = source.hold_downloads();
    let mut harness = section(app_with(&source, app_data.path()));
    harness.run();

    harness
        .get_by_role_and_label(Role::Button, "Check for updates")
        .click();
    wait_for(&mut harness, "Downloading version 0.3.0…");
    let check = harness.get_by_role_and_label(Role::Button, "Check for updates");
    assert!(check.accesskit_node().is_disabled());

    release.send(()).unwrap();
    wait_for(&mut harness, "Version 0.3.0 is ready");
    harness.get_by_label("Faster search.");
    harness
        .get_by_role_and_label(Role::Button, "Restart to update")
        .click();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !source.log.lock().unwrap().contains(&"restart".to_owned()) {
        assert!(Instant::now() < deadline, "restart never ran");
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        *source.log.lock().unwrap(),
        ["check", "download", "install", "restart"]
    );
}

#[test]
fn a_failed_check_says_why() {
    let app_data = tempfile::tempdir().unwrap();
    let source = FakeSource::default();
    *source.unreachable.lock().unwrap() = true;
    let mut harness = section(app_with(&source, app_data.path()));
    harness.run();

    harness
        .get_by_role_and_label(Role::Button, "Check for updates")
        .click();
    wait_for(
        &mut harness,
        "Couldn't reach the update server. Check your connection.",
    );
    let check = harness.get_by_role_and_label(Role::Button, "Check for updates");
    assert!(!check.accesskit_node().is_disabled(), "you can try again");
}

#[test]
fn a_build_without_updates_explains_and_offers_no_check() {
    let app_data = tempfile::tempdir().unwrap();
    let mut app = App::launch(app_data.path());
    app.disable_updates("0.2.0", "Install new versions from noura.app.");
    let mut harness = section(app);
    harness.run();

    harness.get_by_label("You have version 0.2.0. Install new versions from noura.app.");
    assert!(harness.query_by_label("Check for updates").is_none());
}

#[test]
fn every_screen_offers_restart_until_you_choose_later() {
    let app_data = tempfile::tempdir().unwrap();
    let source = FakeSource::default();
    *source.release.lock().unwrap() = Some("0.3.0".into());
    let mut app = app_with(&source, app_data.path());
    app.check_for_updates();
    let ui = NouraUi::new(app, Box::new(FakePicker::default()));
    let mut harness = Harness::builder()
        .with_size([1000.0, 700.0])
        .build_ui_state(|ui, state: &mut NouraUi| state.ui(ui), ui);

    wait_for(&mut harness, "Noura 0.3.0 is ready");
    harness.get_by_label("Restart to finish updating.");
    harness.get_by_role_and_label(Role::Button, "Restart");
    harness.get_by_role_and_label(Role::Button, "Later").click();
    harness.run();

    assert!(harness.query_by_label("Noura 0.3.0 is ready").is_none());
    assert_eq!(*source.log.lock().unwrap(), ["check", "download"]);
}
