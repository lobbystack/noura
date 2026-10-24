//! Seam 3: what a plugin registers under each registration capability
//! (commands, Inbox sections, a page in Settings and tools) and the views
//! it draws there. Events come back by node ID, and the host stores no
//! setting values for the plugin.

mod support;

use std::path::Path;

use plugin_host::{Event, PluginError, Surface};
use support::plugins;

const ALL: [&str; 5] = [
    "name = \"views\"",
    "name = \"commands\"",
    "name = \"inbox-sections\"",
    "name = \"settings\"",
    "name = \"tools\"",
];

/// Every file under `folder`, with its bytes.
fn files(folder: &Path) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    let mut folders = vec![folder.to_owned()];
    while let Some(next) = folders.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                found.push((path.to_string_lossy().into_owned(), bytes));
            }
        }
    }
    found.sort();
    found
}

#[test]
fn a_plugin_registers_commands_inbox_sections_a_settings_page_and_tools() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (plugin, _folders) = plugins::running(&probe, &ALL, None);

    let commands: Vec<(String, String, bool, bool)> = plugin
        .commands()
        .unwrap()
        .into_iter()
        .map(|command| (command.id, command.title, command.palette, command.plugins))
        .collect();
    assert_eq!(
        commands,
        [
            ("say-hello".into(), "Say hello".into(), true, false),
            (
                "describe".into(),
                "Describe the workspace".into(),
                false,
                true
            ),
            ("relay".into(), "Relay a command".into(), false, true),
        ]
    );
    let sections = plugin.inbox_sections().unwrap();
    assert_eq!(
        sections
            .iter()
            .map(|section| (section.id.as_str(), section.title.as_str()))
            .collect::<Vec<_>>(),
        [("probe", "Probe section")]
    );
    assert_eq!(
        plugin.settings_page().unwrap(),
        Some("Probe settings".to_owned())
    );
    let tools = plugin.tools().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].description, "Repeats the text it gets.");
    let schema: serde_json::Value = serde_json::from_str(&tools[0].input_schema).unwrap();
    assert_eq!(schema["required"], serde_json::json!(["text"]));
}

#[test]
fn without_its_capability_a_plugin_registers_nothing_there() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (plugin, _folders) = plugins::running(&probe, &["name = \"views\""], None);

    let denied = |capability| PluginError::CapabilityNotDeclared { capability };
    assert_eq!(plugin.commands().err(), Some(denied("commands")));
    assert_eq!(
        plugin.run_command("say-hello", "").err(),
        Some(denied("commands"))
    );
    assert_eq!(
        plugin.inbox_sections().err(),
        Some(denied("inbox-sections"))
    );
    assert_eq!(
        plugin
            .render_surface(&Surface::InboxSection("probe".into()))
            .err(),
        Some(denied("inbox-sections"))
    );
    assert_eq!(plugin.settings_page().err(), Some(denied("settings")));
    assert_eq!(
        plugin.render_surface(&Surface::Settings).err(),
        Some(denied("settings"))
    );
    assert_eq!(plugin.tools().err(), Some(denied("tools")));
    assert_eq!(plugin.call_tool("echo", "{}").err(), Some(denied("tools")));
    // None of these were failures of the plugin.
    assert_eq!(plugin.health().recent_failures, 0);
}

#[test]
fn a_settings_page_draws_from_nodes_and_sends_events_back_by_node_id_and_the_host_stores_no_values()
{
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (plugin, folders) = plugins::running(&probe, &ALL, None);
    let before: Vec<_> = folders
        .iter()
        .flat_map(|folder| files(folder.path()))
        .collect();

    let nodes = plugin.render_surface(&Surface::Settings).unwrap();
    assert_eq!(plugins::texts(&nodes), ["Pressed 0 times"]);
    plugin
        .handle_surface_event(&Surface::Settings, &Event::Pressed("count".into()))
        .unwrap();
    plugin
        .handle_surface_event(&Surface::Settings, &Event::Pressed("count".into()))
        .unwrap();

    let nodes = plugin.render_surface(&Surface::Settings).unwrap();
    assert_eq!(plugins::texts(&nodes), ["Pressed 2 times"]);
    assert_eq!(
        plugins::texts(&plugin.render("events").unwrap()),
        ["settings pressed count", "settings pressed count"]
    );
    let after: Vec<_> = folders
        .iter()
        .flat_map(|folder| files(folder.path()))
        .collect();
    assert_eq!(after, before);
}

#[test]
fn an_inbox_section_draws_from_nodes_and_sends_events_back_by_node_id() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (plugin, _folders) = plugins::running(&probe, &ALL, None);
    let section = Surface::InboxSection("probe".into());

    let nodes = plugin.render_surface(&section).unwrap();
    plugin
        .handle_surface_event(&section, &Event::Pressed("press".into()))
        .unwrap();

    assert_eq!(plugins::texts(&nodes), ["Section probe is running."]);
    assert_eq!(
        plugins::texts(&plugin.render("events").unwrap()),
        ["inbox probe pressed press"]
    );
}

#[test]
fn commands_and_tools_run_in_the_plugin() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (plugin, _folders) = plugins::running(&probe, &ALL, None);

    assert_eq!(plugin.run_command("say-hello", ""), Ok(Ok("hello".into())));
    assert_eq!(
        plugin.call_tool("echo", r#"{"text":"hi"}"#),
        Ok(Ok(r#"{"text":"hi"}"#.into()))
    );
    assert_eq!(
        plugin.call_tool("missing", "{}"),
        Ok(Err("The probe has no tool missing.".into()))
    );
    assert_eq!(
        plugins::texts(&plugin.render("events").unwrap()),
        [
            "command say-hello",
            r#"tool echo {"text":"hi"}"#,
            "tool missing {}"
        ]
    );
}

#[test]
fn a_plugin_that_offers_nothing_under_a_capability_registers_nothing_there() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    // The hello plugin writes only its view, so the SDK's defaults answer
    // the other interfaces.
    let (plugin, _folders) = plugins::running(&hello, &ALL, None);

    assert_eq!(plugin.commands(), Ok(Vec::new()));
    assert_eq!(plugin.inbox_sections(), Ok(Vec::new()));
    assert_eq!(plugin.settings_page(), Ok(None));
    assert_eq!(plugin.tools(), Ok(Vec::new()));
}
