//! Seam 3: what an update adds to a plugin's capabilities, what a plugin
//! may use once you approved some of them, and revoking one capability,
//! host or target at a time.

use plugin_host::{Capability, Grant};

fn network(hosts: &[&str]) -> Capability {
    Capability::Network {
        hosts: hosts.iter().map(|host| (*host).to_owned()).collect(),
    }
}

fn commands(targets: &[&str]) -> Capability {
    Capability::Commands {
        targets: targets.iter().map(|target| (*target).to_owned()).collect(),
    }
}

#[test]
fn an_update_adds_the_capabilities_hosts_and_targets_the_installed_build_lacks() {
    let installed = [
        Capability::Views,
        network(&["api.example.com"]),
        commands(&["tasks"]),
    ];
    let update = [
        Capability::Views,
        Capability::WorkspaceRead,
        network(&["api.example.com", "tiles.example.com"]),
        commands(&["projects/create", "tasks"]),
    ];

    assert_eq!(
        Capability::added(&installed, &update),
        [
            Capability::WorkspaceRead,
            network(&["tiles.example.com"]),
            commands(&["projects/create"]),
        ]
    );
}

#[test]
fn an_update_that_only_removes_adds_nothing() {
    let installed = [
        Capability::Views,
        Capability::WorkspaceRead,
        network(&["api.example.com", "tiles.example.com"]),
        commands(&["tasks"]),
    ];
    let update = [
        Capability::Views,
        network(&["api.example.com"]),
        commands(&[]),
    ];

    assert!(Capability::added(&installed, &update).is_empty());
}

#[test]
fn a_plugin_may_use_only_what_it_declares_and_you_approved() {
    let declared = [
        Capability::Views,
        Capability::WorkspaceRead,
        network(&["api.example.com", "tiles.example.com"]),
        commands(&["projects", "tasks"]),
    ];
    let approved = [
        Capability::Views,
        Capability::Tools,
        network(&["tiles.example.com", "other.example.com"]),
        commands(&["tasks"]),
    ];

    assert_eq!(
        Capability::granted(&declared, &approved),
        [
            Capability::Views,
            network(&["tiles.example.com"]),
            commands(&["tasks"]),
        ]
    );
}

#[test]
fn approving_merges_hosts_and_targets_into_what_you_approved() {
    let mut approved = vec![Capability::Views, network(&["api.example.com"])];

    Capability::approve(
        &mut approved,
        &[
            network(&["tiles.example.com"]),
            commands(&["tasks"]),
            Capability::Views,
        ],
    );

    assert_eq!(
        approved,
        [
            Capability::Views,
            network(&["api.example.com", "tiles.example.com"]),
            commands(&["tasks"]),
        ]
    );
}

#[test]
fn revoking_takes_away_one_capability_host_or_target() {
    let mut approved = vec![
        Capability::Views,
        Capability::WorkspaceRead,
        network(&["api.example.com", "tiles.example.com"]),
        commands(&["projects", "tasks"]),
    ];

    Capability::revoke(&mut approved, &Grant::Host("api.example.com".into()));
    Capability::revoke(&mut approved, &Grant::Target("tasks".into()));
    Capability::revoke(&mut approved, &Grant::Capability("workspace-read".into()));

    assert_eq!(
        approved,
        [
            Capability::Views,
            network(&["tiles.example.com"]),
            commands(&["projects"]),
        ]
    );

    // The last host takes the network capability with it; a command
    // capability without targets still adds commands.
    Capability::revoke(&mut approved, &Grant::Host("tiles.example.com".into()));
    Capability::revoke(&mut approved, &Grant::Target("projects".into()));
    assert_eq!(approved, [Capability::Views, commands(&[])]);
}

#[test]
fn each_host_and_target_is_its_own_grant_with_a_plain_description() {
    let grants: Vec<(Grant, String)> = [
        Capability::WorkspaceRead,
        network(&["api.example.com", "tiles.example.com"]),
        commands(&["tasks", "projects/create"]),
        Capability::InboxSections,
        Capability::Settings,
        Capability::Tools,
    ]
    .iter()
    .flat_map(Capability::grants)
    .collect();

    assert_eq!(
        grants,
        [
            (
                Grant::Capability("workspace-read".into()),
                "Read the files and folders in your workspace.".into()
            ),
            (
                Grant::Host("api.example.com".into()),
                "Connect to api.example.com.".into()
            ),
            (
                Grant::Host("tiles.example.com".into()),
                "Connect to tiles.example.com.".into()
            ),
            (
                Grant::Capability("commands".into()),
                "Add commands to the command palette and offer them to other plugins.".into()
            ),
            (
                Grant::Target("tasks".into()),
                "Run the commands the tasks plugin offers.".into()
            ),
            (
                Grant::Target("projects/create".into()),
                "Run the create command of the projects plugin.".into()
            ),
            (
                Grant::Capability("inbox-sections".into()),
                "Add sections to the Inbox.".into()
            ),
            (
                Grant::Capability("settings".into()),
                "Add a page to Settings.".into()
            ),
            (
                Grant::Capability("tools".into()),
                "Offer tools that AI assistants can use.".into()
            ),
        ]
    );
}
