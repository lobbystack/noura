//! Seam 1: Settings, its section list and the actions its sections run.

mod support;

use std::{
    io,
    sync::{Arc, Mutex},
};

use app_core::{
    ABOUT_LINKS, App, LICENSE, LoginItem, NoticeKind, OpenAtLogin, SettingsGroup,
    THIRD_PARTY_LICENSES, VERSION, settings_section,
};
use support::{Call, FakeSystem, Fixture};

/// A login item that keeps its state in memory, records each change it's
/// asked for, and refuses changes when told to.
#[derive(Default)]
struct FakeLoginItem {
    enabled: Mutex<bool>,
    refusal: Mutex<Option<io::ErrorKind>>,
    changes: Mutex<Vec<bool>>,
}

impl FakeLoginItem {
    fn new(enabled: bool) -> Arc<Self> {
        let item = Self::default();
        *item.enabled.lock().unwrap() = enabled;
        Arc::new(item)
    }

    fn refuse(&self, kind: io::ErrorKind) {
        *self.refusal.lock().unwrap() = Some(kind);
    }

    fn enabled(&self) -> bool {
        *self.enabled.lock().unwrap()
    }

    /// Change the state as another app or the system's settings would.
    fn set_outside(&self, enabled: bool) {
        *self.enabled.lock().unwrap() = enabled;
    }

    fn changes(&self) -> Vec<bool> {
        self.changes.lock().unwrap().clone()
    }
}

impl LoginItem for FakeLoginItem {
    fn is_enabled(&self) -> io::Result<bool> {
        Ok(self.enabled())
    }

    fn set_enabled(&self, enabled: bool) -> io::Result<()> {
        self.changes.lock().unwrap().push(enabled);
        if let Some(kind) = *self.refusal.lock().unwrap() {
            return Err(io::Error::new(kind, "the fake system refused"));
        }
        self.set_outside(enabled);
        Ok(())
    }
}

/// The section list as each group's title and its sections' titles.
fn sections(app: &App) -> Vec<(String, Vec<String>)> {
    app.settings_sections()
        .into_iter()
        .map(|(group, sections)| {
            (
                group.title().to_owned(),
                sections
                    .into_iter()
                    .map(|section| section.title.clone())
                    .collect(),
            )
        })
        .collect()
}

fn shown_section(app: &App) -> Option<String> {
    app.settings().map(|settings| settings.section.clone())
}

fn open_at_login(app: &App) -> OpenAtLogin {
    app.settings().unwrap().open_at_login.clone()
}

#[test]
fn settings_lists_its_sections_under_this_computer_workspace_and_app() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    assert_eq!(app.settings(), None);

    app.open_settings();

    assert_eq!(
        sections(&app),
        [
            (
                "This computer".to_owned(),
                vec![
                    "General".to_owned(),
                    "Editor".to_owned(),
                    "Keyboard shortcuts".to_owned(),
                    "Sync account".to_owned(),
                    "AI providers".to_owned(),
                    "Web search".to_owned()
                ]
            ),
            (
                "Workspace".to_owned(),
                vec![
                    "Name and location".to_owned(),
                    "Plugins".to_owned(),
                    "Sync".to_owned()
                ]
            ),
            (
                "App".to_owned(),
                vec!["About Noura".to_owned(), "Updates".to_owned()]
            ),
        ]
    );
    assert_eq!(
        shown_section(&app).as_deref(),
        Some(settings_section::GENERAL)
    );

    app.close_settings();
    assert_eq!(app.settings(), None);
}

#[test]
fn later_areas_add_their_sections_to_a_group_and_settings_shows_them() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    app.register_settings_section("plugins", "Plugins", SettingsGroup::Workspace);
    app.register_settings_section("mcp", "External tools", SettingsGroup::Workspace);

    app.open_settings_at("plugins");

    assert_eq!(
        sections(&app)[1],
        (
            "Workspace".to_owned(),
            vec![
                "Name and location".to_owned(),
                "Plugins".to_owned(),
                "Sync".to_owned(),
                "External tools".to_owned()
            ]
        )
    );
    assert_eq!(shown_section(&app).as_deref(), Some("plugins"));

    app.select_settings_section(settings_section::ABOUT);
    assert_eq!(
        shown_section(&app).as_deref(),
        Some(settings_section::ABOUT)
    );
    // A section nobody registered changes nothing.
    app.select_settings_section("missing");
    assert_eq!(
        shown_section(&app).as_deref(),
        Some(settings_section::ABOUT)
    );
}

#[test]
fn open_at_login_reads_its_state_from_the_system_each_time_settings_opens() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let item = FakeLoginItem::new(true);
    app.set_login_item(item.clone());

    app.open_settings();
    assert_eq!(
        open_at_login(&app),
        OpenAtLogin {
            enabled: Some(true),
            problem: None
        }
    );

    app.close_settings();
    item.set_outside(false);
    app.open_settings();
    assert_eq!(open_at_login(&app).enabled, Some(false));
    assert_eq!(item.changes(), [] as [bool; 0]);
}

#[test]
fn flipping_open_at_login_adds_or_removes_noura_as_a_login_item() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let item = FakeLoginItem::new(false);
    app.set_login_item(item.clone());
    app.open_settings();

    app.set_open_at_login(true);
    assert!(item.enabled());
    assert_eq!(open_at_login(&app).enabled, Some(true));

    app.set_open_at_login(false);
    assert!(!item.enabled());
    assert_eq!(
        open_at_login(&app),
        OpenAtLogin {
            enabled: Some(false),
            problem: None
        }
    );
    assert_eq!(item.changes(), [true, false]);
}

#[test]
fn when_the_system_refuses_the_switch_flips_back_and_says_why() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let item = FakeLoginItem::new(false);
    item.refuse(io::ErrorKind::PermissionDenied);
    app.set_login_item(item.clone());
    app.open_settings();

    app.set_open_at_login(true);

    assert!(!item.enabled());
    let state = open_at_login(&app);
    assert_eq!(state.enabled, Some(false));
    let problem = state.problem.expect("the switch says why");
    assert!(problem.contains("permission"), "{problem}");

    item.refuse(io::ErrorKind::Other);
    app.set_open_at_login(true);
    let state = open_at_login(&app);
    assert_eq!(state.enabled, Some(false));
    assert!(state.problem.is_some());
    assert_eq!(item.changes(), [true, true]);
}

#[test]
fn without_a_login_item_the_switch_is_off_limits_and_says_so() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    app.open_settings();

    let state = open_at_login(&app);
    assert_eq!(state.enabled, None);
    assert!(state.problem.is_some());

    app.set_open_at_login(true);
    assert_eq!(open_at_login(&app).enabled, None);
}

#[test]
fn about_names_the_version_and_license_and_hands_each_link_to_the_browser() {
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    app.open_settings_at(settings_section::ABOUT);

    assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
    assert_eq!(LICENSE, "MIT");
    let links: Vec<(&str, &str)> = ABOUT_LINKS
        .iter()
        .map(|link| (link.title, link.url))
        .collect();
    assert_eq!(
        links,
        [
            ("Website", "https://noura.app/"),
            ("Source code", "https://github.com/lobbystack/noura"),
            (
                "Report a problem",
                "https://github.com/lobbystack/noura/issues"
            ),
            ("Privacy policy", "https://noura.app/privacy/"),
            ("Terms and conditions", "https://noura.app/terms/"),
            (
                "Third-party licenses",
                "https://github.com/lobbystack/noura/blob/main/THIRD_PARTY_NOTICES.md"
            ),
        ]
    );

    for link in &ABOUT_LINKS {
        app.open_link(link.url);
    }

    let opened: Vec<Call> = ABOUT_LINKS
        .iter()
        .map(|link| Call::OpenUrl(link.url.to_owned()))
        .collect();
    assert_eq!(system.calls(), opened);
    assert_eq!(app.settings().unwrap().notice, None);
}

#[test]
fn the_third_party_licenses_link_opens_the_list_this_package_ships() {
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    app.open_settings_at(settings_section::ABOUT);
    assert!(ABOUT_LINKS.contains(&THIRD_PARTY_LICENSES));

    // A build that cargo-packager didn't package has no list of its own,
    // so the link goes to the list online.
    app.open_about_link(&THIRD_PARTY_LICENSES);
    let package = tempfile::tempdir().unwrap();
    let list = package.path().join("THIRD_PARTY_LICENSES.html");
    std::fs::write(&list, "<!doctype html>").unwrap();
    app.set_third_party_licenses(list.clone());
    app.open_about_link(&THIRD_PARTY_LICENSES);
    // The other links still go to the browser.
    app.open_about_link(&ABOUT_LINKS[0]);

    assert_eq!(
        system.calls(),
        [
            Call::OpenUrl(THIRD_PARTY_LICENSES.url.to_owned()),
            Call::OpenFile(list),
            Call::OpenUrl(ABOUT_LINKS[0].url.to_owned()),
        ]
    );
    assert_eq!(app.settings().unwrap().notice, None);
}

/// Every window's About links to the list, including windows opened
/// after the desktop app found it.
#[test]
fn a_new_window_opens_the_same_license_list() {
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    let package = tempfile::tempdir().unwrap();
    let list = package.path().join("THIRD_PARTY_LICENSES.html");
    std::fs::write(&list, "<!doctype html>").unwrap();
    app.set_third_party_licenses(list.clone());

    let mut window = app.new_window().expect("a workspace is open");
    window.open_settings_at(settings_section::ABOUT);
    window.open_about_link(&THIRD_PARTY_LICENSES);

    assert_eq!(system.calls(), [Call::OpenFile(list)]);
}

#[test]
fn a_license_list_that_won_t_open_says_why() {
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    app.open_settings_at(settings_section::ABOUT);
    let package = tempfile::tempdir().unwrap();
    let list = package.path().join("THIRD_PARTY_LICENSES.html");
    app.set_third_party_licenses(list.clone());

    app.open_about_link(&THIRD_PARTY_LICENSES);

    assert_eq!(system.calls(), [Call::OpenFile(list)]);
    let notice = app.settings().unwrap().notice.clone().unwrap();
    assert_eq!(notice.kind, NoticeKind::Problem);
    assert_eq!(notice.title, "Couldn't open the license list");
    assert_eq!(
        notice.message,
        "Noura couldn't open the list of third-party licenses it ships with."
    );
}

#[test]
fn only_web_links_reach_the_browser() {
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    app.open_settings_at(settings_section::ABOUT);

    for url in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "noura://open",
        "https://noura.app/ x",
        "",
    ] {
        app.open_link(url);
        let notice = app.settings().unwrap().notice.clone();
        assert_eq!(
            notice.map(|notice| notice.kind),
            Some(NoticeKind::Problem),
            "{url} wasn't refused"
        );
    }
    assert_eq!(system.calls(), []);
}
