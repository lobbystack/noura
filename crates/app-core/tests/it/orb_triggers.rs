//! What starts Orbs without you. A schedule missed while Noura was closed
//! runs once at the next launch, with the checklist in `Heartbeat.md`, and
//! a key keeps it from running twice. A paused Orb fires nothing. After a
//! week away Noura holds every Orb's triggers until you say whether to
//! keep them running. A plugin's event starts the Orbs whose triggers name
//! it, once for each object revision, but an event an Orb's own tool call
//! causes starts no Orb.

use crate::support;
use std::{sync::Arc, time::Duration};

use app_core::{AiOrbRunStatus, AiOrbStart, AiOrbState, App};
use support::{
    FakeSystem, Fixture, ai,
    orbs::{self, MEETINGS, MEETINGS_FILE, Reply, Scripted},
    plugins,
};

/// The Orb Triage, which lists the probe's `emit` tool, and the Meetings
/// Orb, whose trigger names the probe's event `ping`.
const TRIAGE: &str = "orb_01jd8x3k9q2m7v4c6b1n0p5r9a";

fn fixture_with(files: &[(String, Vec<u8>)]) -> Fixture {
    let files: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    Fixture::new(&files)
}

/// Noura on `fixture` at `now` in Toronto, talking to `provider`.
fn launch(fixture: &Fixture, now: &str, provider: &Arc<Scripted>) -> (App, Arc<FakeSystem>) {
    let system = FakeSystem::new();
    system.set_clock(now, "America/Toronto");
    let mut app = fixture.app_with(system.clone());
    app.set_ai_client(provider.clone());
    if app.ai_providers().is_empty() {
        app.save_ai_provider(ai::provider("Local", "ollama", "llama3", None))
            .unwrap();
    }
    (app, system)
}

/// Check the triggers and let what they started finish.
fn check(app: &mut App) {
    app.check_orbs();
    support::settle(app);
    for _ in 0..100 {
        if app
            .orbs()
            .unwrap()
            .iter()
            .all(|orb| orb.state != AiOrbState::Working)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
        support::settle(app);
    }
}

#[test]
fn a_schedule_missed_while_noura_was_closed_runs_once_at_launch_with_its_checklist() {
    let mut files = orbs::meetings(&[], &["schedule 30 8 * * 1-5"], "2026-11-16T06:00:00Z");
    files.push((
        "Orbs/Meetings/Heartbeat.md".to_owned(),
        b"- Read today's meetings.\n- Say HEARTBEAT_OK when nothing needs you.\n".to_vec(),
    ));
    let fixture = fixture_with(&files);
    let provider = Scripted::new(|_| Reply::Text("HEARTBEAT_OK".to_owned()));
    // Wednesday 18 November 2026, 13:00 in Toronto: Monday's, Tuesday's
    // and Wednesday's 8:30 went by while Noura was closed.
    let (mut app, system) = launch(&fixture, "2026-11-18T18:00:00Z", &provider);

    check(&mut app);

    let runs = orbs::runs(&app, MEETINGS);
    assert_eq!(runs.len(), 1, "one run for every missed time: {runs:?}");
    assert_eq!(runs[0].status, AiOrbRunStatus::Succeeded);
    assert_eq!(runs[0].started_by, AiOrbStart::Schedule);
    assert_eq!(runs[0].trigger.as_deref(), Some("schedule 30 8 * * 1-5"));
    assert!(runs[0].quiet, "HEARTBEAT_OK is a quiet answer");
    let record = &orbs::run_files(&fixture.root)[0].1;
    assert_eq!(
        ai::frontmatter(record, "slot").as_deref(),
        Some("2026-11-18T13:30:00Z"),
        "it runs for the last time it missed"
    );
    let asked = provider.requests();
    assert_eq!(
        asked[0].messages,
        [
            "user: **Schedule** `30 8 * * 1-5` · 2026-11-18 08:30\n\n- Read today's meetings.\n\
          - Say HEARTBEAT_OK when nothing needs you."
        ]
    );

    // Checking again, or after a relaunch, starts nothing more.
    check(&mut app);
    assert_eq!(orbs::runs(&app, MEETINGS).len(), 1);
    drop(system);
    app.close_window();
    drop(app);
    let (mut app, system) = launch(&fixture, "2026-11-18T18:05:00Z", &provider);
    check(&mut app);
    assert_eq!(orbs::runs(&app, MEETINGS).len(), 1);

    // The next morning's time runs while Noura stays open.
    system.set_clock("2026-11-19T13:31:00Z", "America/Toronto");
    check(&mut app);
    let runs = orbs::runs(&app, MEETINGS);
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(
        ai::frontmatter(&orbs::run_files(&fixture.root)[1].1, "slot").as_deref(),
        Some("2026-11-19T13:30:00Z")
    );
}

#[test]
fn a_paused_orb_fires_no_trigger() {
    let mut files = orbs::meetings(&[], &["schedule 30 8 * * *"], "2026-11-16T06:00:00Z");
    files[0].1 = String::from_utf8(files[0].1.clone())
        .unwrap()
        .replace("memory:", "paused: true\nmemory:")
        .into_bytes();
    let fixture = fixture_with(&files);
    let provider = Scripted::new(|_| Reply::Text("Done.".to_owned()));
    let (mut app, _) = launch(&fixture, "2026-11-18T18:00:00Z", &provider);

    check(&mut app);

    assert!(orbs::runs(&app, MEETINGS).is_empty());
    assert_eq!(orbs::orb(&app, MEETINGS).state, AiOrbState::Paused);
    // Letting it fire again skips what it missed while paused, since its
    // file changed now.
    let revision = orbs::orb(&app, MEETINGS).revision;
    app.set_orb_paused(MEETINGS, false, &revision).unwrap();
    check(&mut app);
    assert!(orbs::runs(&app, MEETINGS).is_empty());
    assert!(provider.requests().is_empty());
}

#[test]
fn after_a_week_away_noura_holds_the_triggers_until_you_answer() {
    let files = orbs::meetings(&[], &["schedule 30 8 * * *"], "2026-11-01T06:00:00Z");
    let fixture = fixture_with(&files);
    // You last used Noura on this device ten days ago.
    let activity = fixture.app_data.path().join("ai/orbs.json");
    std::fs::create_dir_all(activity.parent().unwrap()).unwrap();
    std::fs::write(&activity, br#"{"last_active":"2026-11-08T18:00:00Z"}"#).unwrap();
    let provider = Scripted::new(|_| Reply::Text("Done.".to_owned()));
    let (mut app, _) = launch(&fixture, "2026-11-18T18:00:00Z", &provider);

    check(&mut app);

    assert!(app.orbs_held());
    assert_eq!(orbs::orb(&app, MEETINGS).state, AiOrbState::Held);
    assert!(orbs::runs(&app, MEETINGS).is_empty());
    assert!(provider.requests().is_empty());

    // The hold survives a relaunch until you answer.
    app.close_window();
    drop(app);
    let (mut app, _) = launch(&fixture, "2026-11-18T18:10:00Z", &provider);
    check(&mut app);
    assert!(app.orbs_held());
    assert!(orbs::runs(&app, MEETINGS).is_empty());

    // Keep them running: the schedule catches up once.
    app.keep_orbs_running(true).unwrap();
    check(&mut app);
    assert!(!app.orbs_held());
    assert_eq!(orbs::runs(&app, MEETINGS).len(), 1);
}

#[test]
fn answering_no_after_a_week_away_pauses_every_orb_with_triggers() {
    let files = orbs::meetings(&[], &["schedule 30 8 * * *"], "2026-11-01T06:00:00Z");
    let fixture = fixture_with(&files);
    let activity = fixture.app_data.path().join("ai/orbs.json");
    std::fs::create_dir_all(activity.parent().unwrap()).unwrap();
    std::fs::write(&activity, br#"{"last_active":"2026-11-08T18:00:00Z"}"#).unwrap();
    let provider = Scripted::new(|_| Reply::Text("Done.".to_owned()));
    let (mut app, _) = launch(&fixture, "2026-11-18T18:00:00Z", &provider);
    check(&mut app);
    assert!(app.orbs_held());

    app.keep_orbs_running(false).unwrap();
    check(&mut app);

    assert!(!app.orbs_held());
    let file = String::from_utf8(fixture.read(MEETINGS_FILE)).unwrap();
    assert!(file.contains("\npaused: true\n"), "{file}");
    assert!(orbs::runs(&app, MEETINGS).is_empty());
}

#[test]
fn an_event_starts_its_orbs_once_per_object_but_never_from_an_orbs_own_tool_call() {
    let mut files = orbs::meetings(&[], &["event probe.ping"], "2026-11-01T06:00:00Z");
    files.push((
        "Orbs/Triage/Triage.md".to_owned(),
        orbs::orb_file(
            TRIAGE,
            "tools:\n- probe__emit\n",
            "Call the emit tool once.\n",
            "2026-11-01T06:00:00Z",
        ),
    ));
    files.push((
        "Notes/Plan.md".to_owned(),
        b"---\nid: note_01jd8x3k9q2m7v4c6b1n0p5r9b\ntype: note\n---\nShip on Friday.\n".to_vec(),
    ));
    let fixture = fixture_with(&files);
    let provider = Scripted::new(|request| {
        let triage = request.system[0].starts_with("You are Triage");
        if triage && !orbs::has_result(request) {
            Reply::Call("probe__emit".to_owned(), serde_json::json!({}))
        } else {
            Reply::Text("Done.".to_owned())
        }
    });
    let (mut app, _) = launch(&fixture, "2026-11-18T18:00:00Z", &provider);
    let Some(probe) = plugins::folder_for(
        "probe_plugin",
        "probe",
        &[
            "name = \"views\"",
            "name = \"tools\"",
            "name = \"events\"\nevents = [{ id = \"ping\", title = \"Ping\" }]",
        ],
    ) else {
        return;
    };
    plugins::install_and_enable(&mut app, probe.path(), "probe");

    // Triage's tool call emits the event: Meetings stays still.
    app.send_to_orb(TRIAGE, "Go.").unwrap();
    orbs::wait_for_runs(&mut app, TRIAGE, 1);
    check(&mut app);
    let triage = orbs::runs(&app, TRIAGE);
    assert_eq!(triage[0].tool_calls, 1);
    let emitted = support::ai::tool_result(&mut app, &fixture.root, "call_1");
    assert_eq!(emitted["answer"], "emitted ping", "{emitted}");
    assert!(
        orbs::runs(&app, MEETINGS).is_empty(),
        "an Orb's own event starts no Orb"
    );

    // The same event from the plugin itself starts Meetings, with the
    // note it names, once for the note as it stands.
    let note = "note_01jd8x3k9q2m7v4c6b1n0p5r9b";
    app.press_plugin_button("plugin:probe:hello", &format!("emit:ping:{note}"));
    support::settle(&mut app);
    check(&mut app);
    app.press_plugin_button("plugin:probe:hello", &format!("emit:ping:{note}"));
    support::settle(&mut app);
    check(&mut app);
    let runs = orbs::runs(&app, MEETINGS);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0].started_by, AiOrbStart::Event);
    assert_eq!(runs[0].trigger.as_deref(), Some("event probe.ping"));
    let opening = provider
        .requests()
        .into_iter()
        .find(|request| request.system[0].starts_with("You are Meetings"))
        .unwrap()
        .messages[0]
        .clone();
    assert_eq!(opening, "user: **Ping** · [[Plan]]\n\nShip on Friday.");

    // Once the note changes, the event starts it again.
    let plan = fixture.root.join("Notes/Plan.md");
    let text = std::fs::read_to_string(&plan).unwrap();
    std::fs::write(&plan, text.replace("Friday", "Monday")).unwrap();
    support::settle(&mut app);
    app.press_plugin_button("plugin:probe:hello", &format!("emit:ping:{note}"));
    support::settle(&mut app);
    check(&mut app);
    assert_eq!(orbs::runs(&app, MEETINGS).len(), 2);

    // An event the manifest doesn't declare is refused.
    let seen = plugins::texts(&app, "plugin:probe:events").unwrap().len();
    app.press_plugin_button("plugin:probe:hello", "emit:pong");
    support::settle(&mut app);
    let refused = plugins::texts(&app, "plugin:probe:events").unwrap()[seen..].to_vec();
    assert!(refused[0].contains("failed"), "{refused:?}");
}
