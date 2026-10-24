//! Seam 1: installing and updating plugins from releases on GitHub,
//! against a real workspace folder and app data. Downloads go to a
//! stand-in for GitHub's servers through the web access boundary; no test
//! reaches github.com.

mod support;

use std::sync::Arc;

use app_core::fake_github::{Entry, FakeGitHub, archive};
use app_core::{App, CapabilityLine, Grant, PluginRow, UpdateCheck};
use support::{Fixture, plugins, settle};

const REPOSITORY: &str = "maker/probe-plugin";
const RELEASE: &str = "https://github.com/maker/probe-plugin/releases/tag/v0.1.0";

/// The capabilities the probe and hello plugins need to draw their pages.
const DRAWS: [&str; 2] = ["name = \"workspace-read\"", "name = \"views\""];

/// A manifest for the plugin `probe` at `version` with these capabilities.
fn manifest(version: &str, capabilities: &[&str]) -> String {
    plugins::manifest("probe", "plugin.wasm", capabilities)
        .replacen("name = \"probe\"", "name = \"Probe\"", 1)
        .replace(
            "version = \"0.1.0\"\ninterface",
            &format!("version = \"{version}\"\ninterface"),
        )
}

fn plugin_archive(manifest: &str, component: &[u8]) -> Vec<u8> {
    archive(&[
        Entry::File("plugin.toml", manifest.as_bytes()),
        Entry::File("plugin.wasm", component),
    ])
}

/// Publish `component` as release `tag` of the probe plugin at `version`.
fn publish(github: &FakeGitHub, tag: &str, version: &str, capabilities: &[&str], component: &[u8]) {
    github.publish_archive(
        REPOSITORY,
        tag,
        plugin_archive(&manifest(version, capabilities), component),
    );
}

/// The app with the workspace open, downloading from `github`.
fn app_on(fixture: &Fixture, github: &Arc<FakeGitHub>) -> App {
    let mut app = fixture.app();
    app.set_plugin_release_transport(github.clone());
    app
}

/// Open the install dialog for `url` and wait for the download.
fn review(app: &mut App, url: &str) {
    app.review_plugin_release(url);
    settle(app);
    assert!(
        !app.plugin_install()
            .expect("the dialog is open")
            .downloading,
        "the download never finished"
    );
}

/// Install the release at `url`, approving what it declares.
fn install(app: &mut App, url: &str) {
    review(app, url);
    assert_eq!(app.plugin_install().unwrap().problem, None);
    assert!(app.approve_plugin_install(), "the plugin installs");
    settle(app);
}

/// Check for an update of the probe plugin and wait for the answer.
fn check(app: &mut App) -> PluginRow {
    assert!(app.check_plugin_update("probe"));
    settle(app);
    let row = probe_row(app);
    assert_ne!(row.update_check, Some(UpdateCheck::Checking));
    row
}

fn probe_row(app: &App) -> PluginRow {
    app.plugins()
        .into_iter()
        .find(|row| row.id == "probe")
        .expect("the probe plugin has a row")
}

fn installed(app: &App) -> Vec<PluginRow> {
    app.plugins()
        .into_iter()
        .filter(|row| row.installed)
        .collect()
}

fn line(name: &str, description: &str) -> CapabilityLine {
    CapabilityLine {
        name: name.into(),
        description: description.into(),
        grant: Grant::Capability(name.into()),
    }
}

fn drawing_lines() -> Vec<CapabilityLine> {
    vec![
        line(
            "workspace-read",
            "Read the files and folders in your workspace.",
        ),
        line("views", "Add pages to the rail and show panels in them."),
    ]
}

/// The title of the first page plugins add to the rail, after the Inbox
/// and Files: Probe for the probe plugin's build, Hello for the hello
/// plugin's.
fn first_plugin_page(app: &App) -> Option<String> {
    let titles: Vec<String> = app.rail().iter().map(|page| page.title.clone()).collect();
    assert_eq!(titles[..2], ["Inbox", "Files"]);
    titles.get(2).cloned()
}

/// The workspace's files with their bytes, `.noura` included, apart from
/// the index and locks.
fn everything(fixture: &Fixture) -> Vec<(String, Vec<u8>)> {
    plugins::files_under(&fixture.root)
        .into_iter()
        .filter(|path| !path.starts_with(".noura/index") && !path.ends_with(".lock"))
        .map(|path| {
            let bytes = fixture.read(&path);
            (path, bytes)
        })
        .collect()
}

fn app_data_plugins(fixture: &Fixture) -> Vec<String> {
    plugins::files_under(&fixture.app_data.path().join("plugins"))
}

fn records(fixture: &Fixture) -> Vec<u8> {
    std::fs::read(fixture.app_data.path().join("plugins/installed.json")).unwrap()
}

#[test]
fn an_address_that_isnt_a_github_release_is_refused_and_nothing_is_fetched() {
    let fixture = Fixture::new(&[]);
    let github = FakeGitHub::new();
    let mut app = app_on(&fixture, &github);

    for url in [
        "http://github.com/maker/probe-plugin/releases/tag/v0.1.0",
        "https://gitlab.com/maker/probe-plugin/-/releases/v0.1.0",
        "https://github.com/maker/probe-plugin",
        "https://example.com/probe.noura-plugin.zip",
        "probe-plugin",
    ] {
        app.review_plugin_release(url);
        settle(&mut app);

        let review = app.plugin_install().expect("the dialog says why");
        assert!(!review.downloading, "{url}");
        let problem = review.problem.clone().expect("the address is refused");
        assert!(problem.contains("release on GitHub"), "{problem}");
        assert!(!app.approve_plugin_install(), "{url}");
        app.cancel_plugin_install();
    }

    assert_eq!(github.lookups(), Vec::<String>::new());
    assert_eq!(github.requests(), Vec::<String>::new());
    assert_eq!(app_data_plugins(&fixture), Vec::<String>::new());
}

#[test]
fn the_install_dialog_shows_the_release_and_its_capabilities_before_anything_installs() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let github = FakeGitHub::new();
    publish(
        &github,
        "v0.1.0",
        "0.1.0",
        &[
            DRAWS[0],
            DRAWS[1],
            "name = \"network\"\nhosts = [\"api.example.com\"]",
            "name = \"commands\"\ntargets = [\"tasks/add\"]",
        ],
        &probe,
    );
    let mut app = app_on(&fixture, &github);

    app.review_plugin_release(RELEASE);
    let waiting = app
        .plugin_install()
        .expect("the dialog opens at once")
        .clone();
    assert!(waiting.downloading);
    assert_eq!(waiting.source, RELEASE);
    settle(&mut app);

    let review = app.plugin_install().expect("the dialog is open");
    assert_eq!(review.problem, None);
    assert!(
        review.from_github,
        "the dialog warns of the unreviewed source"
    );
    assert_eq!(review.name, "Probe");
    assert_eq!(review.version, "0.1.0");
    assert_eq!(review.source, RELEASE);
    let mut expected = drawing_lines();
    expected.extend([
        CapabilityLine {
            name: "network".into(),
            description: "Connect to api.example.com.".into(),
            grant: Grant::Host("api.example.com".into()),
        },
        line(
            "commands",
            "Add commands to the command palette and offer them to other plugins.",
        ),
        CapabilityLine {
            name: "commands".into(),
            description: "Run the add command of the tasks plugin.".into(),
            grant: Grant::Target("tasks/add".into()),
        },
    ]);
    assert_eq!(review.capabilities, expected);
    // Every download went over HTTPS to GitHub, and nothing is installed.
    assert!(!github.requests().is_empty());
    for url in github.requests() {
        assert!(url.starts_with("https://"), "{url}");
    }
    assert!(installed(&app).is_empty());
    assert_eq!(app_data_plugins(&fixture), Vec::<String>::new());
}

#[test]
fn cancel_leaves_nothing_installed_and_stores_no_approval() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let github = FakeGitHub::new();
    publish(&github, "v0.1.0", "0.1.0", &DRAWS, &probe);
    let mut app = app_on(&fixture, &github);
    review(&mut app, RELEASE);

    app.cancel_plugin_install();

    assert!(app.plugin_install().is_none());
    assert!(installed(&app).is_empty());
    assert_eq!(app_data_plugins(&fixture), Vec::<String>::new());

    // Cancelling while the download runs drops what it brings.
    app.review_plugin_release(RELEASE);
    app.cancel_plugin_install();
    settle(&mut app);
    assert!(app.plugin_install().is_none());
    assert_eq!(app_data_plugins(&fixture), Vec::<String>::new());
    drop(app);
    assert!(installed(&fixture.app()).is_empty());
}

#[test]
fn approving_installs_the_plugin_on_this_device_as_a_folder_install_does() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let fixture = Fixture::new(&[("Plan.md", b"plan")]);
    let github = FakeGitHub::new();
    publish(&github, "v0.1.0", "0.1.0", &DRAWS, &probe);
    let mut app = app_on(&fixture, &github);
    let before = everything(&fixture);

    install(&mut app, RELEASE);

    assert!(app.plugin_install().is_none());
    let row = probe_row_installed(&app);
    assert_eq!(row.release.as_deref(), Some(RELEASE));
    assert_eq!(row.folder, None);
    assert_eq!(row.version.as_deref(), Some("0.1.0"));
    assert_eq!(row.approved, drawing_lines());
    assert_eq!(everything(&fixture), before, "no workspace file changed");
    let saved = String::from_utf8(records(&fixture)).unwrap();
    assert!(saved.contains(RELEASE), "{saved}");
    assert!(saved.contains("workspace-read"), "{saved}");

    assert!(app.set_plugin_enabled("probe", true));
    settle(&mut app);
    assert_eq!(first_plugin_page(&app).as_deref(), Some("Probe"));
    drop(app);
    let app = fixture.app();
    assert_eq!(probe_row_installed(&app).release.as_deref(), Some(RELEASE));
    assert_eq!(first_plugin_page(&app).as_deref(), Some("Probe"));
}

fn probe_row_installed(app: &App) -> PluginRow {
    let rows = installed(app);
    assert_eq!(rows.len(), 1, "{rows:?}");
    rows[0].clone()
}

#[test]
fn check_for_updates_offers_the_newest_release_only_when_its_version_is_higher() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let github = FakeGitHub::new();
    publish(&github, "v0.1.0", "0.1.0", &DRAWS, &probe);
    let mut app = app_on(&fixture, &github);
    install(&mut app, RELEASE);

    let row = check(&mut app);
    assert_eq!(row.update_check, Some(UpdateCheck::Newest("0.1.0".into())));
    assert_eq!(row.newer, None);

    // A newest release with a lower version isn't offered.
    publish(&github, "v0.0.9", "0.0.9", &DRAWS, &probe);
    let row = check(&mut app);
    assert_eq!(row.update_check, Some(UpdateCheck::Newest("0.1.0".into())));
    assert_eq!(row.newer, None);
    assert!(
        github
            .requests()
            .iter()
            .any(|url| url.ends_with("/repos/maker/probe-plugin/releases/latest")),
        "{:?}",
        github.requests()
    );

    publish(&github, "v0.2.0", "0.2.0", &DRAWS, &probe);
    let row = check(&mut app);
    assert_eq!(row.update_check, None);
    assert_eq!(row.newer.as_deref(), Some("0.2.0"));
    assert_eq!(row.update, Vec::<CapabilityLine>::new());
    assert_eq!(row.version.as_deref(), Some("0.1.0"));
}

#[test]
fn an_update_that_adds_nothing_installs_once_you_confirm_it() {
    let (Some(probe), Some(hello)) = (
        plugins::built("probe_plugin"),
        plugins::built("hello_plugin"),
    ) else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let github = FakeGitHub::new();
    publish(&github, "v0.1.0", "0.1.0", &DRAWS, &probe);
    let mut app = app_on(&fixture, &github);
    install(&mut app, RELEASE);
    assert!(app.set_plugin_enabled("probe", true));
    settle(&mut app);
    // The new build draws the hello plugin's page.
    publish(&github, "v0.2.0", "0.2.0", &DRAWS, &hello);

    check(&mut app);
    assert_eq!(first_plugin_page(&app).as_deref(), Some("Probe"));

    assert!(app.approve_plugin_update("probe"));
    settle(&mut app);

    let row = probe_row(&app);
    assert_eq!(row.version.as_deref(), Some("0.2.0"));
    assert_eq!(
        row.release.as_deref(),
        Some("https://github.com/maker/probe-plugin/releases/tag/v0.2.0")
    );
    assert_eq!(row.approved, drawing_lines());
    assert_eq!(row.newer, None);
    assert_eq!(first_plugin_page(&app).as_deref(), Some("Hello"));
}

#[test]
fn an_update_that_adds_capabilities_lists_only_the_additions_and_waits_for_approval() {
    let (Some(probe), Some(hello)) = (
        plugins::built("probe_plugin"),
        plugins::built("hello_plugin"),
    ) else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let github = FakeGitHub::new();
    publish(
        &github,
        "v0.1.0",
        "0.1.0",
        &[
            DRAWS[0],
            DRAWS[1],
            "name = \"network\"\nhosts = [\"api.example.com\"]",
        ],
        &probe,
    );
    let mut app = app_on(&fixture, &github);
    install(&mut app, RELEASE);
    assert!(app.set_plugin_enabled("probe", true));
    settle(&mut app);
    let approved = probe_row(&app).approved;
    publish(
        &github,
        "v0.2.0",
        "0.2.0",
        &[
            DRAWS[0],
            DRAWS[1],
            "name = \"network\"\nhosts = [\"api.example.com\", \"tiles.example.com\"]",
            "name = \"commands\"\ntargets = [\"tasks/add\"]",
        ],
        &hello,
    );

    let row = check(&mut app);

    assert_eq!(row.newer.as_deref(), Some("0.2.0"));
    assert_eq!(
        row.update,
        [
            CapabilityLine {
                name: "network".into(),
                description: "Connect to tiles.example.com.".into(),
                grant: Grant::Host("tiles.example.com".into()),
            },
            line(
                "commands",
                "Add commands to the command palette and offer them to other plugins.",
            ),
            CapabilityLine {
                name: "commands".into(),
                description: "Run the add command of the tasks plugin.".into(),
                grant: Grant::Target("tasks/add".into()),
            },
        ]
    );
    // The installed build keeps running with what you approved.
    assert_eq!(row.version.as_deref(), Some("0.1.0"));
    assert_eq!(row.approved, approved);
    assert!(row.running);
    assert_eq!(first_plugin_page(&app).as_deref(), Some("Probe"));

    assert!(app.decline_plugin_update("probe"));
    settle(&mut app);
    let row = probe_row(&app);
    assert_eq!(row.newer, None);
    assert_eq!(row.version.as_deref(), Some("0.1.0"));
    assert_eq!(row.approved, approved);
    assert_eq!(first_plugin_page(&app).as_deref(), Some("Probe"));

    check(&mut app);
    assert!(app.approve_plugin_update("probe"));
    settle(&mut app);
    let row = probe_row(&app);
    assert_eq!(row.version.as_deref(), Some("0.2.0"));
    assert_eq!(row.approved.len(), approved.len() + 3, "{:?}", row.approved);
    assert_eq!(first_plugin_page(&app).as_deref(), Some("Hello"));
}

#[test]
fn a_failed_download_leaves_the_installed_version_and_its_approvals() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let github = FakeGitHub::new();
    publish(&github, "v0.1.0", "0.1.0", &DRAWS, &probe);
    let mut app = app_on(&fixture, &github);
    install(&mut app, RELEASE);
    let saved = records(&fixture);
    let library = plugins::files_under(&fixture.app_data.path().join("plugins/library"));
    publish(
        &github,
        "v0.2.0",
        "0.2.0",
        &[DRAWS[0], DRAWS[1], "name = \"tools\""],
        &probe,
    );

    let failures: [fn(&FakeGitHub); 2] = [
        |github| github.fail_downloads(Some(500)),
        |github| {
            github.fail_downloads(None);
            github.tamper_with_downloads();
        },
    ];
    for fail in failures {
        fail(&github);

        let row = check(&mut app);

        let Some(UpdateCheck::Failed(problem)) = &row.update_check else {
            panic!("the check didn't fail: {row:?}");
        };
        assert!(!problem.is_empty());
        assert_eq!(row.newer, None);
        assert_eq!(row.version.as_deref(), Some("0.1.0"));
        assert_eq!(row.approved, drawing_lines());
        assert_eq!(records(&fixture), saved);
        assert_eq!(
            plugins::files_under(&fixture.app_data.path().join("plugins/library")),
            library
        );
    }
}

#[cfg(unix)]
#[test]
fn a_failed_install_of_an_update_leaves_the_installed_version_and_its_approvals() {
    use std::os::unix::fs::PermissionsExt;

    let (Some(probe), Some(hello)) = (
        plugins::built("probe_plugin"),
        plugins::built("hello_plugin"),
    ) else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let github = FakeGitHub::new();
    publish(&github, "v0.1.0", "0.1.0", &DRAWS, &probe);
    let mut app = app_on(&fixture, &github);
    install(&mut app, RELEASE);
    assert!(app.set_plugin_enabled("probe", true));
    settle(&mut app);
    publish(
        &github,
        "v0.2.0",
        "0.2.0",
        &[DRAWS[0], DRAWS[1], "name = \"tools\""],
        &hello,
    );
    check(&mut app);
    let saved = records(&fixture);
    let library = fixture.app_data.path().join("plugins/library");
    std::fs::set_permissions(&library, std::fs::Permissions::from_mode(0o555)).unwrap();

    let approved = app.approve_plugin_update("probe");
    settle(&mut app);
    std::fs::set_permissions(&library, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(!approved);
    let row = probe_row(&app);
    assert_eq!(row.version.as_deref(), Some("0.1.0"));
    assert_eq!(row.approved, drawing_lines());
    assert_eq!(records(&fixture), saved);
    assert!(row.running);
    assert_eq!(first_plugin_page(&app).as_deref(), Some("Probe"));
    // The update still waits, so you can try again.
    assert_eq!(row.newer.as_deref(), Some("0.2.0"));
    assert!(app.approve_plugin_update("probe"));
    settle(&mut app);
    assert_eq!(probe_row(&app).version.as_deref(), Some("0.2.0"));
}

#[test]
fn only_a_plugin_installed_from_github_checks_for_updates() {
    let Some(folder) = plugins::PluginFolder::hello() else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let github = FakeGitHub::new();
    let mut app = app_on(&fixture, &github);
    plugins::install(&mut app, folder.path());

    assert!(!app.check_plugin_update("hello"));

    let row = installed(&app).remove(0);
    assert_eq!(row.release, None);
    assert_eq!(row.update_check, None);
    assert_eq!(github.requests(), Vec::<String>::new());
}
