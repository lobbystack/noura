//! Seam 3: a plugin calls another plugin's command only when its manifest
//! names the command, or its plugin, as a target, and the target plugin
//! runs and offers it. The command runs in the target with the target's
//! capabilities: here the target reads a workspace the caller can't, and
//! creates an object of its own type, which the caller can't. An
//! undeclared target, a target that is off, and a call cycle back into a
//! busy plugin each return a structured error instead of hanging. A target
//! that fails tells the caller only that it failed, and the router's
//! listener hears of each failure, so the app learns when it stops.

mod support;

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use plugin_host::{
    Entry, EntryKind, Limits, Listed, MAX_FAILURES, ObjectTypes, Router, RunningPlugin, Services,
    WorkspacePath, WorkspaceReads,
};
use support::plugins::{self, FakeObjects, FakeWorkspace};

/// Two plugins built from the probe: `target`, which reads a workspace of
/// two files, and `caller`, which reads nothing itself.
struct Pair {
    router: Arc<Router>,
    caller: Arc<RunningPlugin>,
    target: Arc<RunningPlugin>,
    workspace: Arc<FakeWorkspace>,
    _folders: [[tempfile::TempDir; 2]; 2],
}

fn listing() -> Vec<Listed> {
    ["Plan.md", "Notes.md"]
        .into_iter()
        .map(|path| {
            Listed::Entry(Entry {
                path: WorkspacePath::new(path).unwrap(),
                kind: EntryKind::File,
                object_id: None,
                object_type: None,
                revision: None,
            })
        })
        .collect()
}

/// The pair, with `caller_targets` as the caller's command targets and
/// `target_targets` as the target's.
fn pair(probe: &[u8], caller_targets: &str, target_targets: &str) -> Pair {
    let router = Arc::new(Router::default());
    let workspace = Arc::new(FakeWorkspace {
        listing: listing(),
        ..FakeWorkspace::default()
    });
    let reads: Arc<dyn WorkspaceReads> = workspace.clone();
    let (target, target_folders) = plugins::start(
        "target",
        probe,
        &[
            "name = \"workspace-read\"",
            "name = \"views\"",
            &format!("name = \"commands\"\ntargets = {target_targets}"),
        ],
        Services {
            workspace: Some(reads),
            router: Arc::downgrade(&router),
            ..Services::default()
        },
        Limits::default(),
    );
    let (caller, caller_folders) = plugins::start(
        "caller",
        probe,
        &[
            "name = \"views\"",
            &format!("name = \"commands\"\ntargets = {caller_targets}"),
        ],
        Services {
            router: Arc::downgrade(&router),
            ..Services::default()
        },
        Limits::default(),
    );
    router.insert(target.clone());
    router.insert(caller.clone());
    Pair {
        router,
        caller,
        target,
        workspace,
        _folders: [target_folders, caller_folders],
    }
}

/// What the caller's page `call:<call>` shows.
fn call(caller: &RunningPlugin, call: &str) -> String {
    let nodes = caller
        .render(&format!("call:{call}"))
        .expect("the caller renders");
    plugins::texts(&nodes).join("\n")
}

#[test]
fn a_declared_target_runs_in_its_plugin_with_that_plugins_capabilities() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let pair = pair(&probe, "[\"target/describe\"]", "[\"caller\"]");

    let answer = call(&pair.caller, "target/describe:");

    assert_eq!(answer, "answer probe read 2 entries");
    assert_eq!(pair.workspace.calls(), ["list"]);
    // The caller can't read the workspace itself.
    assert_eq!(
        plugins::texts(&pair.caller.render("list").unwrap()),
        ["error capability-not-declared workspace-read"]
    );
    assert_eq!(pair.workspace.calls(), ["list"]);
    // The target heard the call, with its input.
    assert_eq!(plugins::texts(&pair.target_events()), ["command describe"]);
}

#[test]
fn a_caller_creates_the_targets_objects_through_its_command_without_writing_them_itself() {
    let (Some(probe), Some(recipes)) = (
        plugins::built("probe_plugin"),
        plugins::built("recipes_plugin"),
    ) else {
        return;
    };
    let router = Arc::new(Router::default());
    let types = ObjectTypes::new();
    let objects = Arc::new(FakeObjects::default());
    let (target, _target_folders) = plugins::start(
        "recipes",
        &recipes,
        &[
            "name = \"object-types\"",
            "name = \"views\"",
            "name = \"commands\"",
        ],
        Services {
            objects: Some(objects.clone()),
            types: types.clone(),
            router: Arc::downgrade(&router),
            ..Services::default()
        },
        Limits::default(),
    );
    target.register_object_types().unwrap();
    let (caller, _caller_folders) = plugins::start(
        "caller",
        &probe,
        &[
            "name = \"views\"",
            "name = \"commands\"\ntargets = [\"recipes/run\"]",
        ],
        Services {
            types: types.clone(),
            router: Arc::downgrade(&router),
            ..Services::default()
        },
        Limits::default(),
    );
    router.insert(target.clone());
    router.insert(caller.clone());

    // The caller can't create a recipe itself.
    assert_eq!(
        plugins::texts(&caller.render("create-object:recipe:Soup").unwrap()),
        ["error capability-not-declared object-types"]
    );
    assert!(objects.calls().is_empty(), "{:?}", objects.calls());

    let answer = call(&caller, "recipes/run:create|recipe|Soup|-|{}|Simmer.");

    assert!(
        answer.starts_with(
            "answer object\trecipe_01jbcdefghjkmnpqrstvwxyz0a\trev-1\trecipes/Soup.md"
        ),
        "{answer}"
    );
    assert_eq!(objects.calls(), ["create recipe Soup"]);
    // The command offers only the target's own types.
    assert_eq!(
        call(&caller, "recipes/run:create|note|Plan|-|{}|"),
        "answer error\tcapability-not-declared\tobject-types"
    );
    assert_eq!(objects.calls(), ["create recipe Soup"]);
}

#[test]
fn a_plugin_target_lets_the_caller_run_every_command_that_plugin_offers_to_plugins() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let pair = pair(&probe, "[\"target\"]", "[\"caller\"]");

    assert_eq!(
        call(&pair.caller, "target/describe:"),
        "answer probe read 2 entries"
    );
    // `say-hello` is only for the command palette.
    assert_eq!(
        call(&pair.caller, "target/say-hello:"),
        "error target-unavailable target/say-hello"
    );
    assert_eq!(
        call(&pair.caller, "target/missing:"),
        "error target-unavailable target/missing"
    );
}

#[test]
fn a_target_the_manifest_doesnt_name_is_refused_without_reaching_the_target() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let pair = pair(&probe, "[\"target/relay\"]", "[\"caller\"]");

    assert_eq!(
        call(&pair.caller, "target/describe:"),
        "error target-not-declared target/describe"
    );
    assert_eq!(
        call(&pair.caller, "other/describe:"),
        "error target-not-declared other/describe"
    );
    assert!(pair.workspace.calls().is_empty());
    assert!(plugins::texts(&pair.target_events()).is_empty());
}

#[test]
fn a_plugin_without_the_commands_capability_calls_nothing() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (plugin, _folders) = plugins::running(&probe, &["name = \"views\""], None);

    assert_eq!(
        call(&plugin, "target/describe:"),
        "error capability-not-declared commands"
    );
}

#[test]
fn a_target_that_is_off_returns_target_unavailable() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let pair = pair(&probe, "[\"target\"]", "[\"caller\"]");

    pair.router.remove("target");

    assert_eq!(
        call(&pair.caller, "target/describe:"),
        "error target-unavailable target/describe"
    );
    assert!(pair.workspace.calls().is_empty());
}

#[test]
fn a_call_cycle_back_into_a_busy_plugin_returns_busy_at_once() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    // The caller asks the target to relay a call back to the caller,
    // which is still waiting for the target.
    let pair = pair(&probe, "[\"target\"]", "[\"caller\"]");
    let started = Instant::now();

    let answer = call(&pair.caller, "target/relay:caller/describe:");

    assert_eq!(answer, "answer error busy caller");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "the cycle waited {:?}",
        started.elapsed()
    );
    // Both plugins keep answering.
    assert_eq!(
        call(&pair.caller, "target/describe:"),
        "answer probe read 2 entries"
    );
}

#[test]
fn a_command_that_calls_itself_returns_busy() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let pair = pair(&probe, "[\"target\"]", "[\"caller\", \"target\"]");

    assert_eq!(
        call(&pair.caller, "target/relay:target/describe:"),
        "answer error busy target"
    );
}

#[test]
fn a_target_that_fails_tells_the_caller_only_that_and_stops_after_failing_again_and_again() {
    let (Some(probe), Some(misbehave)) = (
        plugins::built("probe_plugin"),
        plugins::built("misbehave_plugin"),
    ) else {
        return;
    };
    let router = Arc::new(Router::default());
    let heard = Arc::new(Mutex::new(Vec::new()));
    let listener = heard.clone();
    router.on_called(Arc::new(move |id: &str| {
        listener.lock().unwrap().push(id.to_owned());
    }));
    // The misbehave plugin's `loop` command runs past its deadline.
    let (target, _target_folders) = plugins::start(
        "misbehave",
        &misbehave,
        &["name = \"views\"", "name = \"commands\""],
        Services {
            router: Arc::downgrade(&router),
            ..Services::default()
        },
        Limits {
            call_deadline: Duration::from_millis(200),
            ..Limits::default()
        },
    );
    let (caller, _caller_folders) = plugins::start(
        "caller",
        &probe,
        &[
            "name = \"views\"",
            "name = \"commands\"\ntargets = [\"misbehave\"]",
        ],
        Services {
            router: Arc::downgrade(&router),
            ..Services::default()
        },
        Limits::default(),
    );
    router.insert(target.clone());
    router.insert(caller.clone());

    for _ in 0..MAX_FAILURES {
        assert_eq!(
            call(&caller, "misbehave/loop:"),
            "error failed The misbehave plugin failed while running loop."
        );
    }

    assert_eq!(*heard.lock().unwrap(), ["misbehave"; MAX_FAILURES]);
    assert!(target.health().stopped);
    assert_eq!(
        call(&caller, "misbehave/loop:"),
        "error target-unavailable misbehave/loop"
    );
    // The caller keeps answering.
    assert_eq!(
        plugins::texts(&caller.render("hello").unwrap()),
        ["Probe is running."]
    );
}

impl Pair {
    /// The nodes of the target's `events` page: the commands it ran.
    fn target_events(&self) -> Vec<plugin_host::Node> {
        self.target.render("events").expect("the target renders")
    }
}
