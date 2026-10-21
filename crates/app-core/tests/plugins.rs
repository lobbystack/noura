//! Seam 1: the plugins service against a real workspace folder and app
//! data. Plugins install from folders once you approve their capabilities,
//! keep their approvals on this device, turn on and off per workspace,
//! and reload when a new build lands in their folder.

mod support;

use app_core::{App, CapabilityLine, Grant, OpenAs, PluginRow, settings_section};
use support::{
    Fixture,
    plugins::{self, HELLO_PAGE, PluginFolder},
    settle,
};

const MANIFEST: &str = ".noura/workspace.yaml";

fn line(name: &str, description: &str) -> CapabilityLine {
    CapabilityLine {
        name: name.into(),
        description: description.into(),
        grant: Grant::Capability(name.into()),
    }
}

fn host_line(host: &str) -> CapabilityLine {
    CapabilityLine {
        name: "network".into(),
        description: format!("Connect to {host}."),
        grant: Grant::Host(host.into()),
    }
}

fn hello_capabilities() -> Vec<CapabilityLine> {
    vec![
        line(
            "workspace-read",
            "Read the files and folders in your workspace.",
        ),
        line("views", "Add pages to the rail and show panels in them."),
    ]
}

/// The plugin IDs in the workspace manifest's enabled plugins list.
fn enabled_in_manifest(fixture: &Fixture) -> Vec<String> {
    let manifest: local_core::WorkspaceManifest =
        serde_yaml_ng::from_slice(&fixture.read(MANIFEST)).unwrap();
    manifest.enabled_plugins
}

fn rail_titles(app: &App) -> Vec<String> {
    app.rail().iter().map(|page| page.title.clone()).collect()
}

/// The Plugins page's rows for plugins installed on this device. A new
/// workspace also lists Noura's own plugins as enabled, which no device
/// has installed yet.
fn installed(app: &App) -> Vec<PluginRow> {
    app.plugins()
        .into_iter()
        .filter(|row| row.installed)
        .collect()
}

fn enabled(fixture: &Fixture, id: &str) -> bool {
    enabled_in_manifest(fixture)
        .iter()
        .any(|enabled| enabled == id)
}

fn row<'a>(rows: &'a [PluginRow], id: &str) -> &'a PluginRow {
    rows.iter()
        .find(|row| row.id == id)
        .unwrap_or_else(|| panic!("no row for {id}: {rows:?}"))
}

/// The workspace's files and folders, `.noura` included, with their bytes.
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

/// The workspace manifest apart from what turning a plugin on or off
/// changes: the enabled plugins list and the time of the change.
fn manifest_apart_from_enabled(fixture: &Fixture) -> local_core::WorkspaceManifest {
    let mut manifest: local_core::WorkspaceManifest =
        serde_yaml_ng::from_slice(&fixture.read(MANIFEST)).unwrap();
    manifest.enabled_plugins.clear();
    manifest.updated.clear();
    manifest
}

/// The files Noura keeps in `.noura` other than the manifest, the index and
/// locks, with their bytes.
fn other_noura_files(fixture: &Fixture) -> Vec<(String, Vec<u8>)> {
    everything(fixture)
        .into_iter()
        .filter(|(path, _)| path.starts_with(".noura/") && path != MANIFEST)
        .collect()
}

#[test]
fn the_install_dialog_lists_every_capability_before_any_code_runs() {
    let Some(folder) = PluginFolder::hello() else {
        return;
    };
    let fixture = Fixture::new(&[("Plan.md", b"plan")]);
    let mut app = fixture.app();

    app.review_plugin_folder(folder.path().to_owned());

    let review = app.plugin_install().expect("the install dialog opens");
    assert_eq!(review.problem, None);
    assert_eq!(review.name, "Hello");
    assert_eq!(review.id, "hello");
    assert_eq!(review.version, "0.1.0");
    assert_eq!(review.folder, folder.path().to_str().unwrap());
    assert_eq!(review.capabilities, hello_capabilities());
    // Nothing is installed or running yet.
    assert!(installed(&app).is_empty());
    assert_eq!(rail_titles(&app), ["Inbox", "Files"]);
    assert_eq!(
        plugins::files_under(&fixture.app_data.path().join("plugins")),
        Vec::<String>::new()
    );
}

#[test]
fn declining_leaves_no_installed_plugin_compiled_code_or_approval() {
    let Some(folder) = PluginFolder::hello() else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    app.review_plugin_folder(folder.path().to_owned());

    app.cancel_plugin_install();

    assert!(app.plugin_install().is_none());
    assert!(installed(&app).is_empty());
    assert_eq!(
        plugins::files_under(&fixture.app_data.path().join("plugins")),
        Vec::<String>::new()
    );
    drop(app);
    let app = fixture.app();
    assert!(installed(&app).is_empty());
}

#[test]
fn a_folder_the_host_refuses_shows_why_and_installs_nothing() {
    let folder = PluginFolder::new(
        &plugins::manifest("overreach", "plugin.wasm", &["name = \"read-keychain\""]),
        "plugin.wasm",
        b"\0asm",
    );
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();

    app.review_plugin_folder(folder.path().to_owned());

    let review = app.plugin_install().expect("the install dialog opens");
    let problem = review.problem.clone().expect("the dialog says why");
    assert!(problem.contains("read-keychain"), "{problem}");
    assert!(review.capabilities.is_empty());
    assert!(!app.approve_plugin_install());
    assert!(installed(&app).is_empty());
}

#[test]
fn approving_installs_the_plugin_on_this_device_and_changes_no_workspace_file() {
    let Some(folder) = PluginFolder::hello() else {
        return;
    };
    let fixture = Fixture::new(&[("Plan.md", b"plan")]);
    let mut app = fixture.app();
    let before = everything(&fixture);

    plugins::install(&mut app, folder.path());

    assert!(app.plugin_install().is_none());
    let rows = installed(&app);
    assert_eq!(
        rows,
        [PluginRow {
            id: "hello".into(),
            name: "Hello".into(),
            version: Some("0.1.0".into()),
            folder: Some(folder.path().to_str().unwrap().to_owned()),
            approved: hello_capabilities(),
            revoked: Vec::new(),
            installed: true,
            shipped: false,
            enabled: false,
            running: false,
            failed: false,
            problem: None,
            update: Vec::new(),
        }]
    );
    assert_eq!(everything(&fixture), before);
    // The approvals live in this device's app data.
    let records = std::fs::read_to_string(
        fixture
            .app_data
            .path()
            .join("plugins")
            .join("installed.json"),
    )
    .unwrap();
    assert!(records.contains("workspace-read"), "{records}");
    drop(app);
    assert_eq!(installed(&fixture.app()), rows);
}

#[test]
fn turning_a_plugin_on_writes_the_enabled_list_durably_and_adds_its_page() {
    let Some(folder) = PluginFolder::hello() else {
        return;
    };
    let fixture = Fixture::new(&[("Plan.md", b"plan"), ("Notes/Idea.md", b"idea")]);
    let mut app = fixture.app();
    plugins::install(&mut app, folder.path());
    let files = fixture.snapshot();
    let manifest = manifest_apart_from_enabled(&fixture);
    let noura = other_noura_files(&fixture);

    let (turned_on, synced) =
        local_core::durability_probe::record(|| app.set_plugin_enabled("hello", true));
    settle(&mut app);

    assert!(turned_on);
    assert!(enabled(&fixture, "hello"));
    assert!(
        synced.contains(&fixture.root.join(".noura")),
        "the manifest's folder wasn't synced: {synced:?}"
    );
    assert_eq!(fixture.snapshot(), files);
    assert_eq!(manifest_apart_from_enabled(&fixture), manifest);
    assert_eq!(other_noura_files(&fixture), noura);
    assert_eq!(rail_titles(&app), ["Inbox", "Files", "Hello"]);
    assert_eq!(
        plugins::texts(&app, HELLO_PAGE),
        Ok(vec![
            "This workspace has 2 files.".to_owned(),
            "Count: 0".to_owned()
        ])
    );
    let rows = app.plugins();
    assert!(row(&rows, "hello").enabled && row(&rows, "hello").running);
    // It runs again at the next launch.
    drop(app);
    let app = fixture.app();
    assert_eq!(rail_titles(&app), ["Inbox", "Files", "Hello"]);
}

#[test]
fn a_button_press_reaches_the_plugin_and_its_page_redraws() {
    let Some(folder) = PluginFolder::hello() else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    plugins::install(&mut app, folder.path());
    assert!(app.set_plugin_enabled("hello", true));
    settle(&mut app);

    app.press_plugin_button(HELLO_PAGE, "add-one");
    app.press_plugin_button(HELLO_PAGE, "add-one");
    settle(&mut app);

    assert_eq!(
        plugins::texts(&app, HELLO_PAGE),
        Ok(vec![
            "This workspace has 0 files.".to_owned(),
            "Count: 2".to_owned()
        ])
    );
}

#[test]
fn turning_a_plugin_off_stops_it_and_removes_its_page_leaving_every_other_file_alone() {
    let Some(folder) = PluginFolder::hello() else {
        return;
    };
    let fixture = Fixture::new(&[("Plan.md", b"plan")]);
    let mut app = fixture.app();
    plugins::install(&mut app, folder.path());
    assert!(app.set_plugin_enabled("hello", true));
    settle(&mut app);
    app.open_page(HELLO_PAGE, OpenAs::Kept);
    assert_eq!(app.current_page(), HELLO_PAGE);
    let files = fixture.snapshot();
    let manifest = manifest_apart_from_enabled(&fixture);
    let noura = other_noura_files(&fixture);

    let (turned_off, synced) =
        local_core::durability_probe::record(|| app.set_plugin_enabled("hello", false));

    assert!(turned_off);
    assert!(!enabled(&fixture, "hello"));
    assert!(synced.contains(&fixture.root.join(".noura")));
    assert_eq!(fixture.snapshot(), files);
    assert_eq!(manifest_apart_from_enabled(&fixture), manifest);
    assert_eq!(other_noura_files(&fixture), noura);
    assert_eq!(rail_titles(&app), ["Inbox", "Files"]);
    assert!(app.plugin_view(HELLO_PAGE).is_none());
    assert!(app.tabs().is_empty(), "{:?}", app.tabs());
    let rows = app.plugins();
    assert!(!row(&rows, "hello").enabled && !row(&rows, "hello").running);
    assert!(row(&rows, "hello").installed);
}

#[test]
fn a_fresh_device_shows_the_plugin_enabled_but_not_installed_and_runs_none_of_its_code() {
    let Some(folder) = PluginFolder::hello() else {
        return;
    };
    let fixture = Fixture::new(&[("Plan.md", b"plan")]);
    let mut app = fixture.app();
    plugins::install(&mut app, folder.path());
    assert!(app.set_plugin_enabled("hello", true));
    settle(&mut app);
    let root = app.workspace().unwrap().root().to_owned();
    drop(app);
    // Approval-shaped data and the plugin's code planted in the workspace.
    let plugins_data = fixture.app_data.path().join("plugins");
    let records = std::fs::read(plugins_data.join("installed.json")).unwrap();
    for planted in [".noura/plugins/installed.json", "plugins/installed.json"] {
        support::write(&fixture.root, planted, &records);
    }
    for file in plugins::files_under(&plugins_data) {
        let bytes = std::fs::read(plugins_data.join(&file)).unwrap();
        support::write(&fixture.root, &format!(".noura/plugins/{file}"), &bytes);
    }

    let fresh = tempfile::tempdir().unwrap();
    let mut app = App::launch(fresh.path());
    app.open_folder(root.into());
    settle(&mut app);

    assert!(installed(&app).is_empty());
    assert_eq!(
        row(&app.plugins(), "hello"),
        &PluginRow {
            id: "hello".into(),
            name: "hello".into(),
            version: None,
            folder: None,
            approved: Vec::new(),
            revoked: Vec::new(),
            installed: false,
            shipped: false,
            enabled: true,
            running: false,
            failed: false,
            problem: None,
            update: Vec::new(),
        }
    );
    assert_eq!(rail_titles(&app), ["Inbox", "Files"]);
    assert!(app.plugin_view(HELLO_PAGE).is_none());
    assert_eq!(
        plugins::files_under(&fresh.path().join("plugins")),
        Vec::<String>::new()
    );
}

#[test]
fn uninstalling_removes_code_cache_and_approvals_and_leaves_the_workspace_byte_identical() {
    let Some(folder) = PluginFolder::hello() else {
        return;
    };
    let fixture = Fixture::new(&[("Plan.md", b"plan")]);
    let mut app = fixture.app();
    plugins::install(&mut app, folder.path());
    assert!(app.set_plugin_enabled("hello", true));
    settle(&mut app);
    let before = everything(&fixture);

    assert!(app.uninstall_plugin("hello"));

    assert_eq!(everything(&fixture), before);
    assert!(enabled(&fixture, "hello"));
    let left = plugins::files_under(&fixture.app_data.path().join("plugins"));
    assert!(
        left.iter().all(|file| !file.starts_with("library/hello")),
        "{left:?}"
    );
    let records = std::fs::read_to_string(
        fixture
            .app_data
            .path()
            .join("plugins")
            .join("installed.json"),
    )
    .unwrap();
    assert!(!records.contains("hello"), "{records}");
    let rows = app.plugins();
    assert!(!row(&rows, "hello").installed && row(&rows, "hello").enabled);
    assert_eq!(rail_titles(&app), ["Inbox", "Files"]);
    // The folder it came from keeps its files.
    assert!(folder.path().join("hello_plugin.wasm").is_file());
}

#[test]
fn a_new_build_saved_in_the_folder_reloads_and_the_open_page_redraws() {
    let (Some(folder), Some(probe)) = (PluginFolder::hello(), plugins::built("probe_plugin"))
    else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    plugins::install(&mut app, folder.path());
    assert!(app.set_plugin_enabled("hello", true));
    settle(&mut app);
    app.open_page(HELLO_PAGE, OpenAs::Kept);

    folder.write("hello_plugin.wasm", &probe);

    plugins::wait_until(&mut app, "the new build draws the page", |app| {
        plugins::texts(app, HELLO_PAGE) == Ok(vec!["Probe is running.".to_owned()])
    });
    assert_eq!(app.current_page(), HELLO_PAGE);
    assert_eq!(
        rail_titles(&app),
        [
            "Inbox",
            "Files",
            "Probe",
            "Workspace",
            "Markdown",
            "Deep",
            "Broken",
            "Events"
        ]
    );
    assert_eq!(app.active_tab().unwrap().title(), "Probe");
}

#[test]
fn a_reload_that_declares_a_capability_you_havent_approved_waits_for_approval() {
    let (Some(folder), Some(probe)) = (PluginFolder::hello(), plugins::built("probe_plugin"))
    else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    plugins::install(&mut app, folder.path());
    assert!(app.set_plugin_enabled("hello", true));
    settle(&mut app);
    let wider = format!(
        "{}\n[[capabilities]]\nname = \"network\"\nhosts = [\"api.example.com\"]\n",
        plugins::HELLO_MANIFEST
    );

    folder.write("hello_plugin.wasm", &probe);
    folder.write("plugin.toml", wider.as_bytes());

    let network = host_line("api.example.com");
    plugins::wait_until(&mut app, "the update asks for approval", |app| {
        row(&app.plugins(), "hello").update == [network.clone()]
    });
    // The build you approved keeps running meanwhile.
    assert_eq!(
        plugins::texts(&app, HELLO_PAGE),
        Ok(vec![
            "This workspace has 0 files.".to_owned(),
            "Count: 0".to_owned()
        ])
    );

    assert!(app.approve_plugin_update("hello"));
    settle(&mut app);

    assert_eq!(
        plugins::texts(&app, HELLO_PAGE),
        Ok(vec!["Probe is running.".to_owned()])
    );
    let rows = app.plugins();
    let hello = row(&rows, "hello");
    assert!(hello.update.is_empty());
    assert!(hello.approved.contains(&network));
}

#[test]
fn declining_an_update_keeps_the_installed_build_and_its_approvals() {
    let (Some(folder), Some(probe)) = (PluginFolder::hello(), plugins::built("probe_plugin"))
    else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    plugins::install(&mut app, folder.path());
    assert!(app.set_plugin_enabled("hello", true));
    settle(&mut app);
    let wider = format!(
        "{}\n[[capabilities]]\nname = \"tools\"\n",
        plugins::HELLO_MANIFEST
    );
    folder.write("hello_plugin.wasm", &probe);
    folder.write("plugin.toml", wider.as_bytes());
    plugins::wait_until(&mut app, "the update asks for approval", |app| {
        !row(&app.plugins(), "hello").update.is_empty()
    });

    assert!(app.decline_plugin_update("hello"));
    settle(&mut app);

    let rows = app.plugins();
    let hello = row(&rows, "hello");
    assert!(hello.update.is_empty());
    assert_eq!(hello.approved, hello_capabilities());
    assert_eq!(hello.version.as_deref(), Some("0.1.0"));
    assert_eq!(
        plugins::texts(&app, HELLO_PAGE),
        Ok(vec![
            "This workspace has 0 files.".to_owned(),
            "Count: 0".to_owned()
        ])
    );
    assert!(!app.decline_plugin_update("hello"));
}

/// The probe's manifest with `views`, the network hosts `hosts` and the
/// command targets `targets`.
fn probe_manifest(hosts: &str, targets: &str) -> String {
    plugins::manifest(
        "probe",
        "plugin.wasm",
        &[
            "name = \"views\"",
            &format!("name = \"network\"\nhosts = {hosts}"),
            &format!("name = \"commands\"\ntargets = {targets}"),
        ],
    )
}

#[test]
fn an_update_that_adds_a_network_host_or_a_command_target_waits_for_approval() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let folder = PluginFolder::new(
        &probe_manifest("[\"api.example.com\"]", "[\"tasks\"]"),
        "plugin.wasm",
        &probe,
    );
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    plugins::install(&mut app, folder.path());
    assert!(app.set_plugin_enabled("probe", true));
    settle(&mut app);

    folder.write(
        "plugin.toml",
        probe_manifest(
            "[\"api.example.com\", \"tiles.example.com\"]",
            "[\"tasks\"]",
        )
        .as_bytes(),
    );

    let tiles = host_line("tiles.example.com");
    plugins::wait_until(&mut app, "the new host waits for approval", |app| {
        row(&app.plugins(), "probe").update == [tiles.clone()]
    });
    assert!(!row(&app.plugins(), "probe").approved.contains(&tiles));
    assert!(app.approve_plugin_update("probe"));
    settle(&mut app);
    assert!(row(&app.plugins(), "probe").approved.contains(&tiles));

    folder.write(
        "plugin.toml",
        probe_manifest(
            "[\"api.example.com\", \"tiles.example.com\"]",
            "[\"projects/create\", \"tasks\"]",
        )
        .as_bytes(),
    );

    let create = CapabilityLine {
        name: "commands".into(),
        description: "Run the create command of the projects plugin.".into(),
        grant: Grant::Target("projects/create".into()),
    };
    plugins::wait_until(&mut app, "the new target waits for approval", |app| {
        row(&app.plugins(), "probe").update == [create.clone()]
    });
    assert!(app.approve_plugin_update("probe"));
    settle(&mut app);
    assert!(row(&app.plugins(), "probe").approved.contains(&create));
}

#[test]
fn an_update_that_only_removes_capabilities_runs_without_approval() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let folder = PluginFolder::new(
        &plugins::manifest(
            "probe",
            "plugin.wasm",
            &[
                "name = \"workspace-read\"",
                "name = \"views\"",
                "name = \"network\"\nhosts = [\"api.example.com\", \"tiles.example.com\"]",
            ],
        ),
        "plugin.wasm",
        &probe,
    );
    let fixture = Fixture::new(&[("Plan.md", b"plan")]);
    let mut app = fixture.app();
    plugins::install(&mut app, folder.path());
    assert!(app.set_plugin_enabled("probe", true));
    settle(&mut app);
    let workspace = |app: &App| plugins::texts(app, "plugin:probe:workspace").unwrap();
    assert!(workspace(&app)[0].starts_with("file Plan.md"));

    folder.write(
        "plugin.toml",
        plugins::manifest(
            "probe",
            "plugin.wasm",
            &[
                "name = \"views\"",
                "name = \"network\"\nhosts = [\"api.example.com\"]",
            ],
        )
        .as_bytes(),
    );

    plugins::wait_until(&mut app, "the narrower build runs", |app| {
        workspace(app)[0] == "error capability-not-declared workspace-read"
    });
    let rows = app.plugins();
    let probe = row(&rows, "probe");
    assert!(probe.update.is_empty());
    assert_eq!(
        probe.approved,
        [
            line("views", "Add pages to the rail and show panels in them."),
            host_line("api.example.com"),
        ]
    );
    assert!(probe.revoked.is_empty());
}

#[cfg(unix)]
#[test]
fn a_new_build_that_fails_to_install_installs_once_you_save_it_again() {
    use std::os::unix::fs::PermissionsExt;

    let (Some(folder), Some(probe)) = (PluginFolder::hello(), plugins::built("probe_plugin"))
    else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    plugins::install(&mut app, folder.path());
    assert!(app.set_plugin_enabled("hello", true));
    settle(&mut app);
    let library = fixture.app_data.path().join("plugins").join("library");
    let set_mode = |mode| {
        std::fs::set_permissions(&library, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    // The plugin library can't take a new build.
    set_mode(0o555);

    folder.write("hello_plugin.wasm", &probe);

    plugins::wait_until(&mut app, "the new build fails to install", |app| {
        row(&app.plugins(), "hello").problem.is_some()
    });
    let rows = app.plugins();
    let problem = row(&rows, "hello").problem.clone().unwrap();
    assert!(
        problem.ends_with("Save the build again to retry."),
        "{problem}"
    );
    // The build that installed keeps running.
    assert_eq!(
        plugins::texts(&app, HELLO_PAGE),
        Ok(vec![
            "This workspace has 0 files.".to_owned(),
            "Count: 0".to_owned()
        ])
    );

    set_mode(0o755);
    folder.write("hello_plugin.wasm", &probe);

    plugins::wait_until(&mut app, "the build saved again installs", |app| {
        plugins::texts(app, HELLO_PAGE) == Ok(vec!["Probe is running.".to_owned()])
    });
    assert_eq!(row(&app.plugins(), "hello").problem, None);
}

#[test]
fn an_unreadable_approvals_file_runs_no_plugin_and_the_plugin_installs_again() {
    let Some(folder) = PluginFolder::hello() else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    plugins::install(&mut app, folder.path());
    assert!(app.set_plugin_enabled("hello", true));
    settle(&mut app);
    drop(app);
    let records = fixture
        .app_data
        .path()
        .join("plugins")
        .join("installed.json");
    std::fs::write(&records, b"{\"plugins\": {\"hello\": ").unwrap();

    let mut app = fixture.app();

    assert_eq!(rail_titles(&app), ["Inbox", "Files"]);
    assert!(app.plugin_view(HELLO_PAGE).is_none());
    let rows = app.plugins();
    assert!(!row(&rows, "hello").installed && row(&rows, "hello").enabled);
    // The folder it came from installs it again, in place of the copy
    // that lost its approvals.
    plugins::install(&mut app, folder.path());
    assert_eq!(rail_titles(&app), ["Inbox", "Files", "Hello"]);
    assert!(row(&app.plugins(), "hello").running);
}

#[test]
fn plugins_read_entries_files_and_objects_of_the_real_workspace() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let note = support::note("01jbcdefghjkmnpqrstvwxyz22", "Grow tomatoes");
    let fixture = Fixture::new(&[("Notes/Plan.md", &note), ("Readme.txt", b"Hello")]);
    let folder = PluginFolder::new(
        &plugins::manifest(
            "probe",
            "plugin.wasm",
            &["name = \"workspace-read\"", "name = \"views\""],
        ),
        "plugin.wasm",
        &probe,
    );
    let mut app = fixture.app();
    plugins::install(&mut app, folder.path());
    assert!(app.set_plugin_enabled("probe", true));
    settle(&mut app);

    let lines = plugins::texts(&app, "plugin:probe:workspace").unwrap();

    let id = "note_01jbcdefghjkmnpqrstvwxyz22";
    let revision = |line: &str| {
        line.split(" revision ")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .unwrap()
            .to_owned()
    };
    assert_eq!(lines.len(), 8, "{lines:#?}");
    assert_eq!(lines[0], "folder Notes object - type - revision -");
    assert!(
        lines[1].starts_with(&format!(
            "file Notes/Plan.md object {id} type note revision "
        )),
        "{}",
        lines[1]
    );
    // Noura indexes only Markdown, so the listing has no revision for other
    // files until a plugin reads them.
    assert_eq!(lines[2], "file Readme.txt object - type - revision -");
    assert!(
        lines[3].starts_with("file Notes/Plan.md revision "),
        "{}",
        lines[3]
    );
    assert!(lines[3].contains("Grow tomatoes"), "{}", lines[3]);
    assert!(
        lines[4].starts_with(&format!(
            "object {id} type note title Plan path Notes/Plan.md revision "
        )),
        "{}",
        lines[4]
    );
    assert!(lines[4].ends_with("body Grow tomatoes"), "{}", lines[4]);
    // The listing, the file and the object agree on the content revision.
    assert_eq!(revision(&lines[1]), revision(&lines[3]));
    assert_eq!(revision(&lines[1]), revision(&lines[4]));
    assert!(
        lines[5].starts_with("file Readme.txt revision "),
        "{}",
        lines[5]
    );
    assert!(lines[5].ends_with("text Hello"), "{}", lines[5]);
    assert_ne!(revision(&lines[5]), revision(&lines[3]));
    assert_eq!(lines[6], "error invalid-path .noura/workspace.yaml");
    assert_eq!(lines[7], "error invalid-path ../outside.md");
}

#[cfg(target_os = "linux")]
#[test]
fn a_name_a_wit_string_cant_hold_lists_as_the_unsupported_path_error() {
    use std::os::unix::ffi::OsStrExt;

    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let fixture = Fixture::new(&[("Plan.md", b"plan")]);
    std::fs::write(
        fixture
            .root
            .join(std::ffi::OsStr::from_bytes(b"caf\xe9.md")),
        b"latin-1",
    )
    .unwrap();
    let folder = PluginFolder::new(
        &plugins::manifest(
            "probe",
            "plugin.wasm",
            &["name = \"workspace-read\"", "name = \"views\""],
        ),
        "plugin.wasm",
        &probe,
    );
    let mut app = fixture.app();
    plugins::install(&mut app, folder.path());
    assert!(app.set_plugin_enabled("probe", true));
    settle(&mut app);

    let lines = plugins::texts(&app, "plugin:probe:workspace").unwrap();

    assert!(
        lines.iter().any(|line| line == "error unsupported-path"),
        "{lines:#?}"
    );
    assert!(
        lines.iter().all(|line| !line.contains('\u{fffd}')),
        "a lossy name crossed: {lines:#?}"
    );
}

#[test]
fn settings_lists_a_plugins_section() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();

    app.open_settings_at(settings_section::PLUGINS);

    assert_eq!(app.settings().unwrap().section, settings_section::PLUGINS);
}
