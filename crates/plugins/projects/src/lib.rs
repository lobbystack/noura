//! The Projects plugin. It registers the `project` object type with
//! today's statuses. Other plugins create and change projects through its
//! `create` and `update` commands, which [`commands`] documents.
//!
//! Build it with `crates/plugin-sdk/build-plugins.sh`, then install the
//! folder holding `plugin.toml` and the built `projects_plugin.wasm`.

mod commands;
mod project;

use plugin_sdk::{Command, ObjectType, Plugin};

use project::{PROJECT, SCHEMA};

struct Projects;

impl Plugin for Projects {
    fn object_types() -> Vec<ObjectType> {
        vec![ObjectType {
            name: PROJECT.into(),
            schema: SCHEMA.into(),
        }]
    }

    fn commands() -> Vec<Command> {
        let offered = |id: &str, title: &str| Command {
            id: id.into(),
            title: title.into(),
            palette: false,
            plugins: true,
        };
        vec![
            offered("create", "Create a project"),
            offered("update", "Update a project"),
        ]
    }

    fn run_command(id: String, input: String) -> Result<String, String> {
        commands::run(&id, &input)
    }
}

plugin_sdk::export!(Projects);
