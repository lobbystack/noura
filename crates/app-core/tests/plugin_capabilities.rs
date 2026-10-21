//! Seam 1: what running plugins add through the plugins service, against
//! a real workspace folder and app data: commands in the command palette,
//! Inbox sections, pages in Settings and tools; the capabilities, hosts
//! and targets you revoke; the plugins that ship with Noura; and plugins
//! that loop, outgrow their memory or keep failing, which stop without
//! holding up a window.

mod support;

use std::time::{Duration, Instant};

use app_core::{
    App, CapabilityLine, CommandSource, Grant, PluginTool, SettingsGroup, ViewKind, ViewNode,
    command_id, settings_section,
};
use support::{
    Fixture,
    plugins::{self, PluginFolder},
    settle,
};

fn probe(capabilities: &[&str]) -> Option<PluginFolder> {
    plugins::folder_for("probe_plugin", "probe", capabilities)
}

fn misbehave(capabilities: &[&str]) -> Option<PluginFolder> {
    plugins::folder_for("misbehave_plugin", "misbehave", capabilities)
}

/// What the probe heard: its events page, one line per event, command or
/// tool call.
fn events(app: &App) -> Vec<String> {
    plugins::texts(app, "plugin:probe:events").expect("the events page draws")
}

/// The text nodes under `node`, in order.
fn texts_of(node: &ViewNode) -> Vec<String> {
    let mut texts = Vec::new();
    if let ViewKind::Text(text) = &node.kind {
        texts.push(text.clone());
    }
    for child in &node.children {
        texts.extend(texts_of(child));
    }
    texts
}

/// The Inbox's plugin sections as (title, their text lines or why they
/// couldn't draw).
fn sections(app: &App) -> Vec<(String, Result<Vec<String>, String>)> {
    app.inbox()
        .sections
        .into_iter()
        .map(|section| {
            let content = section.content.map(|view| texts_of(view.root()));
            (section.title, content)
        })
        .collect()
}

fn rail_titles(app: &App) -> Vec<String> {
    app.rail().iter().map(|page| page.title.clone()).collect()
}

fn palette_labels(app: &App) -> Vec<String> {
    app.palette_matches()
        .into_iter()
        .map(|command| command.label)
        .collect()
}

/// Poll until `done` holds, for up to 20 seconds, without waiting for
/// plugin work to finish.
fn poll_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(app) {
        app.poll();
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_plugin_command_shows_in_the_palette_with_its_title_and_runs_in_the_plugin_until_it_turns_off()
{
    let Some(folder) = probe(&["name = \"views\"", "name = \"commands\""]) else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    plugins::install_and_enable(&mut app, folder.path(), "probe");
    assert!(app.run_command(
        command_id::COMMAND_PALETTE,
        CommandSource::Menu,
        Instant::now()
    ));

    app.set_palette_query("say hello");
    assert_eq!(palette_labels(&app), ["Say hello"]);
    // Commands the probe offers only to other plugins stay out.
    app.set_palette_query("describe");
    assert!(palette_labels(&app).is_empty());
    app.set_palette_query("say hello");
    assert!(app.run_palette_selection(Instant::now()));
    settle(&mut app);

    assert_eq!(events(&app), ["command say-hello"]);

    assert!(app.set_plugin_enabled("probe", false));
    assert!(
        app.commands()
            .iter()
            .all(|command| command.label != "Say hello")
    );
}

#[test]
fn a_plugin_section_draws_in_the_inbox_while_the_plugin_is_on_and_hears_presses() {
    let Some(folder) = probe(&["name = \"views\"", "name = \"inbox-sections\""]) else {
        return;
    };
    let fixture = Fixture::new(&[("Plan.md", b"# Plan\n")]);
    let mut app = fixture.app();
    plugins::install_and_enable(&mut app, folder.path(), "probe");

    assert_eq!(
        sections(&app),
        [(
            "Probe section".to_owned(),
            Ok(vec!["Section probe is running.".to_owned()])
        )]
    );
    let section = &app.inbox().sections[0];
    assert_eq!(
        (section.plugin_id.as_str(), section.section_id.as_str()),
        ("probe", "probe")
    );
    app.press_inbox_button("probe", "probe", "press");
    settle(&mut app);
    assert_eq!(events(&app), ["inbox probe pressed press"]);

    assert!(app.set_plugin_enabled("probe", false));
    assert!(sections(&app).is_empty());
    assert_eq!(app.inbox().recent.len(), 1);
}

#[test]
fn a_section_that_fails_or_runs_past_its_deadline_shows_an_error_in_its_own_place_while_the_rest_draws()
 {
    let (Some(good), Some(slow)) = (
        probe(&["name = \"views\"", "name = \"inbox-sections\""]),
        misbehave(&["name = \"views\"", "name = \"inbox-sections\""]),
    ) else {
        return;
    };
    let fixture = Fixture::new(&[("Plan.md", b"# Plan\n")]);
    let mut app = fixture.app();
    plugins::install_and_enable(&mut app, slow.path(), "misbehave");
    plugins::install_and_enable(&mut app, good.path(), "probe");

    let shown = sections(&app);

    assert_eq!(shown.len(), 3, "{shown:?}");
    let error_in = |title: &str| {
        shown
            .iter()
            .find(|(shown_title, _)| shown_title == title)
            .unwrap_or_else(|| panic!("{title} has its place"))
            .1
            .clone()
            .expect_err("the section shows an error")
    };
    let reason = error_in("Looping section");
    assert!(reason.contains("1 second limit"), "{reason}");
    let reason = error_in("Broken section");
    assert!(reason.contains("lists child 7"), "{reason}");
    assert!(shown.contains(&(
        "Probe section".to_owned(),
        Ok(vec!["Section probe is running.".to_owned()])
    )));
    assert_eq!(app.inbox().recent.len(), 1);
}

#[test]
fn a_plugin_page_in_settings_draws_from_nodes_hears_presses_and_noura_stores_no_values() {
    let Some(folder) = probe(&["name = \"views\"", "name = \"settings\""]) else {
        return;
    };
    let fixture = Fixture::new(&[("Plan.md", b"# Plan\n")]);
    let mut app = fixture.app();
    plugins::install_and_enable(&mut app, folder.path(), "probe");
    let section = "plugin-settings:probe";
    let plugin_data = || {
        plugins::files_under(&fixture.app_data.path().join("plugins"))
            .into_iter()
            .map(|file| {
                let bytes = std::fs::read(fixture.app_data.path().join("plugins").join(&file));
                (file, bytes.unwrap())
            })
            .collect::<Vec<_>>()
    };
    let workspace = fixture.snapshot();
    let stored = plugin_data();

    let groups = app.settings_sections();
    let workspace_group = groups
        .iter()
        .find(|(group, _)| *group == SettingsGroup::Workspace)
        .map(|(_, sections)| {
            sections
                .iter()
                .map(|section| (section.id.clone(), section.title.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap();
    assert!(
        workspace_group.contains(&(section.to_owned(), "Probe settings".to_owned())),
        "{workspace_group:?}"
    );
    app.open_settings_at(section);
    assert_eq!(app.settings().unwrap().section, section);
    let texts = |app: &App| texts_of(app.plugin_settings_view(section).unwrap().unwrap().root());
    assert_eq!(texts(&app), ["Pressed 0 times"]);

    app.press_plugin_settings_button(section, "count");
    settle(&mut app);

    assert_eq!(texts(&app), ["Pressed 1 times"]);
    assert_eq!(fixture.snapshot(), workspace);
    assert_eq!(plugin_data(), stored);

    assert!(app.set_plugin_enabled("probe", false));
    assert_eq!(
        app.settings().unwrap().section,
        settings_section::PLUGINS,
        "Settings leaves the page that went"
    );
    assert!(
        app.settings_sections()
            .iter()
            .flat_map(|(_, sections)| sections)
            .all(|known| known.id != section)
    );
}

#[test]
fn plugin_tools_list_while_the_plugin_is_on_and_arguments_that_fail_the_schema_never_reach_it() {
    let Some(folder) = probe(&["name = \"views\"", "name = \"tools\""]) else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    plugins::install_and_enable(&mut app, folder.path(), "probe");

    let tools = app.plugin_tools();
    assert_eq!(tools.len(), 1);
    let PluginTool {
        plugin_id,
        name,
        description,
        input_schema,
    } = &tools[0];
    assert_eq!(
        (plugin_id.as_str(), name.as_str(), description.as_str()),
        ("probe", "echo", "Repeats the text it gets.")
    );
    assert!(
        input_schema.contains("\"required\":[\"text\"]"),
        "{input_schema}"
    );

    for arguments in [
        r#"{"text": 5}"#,
        "{}",
        r#"{"text": "hi", "more": 1}"#,
        "not json",
    ] {
        let error = app
            .call_plugin_tool("probe", "echo", arguments)
            .expect_err(arguments);
        assert_eq!(error.code, "plugin_tool_arguments_invalid", "{arguments}");
    }
    let error = app
        .call_plugin_tool("probe", "echo", r#"{"text": 5}"#)
        .unwrap_err();
    assert!(error.message.contains("/text"), "{}", error.message);
    settle(&mut app);
    assert!(events(&app).is_empty(), "{:?}", events(&app));

    assert_eq!(
        app.call_plugin_tool("probe", "echo", r#"{"text":"hi"}"#),
        Ok(r#"{"text":"hi"}"#.to_owned())
    );
    settle(&mut app);
    assert_eq!(events(&app), [r#"tool echo {"text":"hi"}"#]);

    assert!(app.set_plugin_enabled("probe", false));
    assert!(app.plugin_tools().is_empty());
    assert_eq!(
        app.call_plugin_tool("probe", "echo", r#"{"text":"hi"}"#)
            .unwrap_err()
            .code,
        "plugin_tool_unknown"
    );
}

fn line(name: &str, description: &str, grant: Grant) -> CapabilityLine {
    CapabilityLine {
        name: name.into(),
        description: description.into(),
        grant,
    }
}

fn capability(name: &str, description: &str) -> CapabilityLine {
    line(name, description, Grant::Capability(name.into()))
}

#[test]
fn the_plugins_page_lists_each_approval_and_a_revoked_one_takes_effect_on_the_next_call() {
    let Some(folder) = probe(&[
        "name = \"workspace-read\"",
        "name = \"views\"",
        "name = \"network\"\nhosts = [\"api.example.com\"]",
        "name = \"commands\"\ntargets = [\"tasks\"]",
        "name = \"tools\"",
    ]) else {
        return;
    };
    let fixture = Fixture::new(&[("Plan.md", b"# Plan\n")]);
    let mut app = fixture.app();
    plugins::install_and_enable(&mut app, folder.path(), "probe");
    let row = |app: &App| {
        app.plugins()
            .into_iter()
            .find(|row| row.id == "probe")
            .unwrap()
    };
    assert_eq!(
        row(&app).approved,
        [
            capability(
                "workspace-read",
                "Read the files and folders in your workspace."
            ),
            capability("views", "Add pages to the rail and show panels in them."),
            line(
                "network",
                "Connect to api.example.com.",
                Grant::Host("api.example.com".into())
            ),
            capability("commands", "Add commands to the command palette."),
            line(
                "commands",
                "Run the commands the tasks plugin offers.",
                Grant::Target("tasks".into())
            ),
            capability("tools", "Offer tools that AI assistants can use."),
        ]
    );
    let workspace = |app: &App| plugins::texts(app, "plugin:probe:workspace").unwrap();
    assert!(
        workspace(&app)[0].starts_with("file Plan.md"),
        "{:?}",
        workspace(&app)
    );

    assert!(app.revoke_plugin_grant("probe", &Grant::Capability("workspace-read".into())));
    settle(&mut app);

    let lines = workspace(&app);
    assert!(
        lines
            .iter()
            .all(|line| line == "error capability-not-declared workspace-read"),
        "{lines:?}"
    );
    let probe_row = row(&app);
    assert!(
        probe_row
            .approved
            .iter()
            .all(|line| line.name != "workspace-read")
    );
    assert_eq!(
        probe_row.revoked,
        [capability(
            "workspace-read",
            "Read the files and folders in your workspace."
        )]
    );

    // A host and a target go one at a time.
    assert!(app.revoke_plugin_grant("probe", &Grant::Host("api.example.com".into())));
    assert!(app.revoke_plugin_grant("probe", &Grant::Target("tasks".into())));
    let approved: Vec<Grant> = row(&app)
        .approved
        .into_iter()
        .map(|line| line.grant)
        .collect();
    assert!(!approved.contains(&Grant::Host("api.example.com".into())));
    assert!(!approved.contains(&Grant::Target("tasks".into())));
    assert!(approved.contains(&Grant::Capability("commands".into())));

    // Revoking a registration capability takes away what the plugin
    // registered under it.
    assert!(!app.plugin_tools().is_empty());
    assert!(app.revoke_plugin_grant("probe", &Grant::Capability("tools".into())));
    assert!(app.plugin_tools().is_empty());
    assert!(app.revoke_plugin_grant("probe", &Grant::Capability("views".into())));
    settle(&mut app);
    assert_eq!(rail_titles(&app), ["Inbox", "Files"]);
    assert!(app.plugin_view("plugin:probe:workspace").is_none());

    // The revocations stay on this device, and you can allow one again.
    drop(app);
    let mut app = fixture.app();
    assert_eq!(rail_titles(&app), ["Inbox", "Files"]);
    assert!(app.allow_plugin_grant("probe", &Grant::Capability("views".into())));
    settle(&mut app);
    assert!(rail_titles(&app).contains(&"Workspace".to_owned()));
    assert_eq!(
        workspace(&app)[0],
        "error capability-not-declared workspace-read"
    );
}

/// A folder of the plugins that ship with Noura, holding the hello plugin.
fn shipped_with_hello() -> Option<tempfile::TempDir> {
    let hello = plugins::built("hello_plugin")?;
    let shipped = tempfile::tempdir().unwrap();
    let folder = shipped.path().join("hello");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("plugin.toml"), plugins::HELLO_MANIFEST).unwrap();
    std::fs::write(folder.join("hello_plugin.wasm"), hello).unwrap();
    Some(shipped)
}

#[test]
fn the_first_run_screen_lists_shipped_plugins_and_one_approval_runs_them_on_this_device() {
    let Some(shipped) = shipped_with_hello() else {
        return;
    };
    let fixture = Fixture::new(&[("Plan.md", b"# Plan\n")]);
    let mut app = fixture.app();
    assert!(app.set_plugin_enabled("hello", true));
    app.set_shipped_plugins(shipped.path().to_owned());

    let review = app.shipped_plugins();

    assert_eq!(review.len(), 1);
    assert_eq!(
        (
            review[0].id.as_str(),
            review[0].name.as_str(),
            review[0].version.as_str()
        ),
        ("hello", "Hello", "0.1.0")
    );
    assert_eq!(
        review[0].capabilities,
        [
            capability(
                "workspace-read",
                "Read the files and folders in your workspace."
            ),
            capability("views", "Add pages to the rail and show panels in them."),
        ]
    );
    // None of its code runs before you approve.
    settle(&mut app);
    assert_eq!(rail_titles(&app), ["Inbox", "Files"]);
    assert_eq!(
        plugins::files_under(&fixture.app_data.path().join("plugins")),
        Vec::<String>::new()
    );

    assert!(app.approve_shipped_plugins());
    settle(&mut app);

    assert!(app.shipped_plugins().is_empty());
    assert_eq!(rail_titles(&app), ["Inbox", "Files", "Hello"]);
    let row = app
        .plugins()
        .into_iter()
        .find(|row| row.id == "hello")
        .unwrap();
    assert!(row.shipped && row.installed && row.running, "{row:?}");
    assert_eq!(row.approved.len(), 2);

    // The approval stays on this device.
    drop(app);
    let mut app = fixture.app();
    app.set_shipped_plugins(shipped.path().to_owned());
    assert!(app.shipped_plugins().is_empty());
    assert_eq!(rail_titles(&app), ["Inbox", "Files", "Hello"]);
    drop(app);

    // Another device asks again, and runs nothing until then.
    let root = fixture.root.clone();
    let fresh = tempfile::tempdir().unwrap();
    let mut app = App::launch(fresh.path());
    app.set_shipped_plugins(shipped.path().to_owned());
    app.open_folder(root);
    settle(&mut app);
    assert_eq!(app.shipped_plugins().len(), 1);
    assert_eq!(rail_titles(&app), ["Inbox", "Files"]);
}

#[test]
fn a_plugin_that_loops_allocates_or_panics_fails_alone_and_windows_keep_polling_until_it_stops() {
    let (Some(hello), Some(bad)) = (PluginFolder::hello(), misbehave(&["name = \"views\""])) else {
        return;
    };
    let fixture = Fixture::new(&[("Plan.md", b"# Plan\n")]);
    let mut app = fixture.app();
    plugins::install_and_enable(&mut app, hello.path(), "hello");
    plugins::install_and_enable(&mut app, bad.path(), "misbehave");
    let page = "plugin:misbehave:misbehave";
    let count = |app: &App| plugins::texts(app, plugins::HELLO_PAGE).unwrap()[1].clone();

    // The press returns at once, and the window keeps polling while the
    // plugin loops on its own thread.
    let pressed = Instant::now();
    app.press_plugin_button(page, "loop");
    assert!(pressed.elapsed() < Duration::from_millis(500));
    app.press_plugin_button(plugins::HELLO_PAGE, "add-one");
    let mut slowest = Duration::ZERO;
    poll_until(&mut app, "the hello plugin answers", |app| {
        count(app) == "Count: 1"
    });
    assert!(
        app.is_plugin_work_pending(),
        "the hello plugin answered while the other one still looped"
    );
    while app.is_plugin_work_pending() {
        let polled = Instant::now();
        app.poll();
        slowest = slowest.max(polled.elapsed());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        slowest < Duration::from_millis(250),
        "a poll took {slowest:?}"
    );
    let reason = plugins::texts(&app, page).expect_err("the page says why");
    assert!(reason.contains("1 second limit"), "{reason}");

    app.press_plugin_button(page, "allocate");
    settle(&mut app);
    let reason = plugins::texts(&app, page).expect_err("the page says why");
    assert!(reason.contains("256 MB"), "{reason}");
    assert_eq!(count(&app), "Count: 1");

    // A third failure within a minute stops the plugin for the session.
    app.press_plugin_button(page, "panic");
    settle(&mut app);

    let row = app
        .plugins()
        .into_iter()
        .find(|row| row.id == "misbehave")
        .unwrap();
    assert!(row.failed && !row.running, "{row:?}");
    assert!(row.problem.is_some());
    assert!(row.enabled, "the workspace still lists it as enabled");
    let manifest: local_core::WorkspaceManifest =
        serde_yaml_ng::from_slice(&fixture.read(".noura/workspace.yaml")).unwrap();
    assert!(manifest.enabled_plugins.contains(&"misbehave".to_owned()));
    assert!(!rail_titles(&app).contains(&"Misbehave".to_owned()));
    app.press_plugin_button(plugins::HELLO_PAGE, "add-one");
    settle(&mut app);
    assert_eq!(count(&app), "Count: 2");

    assert!(app.restart_plugin("misbehave"));
    settle(&mut app);

    assert_eq!(
        plugins::texts(&app, page),
        Ok(vec!["Still fine.".to_owned()])
    );
    let row = app
        .plugins()
        .into_iter()
        .find(|row| row.id == "misbehave")
        .unwrap();
    assert!(
        row.running && !row.failed && row.problem.is_none(),
        "{row:?}"
    );
}
